//! 高清（720P）下载通道。
//!
//! **原理**：央视在线点播的 720P 高清流（enc/h5e 通道）分片是私有加密的
//! （TS 结构完整，但音视频负载为密文，没有标准 HLS KEY 标签），解密由官方播放页
//! 内的 WASM 模块（CNTV_jsdecVOD*）在 hls.js demux 阶段完成。
//!
//! 本模块驱动真实播放页（headless Chromium），挂钩 `SourceBuffer.appendBuffer`，
//! 把「解密 + 重封装后」的 fMP4 分段拦截落盘，再无损合并为 MP4。因此产物天然就是
//! 官方播放器实际解码的画面，画质 1280×720。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use once_cell::sync::Lazy;
use regex::Regex;
use serde::Serialize;

use crate::cdp::CdpSession;
use crate::error::{Error, Result};
use crate::ffmpeg;

/// 高清通道进度。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HdProgress {
    /// 已缓冲到的时间点（秒）
    pub buf_end: f64,
    /// 目标时间点（秒）
    pub target: f64,
    /// 已落盘的分片数
    pub saved: usize,
    pub resolution: String,
}

pub type ProgressFn = Arc<dyn Fn(HdProgress) + Send + Sync>;

/// 拦截 `SourceBuffer.appendBuffer`，把解密后的数据留在内存里等主循环取走。
const HOOK_JS: &str = r#"(function(){
  if (window.__cntvHooked) return; window.__cntvHooked = 1;
  window.__bufs = [];
  var oa = SourceBuffer.prototype.appendBuffer;
  SourceBuffer.prototype.appendBuffer = function (data) {
    try {
      var u8 = new Uint8Array(data instanceof ArrayBuffer ? data : data.buffer, data.byteOffset || 0, data.byteLength);
      window.__bufs.push({ k: this.__kind || "data", d: u8 });
    } catch (e) {}
    return oa.call(this, data);
  };
  var oadd = MediaSource.prototype.addSourceBuffer;
  MediaSource.prototype.addSourceBuffer = function (mime) {
    var sb = oadd.call(this, mime);
    sb.__kind = String(mime).indexOf("audio") >= 0 ? "audio" : "video";
    return sb;
  };
})();"#;

/// 把缓冲区里的数据取走（base64 传输）。
const DRAIN_JS: &str = r#"(() => {
  const out = (window.__bufs || []).map(it => {
    const u8 = it.d; let s = "";
    for (let i = 0; i < u8.length; i += 0x8000) s += String.fromCharCode.apply(null, u8.subarray(i, i + 0x8000));
    return { k: it.k, b64: btoa(s) };
  });
  window.__bufs = [];
  return out;
})()"#;

const STATE_JS: &str = r#"(() => {
  const v = document.querySelector("video") || {};
  let end = 0;
  try { for (let i = 0; i < v.buffered.length; i++) end = Math.max(end, v.buffered.end(i)); } catch (e) {}
  return { t: v.currentTime || 0, dur: v.duration || 0, bufEnd: end, paused: !!v.paused, vw: v.videoWidth || 0, vh: v.videoHeight || 0 };
})()"#;

const PLAY_JS: &str = r#"(() => {
  const v = document.querySelector("video");
  if (v && v.paused) { const p = v.play(); if (p && p.catch) p.catch(() => { v.muted = true; v.play(); }); }
  return true;
})()"#;

const SEEK_JS_PREFIX: &str = r#"((pos) => { const v = document.querySelector("video"); if (v) { try { v.currentTime = pos; } catch (e) {} } return true; })("#;

static CAPTURE_NAME_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^(video|audio|data)_(init|seg)_\d+\.(mp4|m4s)$").unwrap()
});

