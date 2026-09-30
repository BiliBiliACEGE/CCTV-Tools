//! 视频预览：把点播视频解析成「能直接播的流地址」，据此取帧 / 播放。
//!
//! 为什么预览只能覆盖明文档位：
//!
//! * **plain 明文通道**：分片是明文 TS，hls.js / ffmpeg 都能直接解码，
//!   所以「窗口内嵌播放」「抓帧缩略图」「外部播放器播放」都可用；
//! * **enc / h5e 加密档**：CDRM 私有加密，密钥只存在于官方网页播放器的 WASM
//!   解密器里，直接喂给播放器会花屏 —— 这类档位在预览里只做提示，
//!   引导用户「用浏览器打开视频页」或用「高清通道下载」。
//!
//! 实测央视 CDN 返回 `Access-Control-Allow-Origin: *`，所以内嵌播放由前端
//! hls.js 直连 CDN 即可，无需本地代理。

use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

use crate::cctv::video;
use crate::error::{Error, Result};
use crate::ffmpeg;
use crate::model::Quality;
use crate::util::sanitize_filename;

/// ffplay 的显示名（永远排第一）。
pub const FFPLAY_NAME: &str = "ffplay（随 ffmpeg 自带）";

/// 常见第三方播放器（有就列出来让用户挑）。
const PLAYER_CANDIDATES: &[(&str, &[&str])] = &[
    (
        "mpv",
        &["mpv.exe", r"C:\Program Files\mpv\mpv.exe", r"D:\Program Files\mpv\mpv.exe"],
    ),
    (
        "PotPlayer",
        &[
            r"C:\Program Files\DAUM\PotPlayer\PotPlayerMini64.exe",
            r"C:\Program Files (x86)\DAUM\PotPlayer\PotPlayerMini.exe",
            r"D:\Program Files\DAUM\PotPlayer\PotPlayerMini64.exe",
        ],
    ),
    (
        "VLC",
        &[
            r"C:\Program Files\VideoLAN\VLC\vlc.exe",
            r"C:\Program Files (x86)\VideoLAN\VLC\vlc.exe",
        ],
    ),
];

/// 播放器信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerInfo {
    pub name: String,
    pub path: String,
}

/// 一次预览的解析结果。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewTarget {
    pub guid: String,
    pub title: String,
    pub duration: String,
    pub channel: String,
    /// 官方视频页（加密档用浏览器打开）
    pub page_url: String,
    pub qualities: Vec<Quality>,
    /// 当前选中的档位
    pub quality: Option<Quality>,
    /// 已落到档位的媒体播放列表地址
    pub media_url: String,
    /// 只有加密档（明文档位一个都没有）
    pub encrypted_only: bool,
    pub audio: bool,
    /// 给界面显示的一句话摘要
    pub summary: String,
}

impl PreviewTarget {
    fn compute_summary(&mut self) {
        if self.encrypted_only {
            let best = self
                .qualities
                .iter()
                .find(|quality| quality.encrypted)
                .cloned();
            let detail = match best {
                Some(quality) if !quality.resolution().is_empty() => {
                    format!("最高 {}（{}）", quality.label, quality.resolution())
                }
                Some(quality) => format!("最高 {}", quality.label),
                None => "无可用档位".to_string(),
            };
            self.summary = format!("加密档 · {detail}");
            return;
        }
        match &self.quality {
            None => self.summary = "未解析到流".to_string(),
            Some(quality) => {
                let mut parts = vec![quality.label.clone()];
                if !quality.resolution().is_empty() {
                    parts.push(quality.resolution());
                }
                if quality.kbps > 0 {
                    parts.push(format!("{}kbps", quality.kbps));
                }
                parts.push(if self.audio { "音频流".to_string() } else { "明文流".to_string() });
                self.summary = parts.join(" · ");
            }
        }
    }
}

/// 解析预览目标。
///
/// 明文通道最高档通常是 450/850（4K 专区另有明文 1080P）；720P 及以上只存在于
/// enc/h5e 加密通道，此时 `encrypted_only` 为 true、`media_url` 为空，
/// 调用方应改用浏览器打开 `page_url` 或走高清通道下载。
pub async fn resolve(
    guid: &str,
    br: Option<i64>,
    audio: bool,
    known_page_url: &str,
) -> Result<PreviewTarget> {
    let info = video::get_video_info(guid).await?;
    let page_url = if !known_page_url.is_empty() {
        known_page_url.to_string()
    } else {
        info.page_url.clone()
    };

    let mut target = PreviewTarget {
        guid: guid.to_string(),
        title: if info.title.is_empty() {
            guid.to_string()
        } else {
            info.title.clone()
        },
        duration: info.duration.clone(),
        channel: info.channel.clone(),
        page_url,
        audio,
        ..Default::default()
    };

    if audio {
        let stream = info.audio_stream();
        if stream.is_empty() {
            return Err(Error::cctv("该节目没有可用的音频流"));
        }
        // hls_audio_url 是带多档的主表，必须先落到具体档位
        target.media_url = video::pick_media_playlist(&stream).await?;
        let quality = Quality {
            br: 0,
            url: stream,
            label: "音频流".to_string(),
            channel: "plain".to_string(),
            encrypted: false,
            ..Default::default()
        };
        target.qualities = vec![quality.clone()];
        target.quality = Some(quality);
        target.compute_summary();
        return Ok(target);
    }

    let qualities = video::list_qualities(info.into(), false).await?;
    target.qualities = qualities;
    let plain: Vec<Quality> = target
        .qualities
        .iter()
        .filter(|quality| !quality.encrypted)
        .cloned()
        .collect();
    if plain.is_empty() {
        target.encrypted_only = true;
        target.compute_summary();
        return Ok(target);
    }

    let chosen = match br {
        Some(target_br) => plain
            .iter()
            .find(|quality| quality.br == target_br)
            .cloned()
            .unwrap_or_else(|| plain[0].clone()),
        None => plain[0].clone(),
    };
    target.media_url = video::media_url_for(&chosen).await?;
    target.quality = Some(chosen);
    target.compute_summary();
    Ok(target)
}

