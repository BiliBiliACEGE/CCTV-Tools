//! HLS 下载引擎：并发分片下载 → 合并 → （可选）ffmpeg 无损转封装。
//!
//! 清晰度选择上的硬规则：**默认只从明文档位里挑最高档**。enc/h5e 通道是央视
//! 私有加密（CDRM）流，标准通道直接下载会得到花屏文件，必须走浏览器解密通道
//! （见 [`crate::hd`]）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

use crate::cctv::video;
use crate::error::{Error, Result};
use crate::ffmpeg;
use crate::http;
use crate::model::Episode;
use crate::util::{guess_episode_number, sanitize_filename, url_ext};

/// 进度回调：`(已完成分片, 总分片, 文本)`。
pub type ProgressFn = Arc<dyn Fn(usize, usize, String) + Send + Sync>;

/// 一次下载的结果。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadResult {
    pub success: bool,
    pub output_path: String,
    pub merged_file: String,
    pub segment_count: usize,
    pub bytes_written: u64,
    pub elapsed: f64,
    pub message: String,
    /// 实际使用的清晰度展示名
    pub quality_label: String,
}

impl DownloadResult {
    fn fail(message: impl Into<String>) -> Self {
        DownloadResult {
            success: false,
            message: message.into(),
            ..Default::default()
        }
    }
}

fn cancelled(cancel: &Option<Arc<AtomicBool>>) -> bool {
    cancel
        .as_ref()
        .map(|flag| flag.load(Ordering::SeqCst))
        .unwrap_or(false)
}

fn report(progress: &Option<ProgressFn>, done: usize, total: usize, text: &str) {
    if let Some(callback) = progress {
        callback(done, total, text.to_string());
    }
}

/// 下载单个分片（带重试）。
async fn fetch_segment(url: &str, path: &Path, cancel: &Option<Arc<AtomicBool>>) -> Result<u64> {
    let mut last: Option<Error> = None;
    for attempt in 0..4u32 {
        if cancelled(cancel) {
            return Err(Error::Cancelled);
        }
        match http::get_bytes_full(url, http::REFERER, 40, 1).await {
            Ok(data) if !data.is_empty() => {
                tokio::fs::write(path, &data)
                    .await
                    .map_err(|exc| Error::io(exc.to_string()))?;
                return Ok(data.len() as u64);
            }
            Ok(_) => last = Some(Error::cctv("空分片")),
            Err(exc) => last = Some(exc),
        }
        tokio::time::sleep(Duration::from_millis(600 * (attempt as u64 + 1))).await;
    }
    Err(last.unwrap_or_else(|| Error::cctv("分片下载失败")))
}