/// 通过播放页解密下载 720P 高清视频。
///
/// * `page_url`    视频页地址（`https://tv.cctv.com/....shtml`）
/// * `out_path`    输出 MP4 路径
/// * `max_seconds` 只下载前 N 秒（0 = 全部）
pub async fn download_hd(
    page_url: &str,
    out_path: &str,
    max_seconds: f64,
    progress: Option<ProgressFn>,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<String> {
    if page_url.trim().is_empty() {
        return Err(Error::cctv("缺少视频页地址，无法走高清通道"));
    }
    let is_cancelled = || {
        cancel
            .as_ref()
            .map(|flag| flag.load(Ordering::SeqCst))
            .unwrap_or(false)
    };

    let workdir = std::env::temp_dir().join(format!(
        "cctv_hd_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    tokio::fs::create_dir_all(&workdir)
        .await
        .map_err(|exc| Error::io(format!("创建捕获目录失败：{exc}")))?;

    let mut session = CdpSession::launch().await?;
    let result = capture_stream(&mut session, page_url, &workdir, max_seconds, &progress, &is_cancelled).await;
    session.shutdown().await;

    let merge_result = match result {
        Ok(()) => merge_fmp4(&workdir, out_path).await,
        Err(exc) => Err(exc),
    };
    let _ = tokio::fs::remove_dir_all(&workdir).await;
    merge_result.map(|_| out_path.to_string())
}

async fn capture_stream(
    session: &mut CdpSession,
    page_url: &str,
    workdir: &Path,
    max_seconds: f64,
    progress: &Option<ProgressFn>,
    is_cancelled: &(dyn Fn() -> bool + Send + Sync),
) -> Result<()> {
    session.call("Page.enable", serde_json::json!({})).await?;
    session.call("Runtime.enable", serde_json::json!({})).await?;
    session
        .call(
            "Page.addScriptToEvaluateOnNewDocument",
            serde_json::json!({ "source": HOOK_JS }),
        )
        .await?;

    session.navigate(page_url).await?;

    // 等 <video> 出现
    let mut found = false;
    for _ in 0..60 {
        if is_cancelled() {
            return Err(Error::Cancelled);
        }
        match session.evaluate("!!document.querySelector('video')").await {
            Ok(value) if value.as_bool() == Some(true) => {
                found = true;
                break;
            }
            _ => tokio::time::sleep(std::time::Duration::from_millis(500)).await,
        }
    }
    if !found {
        return Err(Error::cctv(
            "播放页未出现 video 元素（页面结构可能已变，或网络不通）",
        ));
    }

    tokio::time::sleep(std::time::Duration::from_millis(5000)).await;
    let _ = session.evaluate(PLAY_JS).await;
    tokio::time::sleep(std::time::Duration::from_millis(3000)).await;

    let initial = session.evaluate(STATE_JS).await?;
    let mut saved_total = 0usize;
    let mut stall = 0u32;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2 * 3600);

    loop {
        if is_cancelled() {
            return Err(Error::Cancelled);
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }

        tokio::time::sleep(std::time::Duration::from_millis(2000)).await;

        // 取走解密后的数据
        let drained = session.evaluate(DRAIN_JS).await?;
        let mut chunk_count = 0usize;
        if let Some(items) = drained.as_array() {
            for item in items {
                let kind = item.get("k").and_then(|v| v.as_str()).unwrap_or("data");
                let b64 = item.get("b64").and_then(|v| v.as_str()).unwrap_or("");
                if b64.is_empty() {
                    continue;
                }
                let bytes = match base64::Engine::decode(
                    &base64::engine::general_purpose::STANDARD,
                    b64,
                ) {
                    Ok(bytes) => bytes,
                    Err(_) => continue,
                };
                if bytes.len() < 32 {
                    continue;
                }
                saved_total += 1;
                chunk_count += 1;
                let head = &bytes[..bytes.len().min(64)];
                let is_init = head.windows(4).any(|window| window == b"ftyp");
                let name = format!(
                    "{kind}_{}_{:05}.{}",
                    if is_init { "init" } else { "seg" },
                    saved_total,
                    if is_init { "mp4" } else { "m4s" }
                );
                let path = workdir.join(name);
                tokio::fs::write(&path, &bytes)
                    .await
                    .map_err(|exc| Error::io(format!("写入捕获文件失败：{exc}")))?;
            }
        }

        let state = session.evaluate(STATE_JS).await?;
        let duration = state.get("dur").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let buf_end = state.get("bufEnd").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let paused = state.get("paused").and_then(|v| v.as_bool()).unwrap_or(false);
        let width = state.get("vw").and_then(|v| v.as_i64()).unwrap_or(0);
        let height = state.get("vh").and_then(|v| v.as_i64()).unwrap_or(0);

        let target = if max_seconds > 0.0 {
            if duration > 0.0 {
                max_seconds.min(duration)
            } else {
                max_seconds
            }
        } else {
            duration
        };
        let done = target > 0.0 && buf_end >= target - 1.5;

        if let Some(callback) = progress {
            callback(HdProgress {
                buf_end,
                target,
                saved: saved_total,
                resolution: format!("{width}x{height}"),
            });
        }

        if paused && !done {
            let _ = session.evaluate(PLAY_JS).await;
        }
        if !done && target > 0.0 && buf_end > 0.0 && buf_end < target - 1.0 {
            let position = (buf_end + 0.5).min(target);
            let _ = session
                .evaluate(&format!("{SEEK_JS_PREFIX}{position:.3})"))
                .await;
            stall = 0;
        }

        if done {
            break;
        }
        if chunk_count == 0 {
            stall += 1;
            if stall > 90 {
                return Err(Error::cctv("播放停滞超时（3 分钟无新数据）"));
            }
        } else {
            stall = 0;
        }
    }

    let _ = initial;
    Ok(())
}

// --------------------------------------------------------------------------- //
// fMP4 合并
// --------------------------------------------------------------------------- //
/// 遍历顶层 box，返回 `(type, offset, size)`。
fn parse_box_tree(data: &[u8], start: usize, end: usize) -> Vec<(String, usize, usize)> {
    let mut boxes = Vec::new();
    let end = if end == 0 { data.len() } else { end };
    let mut index = start;
    while index + 8 <= end {
        let mut size = u32::from_be_bytes([
            data[index],
            data[index + 1],
            data[index + 2],
            data[index + 3],
        ]) as usize;
        let kind = String::from_utf8_lossy(&data[index + 4..index + 8]).into_owned();
        if size == 0 {
            break;
        }
        if size == 1 {
            if index + 16 > data.len() {
                break;
            }
            let mut buffer = [0u8; 8];
            buffer.copy_from_slice(&data[index + 8..index + 16]);
            size = u64::from_be_bytes(buffer) as usize;
        }
        if size < 8 || index + size > data.len() {
            break;
        }
        boxes.push((kind, index, size));
        index += size;
    }
    boxes
}

fn u32_at(data: &[u8], offset: usize) -> u32 {
    if offset + 4 > data.len() {
        return 0;
    }
    u32::from_be_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]])
}