/// 列出可用播放器，ffplay 在最前。
pub fn list_players() -> Vec<PlayerInfo> {
    let mut players = Vec::new();
    if let Some(path) = ffmpeg::find_ffplay() {
        players.push(PlayerInfo {
            name: FFPLAY_NAME.to_string(),
            path,
        });
    }
    for (name, candidates) in PLAYER_CANDIDATES {
        for candidate in candidates.iter() {
            let resolved = if candidate.contains('\\') || candidate.contains('/') {
                if std::path::Path::new(candidate).is_file() {
                    Some((*candidate).to_string())
                } else {
                    None
                }
            } else {
                ffmpeg::which(candidate)
            };
            if let Some(path) = resolved {
                players.push(PlayerInfo {
                    name: (*name).to_string(),
                    path,
                });
                break;
            }
        }
    }
    players
}

/// 当前正在播放的进程（用于「停止」）。
static PLAYER_PROCESS: Lazy<Mutex<Option<Child>>> = Lazy::new(|| Mutex::new(None));

/// 用外部播放器打开流地址（不阻塞），返回进程号。
///
/// ffplay / mpv / VLC 支持自定义 Referer + UA；PotPlayer 不支持，只能给裸 URL
/// （实测央视 CDN 不带 Referer 也放行）。
pub fn play(url: &str, title: &str, player_path: &str) -> Result<u32> {
    if url.is_empty() {
        return Err(Error::cctv("没有可用的流地址"));
    }
    let exe = if player_path.is_empty() {
        ffmpeg::find_ffplay().ok_or_else(|| {
            Error::cctv("没有找到可用的播放器（ffplay / mpv / PotPlayer / VLC 都没有）")
        })?
    } else {
        player_path.to_string()
    };
    let name = std::path::Path::new(&exe)
        .file_name()
        .map(|value| value.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let title = if title.is_empty() { "视频预览" } else { title };

    let mut command = Command::new(&exe);
    if name.starts_with("ffplay") {
        command.args([
            "-loglevel",
            "warning",
            "-window_title",
            title,
            "-headers",
            &format!("Referer: {}\r\nUser-Agent: {}\r\n", crate::http::REFERER, crate::http::UA),
            "-user_agent",
            crate::http::UA,
            "-i",
            url,
        ]);
    } else if name.starts_with("mpv") {
        command.args([
            url,
            &format!("--referrer={}", crate::http::REFERER),
            &format!("--user-agent={}", crate::http::UA),
            &format!("--force-media-title={title}"),
        ]);
    } else if name.starts_with("vlc") {
        command.args([
            url,
            &format!("--http-referrer={}", crate::http::REFERER),
            &format!("--http-user-agent={}", crate::http::UA),
        ]);
    } else {
        command.arg(url);
    }

    stop_player();
    let child = command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|exc| Error::io(format!("启动播放器失败：{exc}")))?;
    let pid = child.id();
    if let Ok(mut guard) = PLAYER_PROCESS.lock() {
        *guard = Some(child);
    }
    Ok(pid)
}

/// 结束上一个播放器进程。
pub fn stop_player() {
    if let Ok(mut guard) = PLAYER_PROCESS.lock() {
        if let Some(mut child) = guard.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// 用系统默认浏览器打开链接（加密档「能看清楚画面」的唯一办法）。
pub fn open_in_browser(url: &str) -> Result<()> {
    if url.is_empty() {
        return Err(Error::cctv("没有可打开的链接"));
    }
    #[cfg(target_os = "android")]
    {
        // 安卓没有 xdg-open 这类外部打开器；真实需求（复制链接 / 分享）由前端做，
        // 这里给出可读原因而不是让调用方看到一个 shell 报错。
        let _ = url;
        return Err(Error::cctv("安卓版请复制链接后在浏览器打开"));
    }
    #[cfg(windows)]
    {
        Command::new("cmd")
            .args(["/C", "start", "", url])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|exc| Error::io(format!("打开浏览器失败：{exc}")))?;
        return Ok(());
    }
    #[cfg(not(any(windows, target_os = "android")))]
    {
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        Command::new(opener)
            .arg(url)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|exc| Error::io(format!("打开浏览器失败：{exc}")))?;
        Ok(())
    }
}

/// 预览图的临时文件路径。
pub fn temp_frame_path(guid: &str) -> std::path::PathBuf {
    let folder = std::env::temp_dir().join("cctv_preview");
    let _ = std::fs::create_dir_all(&folder);
    folder.join(format!("{}.jpg", sanitize_filename(guid, 60)))
}

/// 取帧并返回 `data:image/jpeg;base64,...`，前端可直接塞进 `<img>`。
pub async fn grab_frame_data_url(
    media_url: &str,
    guid: &str,
    at: f64,
    width: u32,
) -> Result<String> {
    let path = temp_frame_path(guid);
    let path_str = path.to_string_lossy().into_owned();
    ffmpeg::grab_frame(media_url, &path_str, at, width, 90).await?;
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|exc| Error::io(format!("读取预览图失败：{exc}")))?;
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(format!("data:image/jpeg;base64,{encoded}"))
}

/// 取帧并保存到指定路径（「另存图片」用）。
pub async fn save_frame(
    media_url: &str,
    out_path: &str,
    at: f64,
    width: u32,
) -> Result<String> {
    ffmpeg::grab_frame(media_url, out_path, at, width, 90).await?;
    Ok(out_path.to_string())
}