/// 下载一个 HLS 流并输出为文件。
///
/// `output_path` 建议以 `.mp4` 结尾；本机没有 ffmpeg 时自动退化为 `.ts`。
/// `merged_ext` 可指定合并中间文件的扩展名（音频流的分片是 `.mp3`，需要保留）。
#[allow(clippy::too_many_arguments)]
pub async fn download_hls(
    media_m3u8_url: &str,
    output_path: &str,
    workers: usize,
    on_progress: Option<ProgressFn>,
    cancel: Option<Arc<AtomicBool>>,
    make_mp4: bool,
    keep_ts: bool,
    merged_ext: &str,
) -> DownloadResult {
    let started = Instant::now();
    report(&on_progress, 0, 0, "解析播放列表…");

    let segments = match video::get_segments(media_m3u8_url).await {
        Ok(segments) => segments,
        Err(exc) => return DownloadResult::fail(format!("解析分片失败：{exc}")),
    };
    if segments.is_empty() {
        return DownloadResult::fail("播放列表中没有分片");
    }

    let total = segments.len();
    let tmp_dir = std::env::temp_dir().join(format!(
        "cctvdl_{}_{}",
        std::process::id(),
        started.elapsed().as_nanos()
    ));
    if let Err(exc) = tokio::fs::create_dir_all(&tmp_dir).await {
        return DownloadResult::fail(format!("创建临时目录失败：{exc}"));
    }

    let counter = Arc::new(AtomicU64::new(0));
    let bytes = Arc::new(AtomicU64::new(0));

    let fetch_list: Vec<(usize, String)> = segments.iter().cloned().enumerate().collect();
    let tmp_for_fetch = tmp_dir.clone();
    let counter_for_fetch = Arc::clone(&counter);
    let bytes_for_fetch = Arc::clone(&bytes);
    let progress_for_fetch = on_progress.clone();
    let cancel_for_fetch = cancel.clone();

    let stream = futures_util::stream::iter(fetch_list.into_iter().map(move |(index, url)| {
        let path = tmp_for_fetch.join(format!("{index:06}.part"));
        let counter = Arc::clone(&counter_for_fetch);
        let bytes = Arc::clone(&bytes_for_fetch);
        let progress = progress_for_fetch.clone();
        let cancel = cancel_for_fetch.clone();
        async move {
            if cancelled(&cancel) {
                return Err(Error::Cancelled);
            }
            let written = fetch_segment(&url, &path, &cancel).await?;
            bytes.fetch_add(written, Ordering::SeqCst);
            let done = counter.fetch_add(1, Ordering::SeqCst) as usize + 1;
            if let Some(callback) = &progress {
                callback(done, total, format!("已下载 {done}/{total} 个分片"));
            }
            Ok::<usize, Error>(index)
        }
    }))
    .buffered(workers.max(1));

    let mut parts: Vec<Option<PathBuf>> = vec![None; total];
    let mut failure: Option<Error> = None;
    futures_util::pin_mut!(stream);
    while let Some(item) = stream.next().await {
        match item {
            Ok(index) => parts[index] = Some(tmp_dir.join(format!("{index:06}.part"))),
            Err(exc) => {
                failure = Some(exc);
                break;
            }
        }
    }

    if let Some(exc) = failure {
        let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
        return match exc {
            Error::Cancelled => DownloadResult::fail("已取消"),
            other => DownloadResult::fail(format!("下载失败：{other}")),
        };
    }

    // 合并
    report(&on_progress, total, total, "合并分片…");
    let (base, ext) = split_extension(output_path);
    let ts_path = if !merged_ext.is_empty() {
        format!("{base}{merged_ext}")
    } else if ext.eq_ignore_ascii_case("ts") {
        output_path.to_string()
    } else {
        format!("{base}.ts")
    };

    let mut merged = Vec::with_capacity(total);
    for path in parts.iter() {
        match path {
            Some(path) => merged.push(path.clone()),
            None => {
                let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
                return DownloadResult::fail("合并失败：分片缺失");
            }
        }
    }
    if let Err(exc) = concat_files(&merged, &ts_path).await {
        let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
        return DownloadResult::fail(format!("合并失败：{exc}"));
    }
    let _ = tokio::fs::remove_dir_all(&tmp_dir).await;

    let mut final_path = ts_path.clone();
    if make_mp4 && !ext.eq_ignore_ascii_case("ts") {
        if let Some(ffmpeg_path) = ffmpeg::find_ffmpeg() {
            let target = if ext.is_empty() {
                format!("{base}.mp4")
            } else {
                output_path.to_string()
            };
            let label = if ext.is_empty() {
                "MP4".to_string()
            } else {
                ext.trim_start_matches('.').to_uppercase()
            };
            report(&on_progress, total, total, &format!("转封装为 {label}…"));
            if ffmpeg::remux_to_mp4(&ffmpeg_path, &ts_path, &target).await {
                final_path = target;
                if !keep_ts {
                    let _ = tokio::fs::remove_file(&ts_path).await;
                }
            } else {
                report(&on_progress, total, total, "ffmpeg 转封装失败，保留合并文件");
            }
        }
    }

    DownloadResult {
        success: true,
        output_path: final_path,
        merged_file: ts_path,
        segment_count: total,
        bytes_written: bytes.load(Ordering::SeqCst),
        elapsed: started.elapsed().as_secs_f64(),
        message: "完成".to_string(),
        quality_label: String::new(),
    }
}