fn u64_at(data: &[u8], offset: usize) -> u64 {
    if offset + 8 > data.len() {
        return 0;
    }
    let mut buffer = [0u8; 8];
    buffer.copy_from_slice(&data[offset..offset + 8]);
    u64::from_be_bytes(buffer)
}

/// 返回 `(track_id, baseMediaDecodeTime)`。
fn segment_meta(data: &[u8]) -> (i64, u64) {
    let mut track: i64 = -1;
    let mut tfdt: u64 = 0;

    for (kind, offset, size) in parse_box_tree(data, 0, 0) {
        if kind != "moof" {
            continue;
        }
        for (kind2, offset2, size2) in parse_box_tree(data, offset + 8, offset + size) {
            if kind2 != "traf" {
                continue;
            }
            for (kind3, offset3, size3) in parse_box_tree(data, offset2 + 8, offset2 + size2) {
                if kind3 == "tfhd" && track == -1 {
                    track = u32_at(data, offset3 + 12) as i64;
                } else if kind3 == "tfdt" {
                    let version = data.get(offset3 + 8).copied().unwrap_or(0);
                    tfdt = if version == 1 {
                        u64_at(data, offset3 + 12)
                    } else {
                        u32_at(data, offset3 + 12) as u64
                    };
                }
                let _ = size3;
            }
        }
    }
    (track, tfdt)
}

