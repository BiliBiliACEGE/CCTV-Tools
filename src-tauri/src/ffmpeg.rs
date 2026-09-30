//! ffmpeg / ffplay 探测与调用。

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;

use crate::error::{Error, Result};

/// 常见安装位置（PATH 找不到时的兜底）。
const FFMPEG_CANDIDATES: &[&str] = &[
    r"D:\FFmpeg\bin\ffmpeg.exe",
    r"C:\ffmpeg\bin\ffmpeg.exe",
    r"C:\Program Files\ffmpeg\bin\ffmpeg.exe",
    r"D:\Program Files\ffmpeg\bin\ffmpeg.exe",
    r"~\ffmpeg\bin\ffmpeg.exe",
];

/// 在 PATH 里查找可执行文件。
pub fn which(exe: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(exe);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

/// 定位 ffmpeg，找不到返回 None（此时只输出 .ts）。
pub fn find_ffmpeg() -> Option<String> {
    if let Some(found) = which("ffmpeg.exe").or_else(|| which("ffmpeg")) {
        return Some(found);
    }
    for candidate in FFMPEG_CANDIDATES {
        let expanded = if let Some(rest) = candidate.strip_prefix("~\\") {
            match std::env::var_os("USERPROFILE") {
                Some(home) => PathBuf::from(home).join(rest.replace('\\', "/")),
                None => continue,
            }
        } else {
            PathBuf::from(candidate)
        };
        if expanded.is_file() {
            return Some(expanded.to_string_lossy().into_owned());
        }
    }
    None
}

/// 定位 ffplay，一般与 ffmpeg 同目录。
pub fn find_ffplay() -> Option<String> {
    if let Some(found) = which("ffplay.exe").or_else(|| which("ffplay")) {
        return Some(found);
    }
    let ffmpeg = find_ffmpeg()?;
    let dir = Path::new(&ffmpeg).parent()?;
    for name in ["ffplay.exe", "ffplay"] {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

/// 不重新编码，仅换封装为 MP4（AAC 需要 `aac_adtstoasc`）。
pub async fn remux_to_mp4(ffmpeg: &str, ts_path: &str, mp4_path: &str) -> bool {
    let _ = tokio::fs::remove_file(mp4_path).await;
    let output = Command::new(ffmpeg)
        .args([
            "-y",
            "-loglevel",
            "error",
            "-i",
            ts_path,
            "-c",
            "copy",
            "-bsf:a",
            "aac_adtstoasc",
            "-movflags",
            "+faststart",
            mp4_path,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await;

    match output {
        Ok(result) => {
            result.status.success()
                && tokio::fs::metadata(mp4_path)
                    .await
                    .map(|meta| meta.len() > 0)
                    .unwrap_or(false)
        }
        Err(_) => false,
    }
}

/// 取 `at` 秒处的一帧存成图片（jpg）。用于界面里的预览图。
pub async fn grab_frame(
    media_url: &str,
    out_path: &str,
    at: f64,
    width: u32,
    timeout_secs: u64,
) -> Result<()> {
    let ffmpeg = find_ffmpeg().ok_or_else(|| Error::cctv("未找到 ffmpeg，无法取帧"))?;
    if media_url.is_empty() {
        return Err(Error::cctv("没有可用的流地址"));
    }
    let _ = tokio::fs::remove_file(out_path).await;
    if let Some(parent) = Path::new(out_path).parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }

    let headers = format!("Referer: {}\r\nUser-Agent: {}\r\n", crate::http::REFERER, crate::http::UA);
    let future = Command::new(&ffmpeg)
        .args([
            "-y",
            "-loglevel",
            "error",
            "-nostdin",
            "-headers",
            &headers,
            "-user_agent",
            crate::http::UA,
            "-ss",
            &format!("{:.2}", at.max(0.0)),
            "-i",
            media_url,
            "-frames:v",
            "1",
            "-vf",
            &format!("scale={width}:-2"),
            "-q:v",
            "4",
            out_path,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();

    let output = match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), future).await
    {
        Ok(Ok(output)) => output,
        Ok(Err(exc)) => return Err(Error::io(format!("取帧失败：{exc}"))),
        Err(_) => return Err(Error::cctv(format!("取帧超时（{timeout_secs}s）"))),
    };

    let ok = tokio::fs::metadata(out_path)
        .await
        .map(|meta| meta.len() > 0)
        .unwrap_or(false);
    if ok {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr
        .lines()
        .filter(|line| !line.trim().is_empty())
        .next_back()
        .unwrap_or("");
    Err(Error::cctv(if detail.is_empty() {
        "取帧失败".to_string()
    } else {
        format!("取帧失败：{detail}")
    }))
}

/// 用多个输入文件无损合并为 MP4（`-c copy`，用于 fMP4 捕获产物）。
pub async fn concat_copy(ffmpeg: &str, inputs: &[String], out_path: &str) -> Result<()> {
    let mut command = Command::new(ffmpeg);
    command.args(["-v", "error", "-y"]);
    for input in inputs {
        command.args(["-i", input]);
    }
    command.args(["-c", "copy", "-movflags", "+faststart", out_path]);
    let output = command
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|exc| Error::io(format!("调用 ffmpeg 失败：{exc}")))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let head: String = stderr.chars().take(400).collect();
    Err(Error::cctv(format!("ffmpeg 合并失败：{head}")))
}