fn split_extension(path: &str) -> (String, String) {
    match path.rfind('.') {
        Some(index) if index > path.rfind(['/', '\\']).map(|p| p + 1).unwrap_or(0) => (
            path[..index].to_string(),
            path[index..].to_string(),
        ),
        _ => (path.to_string(), String::new()),
    }
}

async fn concat_files(parts: &[PathBuf], out_path: &str) -> Result<()> {
    if let Some(parent) = Path::new(out_path).parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|exc| Error::io(exc.to_string()))?;
    }
    let mut buffer: Vec<u8> = Vec::new();
    for path in parts {
        let data = tokio::fs::read(path)
            .await
            .map_err(|exc| Error::io(format!("读取分片失败：{exc}")))?;
        buffer.extend_from_slice(&data);
    }
    tokio::fs::write(out_path, &buffer)
        .await
        .map_err(|exc| Error::io(format!("写入失败：{exc}")))?;
    Ok(())
}

/// 按 guid 下载单个视频（自动选清晰度、自动命名）。
///
/// `br = None` 时只从「明文」清晰度里选最高档。
pub async fn download_video(
    guid: &str,
    output_dir: &str,
    filename: &str,
    br: Option<i64>,
    workers: usize,
    on_progress: Option<ProgressFn>,
    cancel: Option<Arc<AtomicBool>>,
    make_mp4: bool,
    keep_ts: bool,
) -> DownloadResult {
    if cancelled(&cancel) {
        return DownloadResult::fail("已取消");
    }
    let info = match video::get_video_info(guid).await {
        Ok(info) => info,
        Err(exc) => return DownloadResult::fail(format!("获取视频信息失败：{exc}")),
    };
    let qualities = match video::list_qualities(info.clone().into(), false).await {
        Ok(qualities) => qualities,
        Err(exc) => return DownloadResult::fail(exc.to_string()),
    };
    if qualities.is_empty() {
        return DownloadResult::fail("没有可用清晰度");
    }

    let plain: Vec<&crate::model::Quality> =
        qualities.iter().filter(|quality| !quality.encrypted).collect();
    let chosen = match br {
        None => match plain.first() {
            Some(quality) => (*quality).clone(),
            None => qualities[0].clone(),
        },
        Some(target) => match qualities.iter().find(|quality| quality.br == target) {
            Some(quality) if quality.encrypted => {
                return DownloadResult::fail(format!(
                    "{} 是加密流（enc/h5e），标准通道下载会花屏，请改用高清通道",
                    quality.label
                ));
            }
            Some(quality) => quality.clone(),
            None => qualities[0].clone(),
        },
    };

    if chosen.encrypted {
        return DownloadResult::fail(
            "该视频只有加密流（enc/h5e）可用，请使用高清通道下载",
        );
    }

    let name = if filename.is_empty() {
        if info.title.is_empty() {
            guid.to_string()
        } else {
            info.title.clone()
        }
    } else {
        filename.to_string()
    };
    let target_dir = PathBuf::from(output_dir);
    if let Err(exc) = tokio::fs::create_dir_all(&target_dir).await {
        return DownloadResult::fail(format!("创建输出目录失败：{exc}"));
    }
    let output_path = target_dir.join(format!("{}.mp4", sanitize_filename(&name, 120)));

    let mut result = download_hls(
        &chosen.url,
        &output_path.to_string_lossy(),
        workers,
        on_progress,
        cancel,
        make_mp4,
        keep_ts,
        "",
    )
    .await;
    if result.success {
        result.message = format!("{} · {} 分片", chosen.label, result.segment_count);
        result.quality_label = chosen.label.clone();
    }
    result
}