/// 取 init segment 的编码参数签名（avcC / hvcC / esds 片段）。
fn codec_sig(data: &[u8]) -> Vec<u8> {
    for marker in [b"avcC".as_slice(), b"hvcC".as_slice()] {
        if let Some(index) = find_subslice(data, marker) {
            let end = (index + 24).min(data.len());
            return data[index..end].to_vec();
        }
    }
    match find_subslice(data, b"esds") {
        Some(index) => {
            let end = (index + 24).min(data.len());
            data[index..end].to_vec()
        }
        None => Vec::new(),
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

struct KindBucket {
    inits: Vec<Vec<u8>>,
    /// (tfdt, data)
    segments: Vec<(u64, Vec<u8>)>,
    /// 按文件出现顺序记录 (is_init, data)
    order: Vec<(bool, Vec<u8>)>,
}

/// 按 fMP4 内嵌时间戳（tfdt）排序去重合并，天然容忍重复 append 与同档重初始化。
///
/// 若检测到真正的多档混流（编码参数签名不同的多个 init），退化为按 init 边界分组
/// 并保留字节量最大的一组。
/// 合并捕获的 fMP4 分片为 MP4。
async fn merge_fmp4(cap_dir: &Path, out_mp4: &str) -> Result<()> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(cap_dir)
        .map_err(|exc| Error::io(format!("读取捕获目录失败：{exc}")))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .collect();
    entries.sort();

    let mut buckets: HashMap<String, KindBucket> = HashMap::new();
    for path in entries {
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };
        let caps = match CAPTURE_NAME_RE.captures(&name) {
            Some(caps) => caps,
            None => continue,
        };
        let kind = caps[1].to_string();
        let is_init = &caps[2] == "init";
        let data = match std::fs::read(&path) {
            Ok(data) => data,
            Err(_) => continue,
        };
        if data.len() < 32 {
            continue;
        }
        let bucket = buckets.entry(kind).or_insert_with(|| KindBucket {
            inits: Vec::new(),
            segments: Vec::new(),
            order: Vec::new(),
        });
        if is_init {
            bucket.inits.push(data.clone());
        } else {
            let (_, tfdt) = segment_meta(&data);
            bucket.segments.push((tfdt, data.clone()));
        }
        bucket.order.push((is_init, data));
    }

    let out_dir = Path::new(out_mp4).parent().map(|p| p.to_path_buf());
    if let Some(dir) = &out_dir {
        let _ = std::fs::create_dir_all(dir);
    }

    let mut inputs: Vec<String> = Vec::new();
    for kind in ["video", "audio", "data"] {
        let bucket = match buckets.get(kind) {
            Some(bucket) if !bucket.segments.is_empty() => bucket,
            _ => continue,
        };

        let mut init: Vec<u8> = bucket.inits.first().cloned().unwrap_or_default();
        let ordered: Vec<Vec<u8>>;

        if bucket.inits.len() > 1 {
            let signatures: std::collections::HashSet<Vec<u8>> =
                bucket.inits.iter().map(|data| codec_sig(data)).collect();
            if signatures.len() > 1 {
                // 真正的多档：按文件序以 init 为界分组，保留字节量最大的一组
                let mut groups: Vec<(Vec<u8>, Vec<Vec<u8>>)> = Vec::new();
                let mut current_init: Vec<u8> = Vec::new();
                let mut current_segments: Vec<Vec<u8>> = Vec::new();
                for (is_init, data) in &bucket.order {
                    if *is_init {
                        if !current_segments.is_empty() {
                            groups.push((current_init.clone(), current_segments.clone()));
                        }
                        current_init = data.clone();
                        current_segments = Vec::new();
                    } else {
                        current_segments.push(data.clone());
                    }
                }
                if !current_segments.is_empty() {
                    groups.push((current_init.clone(), current_segments.clone()));
                }
                if let Some((best_init, best_segments)) = groups
                    .into_iter()
                    .max_by_key(|(_, segments)| segments.iter().map(|d| d.len()).sum::<usize>())
                {
                    let mut with_meta: Vec<(u64, Vec<u8>)> = best_segments
                        .into_iter()
                        .map(|data| {
                            let (_, tfdt) = segment_meta(&data);
                            (tfdt, data)
                        })
                        .collect();
                    with_meta.sort_by_key(|(tfdt, _)| *tfdt);
                    init = best_init;
                    ordered = with_meta.into_iter().map(|(_, data)| data).collect();
                    write_mux(cap_dir, kind, &init, &ordered, &mut inputs)?;
                    continue;
                }
            }
            // 同参数多个 init：丢弃后续 init，只保留第一份
        }

        let mut seen: HashMap<u64, Vec<u8>> = HashMap::new();
        let mut sorted: Vec<(u64, Vec<u8>)> = bucket.segments.clone();
        sorted.sort_by(|a, b| (a.0, a.1.len()).cmp(&(b.0, b.1.len())));
        for (tfdt, data) in sorted {
            seen.entry(tfdt).or_insert(data);
        }
        let mut with_meta: Vec<(u64, Vec<u8>)> = seen.into_iter().collect();
        with_meta.sort_by_key(|(tfdt, _)| *tfdt);
        ordered = with_meta.into_iter().map(|(_, data)| data).collect();
        write_mux(cap_dir, kind, &init, &ordered, &mut inputs)?;
    }

    if inputs.is_empty() {
        return Err(Error::cctv("捕获目录中没有可合并的分片（可能未成功解密）"));
    }

    let ffmpeg_path = ffmpeg::find_ffmpeg().ok_or_else(|| Error::cctv("未找到 ffmpeg，无法合并"))?;
    let runtime = tokio::runtime::Handle::current();
    runtime.block_on(ffmpeg::concat_copy(&ffmpeg_path, &inputs, out_mp4))?;

    for path in &inputs {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

fn write_mux(
    cap_dir: &Path,
    kind: &str,
    init: &[u8],
    ordered: &[Vec<u8>],
    inputs: &mut Vec<String>,
) -> Result<()> {
    if ordered.is_empty() {
        return Ok(());
    }
    let target = cap_dir.join(format!("merged_{kind}.mp4"));
    let mut buffer: Vec<u8> = Vec::new();
    if !init.is_empty() {
        buffer.extend_from_slice(init);
    }
    for data in ordered {
        buffer.extend_from_slice(data);
    }
    std::fs::write(&target, &buffer)
        .map_err(|exc| Error::io(format!("写入中间文件失败：{exc}")))?;
    inputs.push(target.to_string_lossy().into_owned());
    Ok(())
}