/// 下载听音专区的音频节目（纯音频，无需解密）。
///
/// 分片是独立 `.mp3` 时直接拼接成 `.mp3`；若是 TS+AAC 音频则转封装成 `.m4a`。
pub async fn download_audio(
    guid: &str,
    output_dir: &str,
    filename: &str,
    workers: usize,
    on_progress: Option<ProgressFn>,
    cancel: Option<Arc<AtomicBool>>,
    keep_ts: bool,
) -> DownloadResult {
    let info = match video::get_video_info(guid).await {
        Ok(info) => info,
        Err(exc) => return DownloadResult::fail(format!("获取音频信息失败：{exc}")),
    };
    let stream = info.audio_stream();
    if stream.is_empty() {
        return DownloadResult::fail("没有可用的音频流");
    }

    // 听音的音频地址是带多档的主表，先落到具体档位
    let media_url = match video::pick_media_playlist(&stream).await {
        Ok(url) => url,
        Err(exc) => return DownloadResult::fail(format!("解析音频列表失败：{exc}")),
    };
    let segments = match video::get_segments(&media_url).await {
        Ok(segments) => segments,
        Err(exc) => return DownloadResult::fail(format!("解析音频列表失败：{exc}")),
    };
    if segments.is_empty() {
        return DownloadResult::fail("音频列表中没有分片");
    }

    let sample: Vec<String> = segments.iter().take(8).map(|url| url_ext(url)).collect();
    let all_mp3 = sample.iter().all(|ext| ext == "mp3");
    let has_aac = sample.iter().any(|ext| ext == "aac");
    let (media_ext, merged_ext, make_mp4) = if all_mp3 {
        (".mp3", ".mp3", false)
    } else if has_aac {
        (".m4a", ".aac", false)
    } else {
        (".m4a", ".ts", true)
    };

    let name = if filename.is_empty() {
        if info.title.is_empty() {
            guid.to_string()
        } else {
            info.title.clone()
        }
    } else {
        filename.to_string()
    };
    let target_dir = PathBuf::from(output_dir);
    if let Err(exc) = tokio::fs::create_dir_all(&target_dir).await {
        return DownloadResult::fail(format!("创建输出目录失败：{exc}"));
    }
    let target = target_dir.join(format!(
        "{}{}",
        sanitize_filename(&name, 120),
        media_ext
    ));

    let mut result = download_hls(
        &media_url,
        &target.to_string_lossy(),
        workers,
        on_progress,
        cancel,
        make_mp4,
        keep_ts,
        merged_ext,
    )
    .await;
    if result.success {
        result.message = format!("音频 · {} 分片", result.segment_count);
    }
    result
}

/// 下载专辑 / 栏目中的一集，输出到 `输出目录/专辑名/` 下。
///
/// 目前 GUI 直接走 [`download_video`]，这里保留完整的「自动集号命名」实现，
/// 方便 CLI / 后续复用。
#[allow(dead_code)]
#[allow(clippy::too_many_arguments)]
pub async fn download_episode(
    episode: &Episode,
    album_title: &str,
    output_dir: &str,
    br: Option<i64>,
    workers: usize,
    on_progress: Option<ProgressFn>,
    cancel: Option<Arc<AtomicBool>>,
    make_mp4: bool,
    keep_ts: bool,
) -> DownloadResult {
    let title = episode.title.trim().to_string();
    let number = if episode.part_fabricated {
        0
    } else if episode.part > 0 {
        episode.part
    } else {
        guess_episode_number(&title)
    };

    let filename = if !title.is_empty() && guess_episode_number(&title) > 0 {
        sanitize_filename(&title, 120)
    } else if number > 0 {
        format!("{} 第{:02}集", album_title, number)
    } else {
        let safe = sanitize_filename(&title, 120);
        if safe.is_empty() || safe == "video" {
            if album_title.is_empty() {
                "视频".to_string()
            } else {
                album_title.to_string()
            }
        } else {
            safe
        }
    };

    let target_dir = if album_title.is_empty() {
        PathBuf::from(output_dir)
    } else {
        PathBuf::from(output_dir).join(sanitize_filename(album_title, 120))
    };

    download_video(
        &episode.guid,
        &target_dir.to_string_lossy(),
        &filename,
        br,
        workers,
        on_progress,
        cancel,
        make_mp4,
        keep_ts,
    )
    .await
}
