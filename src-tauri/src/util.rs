//! 通用小工具：标签清理、jsonp 解析、秒数格式化、文件名清理、集号解析。

use once_cell::sync::Lazy;
use regex::Regex;

static TAG_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"<[^>]+>").unwrap());
static JSONP_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)^[^(]*\((.*)\)\s*;?\s*$").unwrap());
static ILLEGAL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"[\\/:*?"<>|\r\n\t]"#).unwrap());
static SPACE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
static EPISODE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"第\s*(\d+)\s*[集话期]").unwrap());

/// 去掉搜索结果里的 `<font color="red">` 高亮标签。
pub fn strip_tags(text: &str) -> String {
    TAG_RE.replace_all(text, "").trim().to_string()
}

/// 解析 jsonp 响应，兼容纯 JSON。
pub fn jsonp(text: &str) -> crate::error::Result<serde_json::Value> {
    let trimmed = text.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return Ok(serde_json::from_str(trimmed)?);
    }
    match JSONP_RE.captures(trimmed) {
        Some(caps) => Ok(serde_json::from_str(&caps[1])?),
        None => {
            let head: String = trimmed.chars().take(160).collect();
            Err(crate::error::Error::parse(format!("无法解析响应：{head}")))
        }
    }
}

/// 秒数 -> `H:MM:SS` / `MM:SS`；非正数返回空串。
pub fn fmt_seconds(value: &serde_json::Value) -> String {
    let seconds = match value {
        serde_json::Value::Number(n) => n.as_i64().unwrap_or(0),
        serde_json::Value::String(s) => s.parse::<i64>().unwrap_or(0),
        _ => 0,
    };
    if seconds <= 0 {
        return String::new();
    }
    let (h, rem) = (seconds / 3600, seconds % 3600);
    let (m, s) = (rem / 60, rem % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// 把 JSON 里的任意值当整数读（缺失 / 类型不符均为 0）。
pub fn safe_int(value: &serde_json::Value) -> i64 {
    match value {
        serde_json::Value::Number(n) => n.as_i64().unwrap_or(0),
        serde_json::Value::String(s) => s.trim().parse::<i64>().unwrap_or(0),
        _ => 0,
    }
}

/// 把 JSON 对象里的字段当字符串读（数字也转成字符串）。
pub fn json_str(value: &serde_json::Value, key: &str) -> String {
    match value.get(key) {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// 清理 Windows / Linux 下的非法文件名字符。
pub fn sanitize_filename(name: &str, max_length: usize) -> String {
    let cleaned = ILLEGAL_RE.replace_all(name.trim(), "_");
    let cleaned = SPACE_RE.replace_all(&cleaned, " ");
    let cleaned = cleaned.trim_end_matches(['.', ' ']).to_string();
    if cleaned.is_empty() {
        return "video".to_string();
    }
    cleaned.chars().take(max_length).collect()
}

/// 从标题里解析集号，如 `《能有多大事》 第12集` -> 12。
pub fn guess_episode_number(title: &str) -> i64 {
    EPISODE_RE
        .captures(title)
        .and_then(|caps| caps[1].parse::<i64>().ok())
        .unwrap_or(0)
}

/// 从 URL 里取扩展名（不含点），取不到返回空串。
pub fn url_ext(url: &str) -> String {
    let path = url.split('?').next().unwrap_or("").rsplit('/').next().unwrap_or("");
    match path.rsplit_once('.') {
        Some((_, ext)) => ext.to_lowercase(),
        None => String::new(),
    }
}

/// 取 URL 的 scheme://host 部分。
pub fn origin(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(parsed) => {
            let host = parsed.host_str().unwrap_or("");
            if host.is_empty() {
                String::new()
            } else {
                format!("{}://{}", parsed.scheme(), host)
            }
        }
        Err(_) => String::new(),
    }
}

/// 用给定 base 把相对地址补成绝对地址（HLS 播放列表里的分片 / 档位地址）。
pub fn absolutize(base_url: &str, reference: &str) -> String {
    if reference.starts_with("http://") || reference.starts_with("https://") {
        return reference.to_string();
    }
    if reference.starts_with("//") {
        let scheme = base_url.split("://").next().unwrap_or("https");
        return format!("{scheme}:{reference}");
    }
    if reference.starts_with('/') {
        return format!("{}{}", origin(base_url), reference);
    }
    let base = base_url.rsplit_once('/').map(|(b, _)| b).unwrap_or(base_url);
    format!("{base}/{reference}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fmt_seconds() {
        assert_eq!(fmt_seconds(&serde_json::json!(0)), "");
        assert_eq!(fmt_seconds(&serde_json::json!(75)), "01:15");
        assert_eq!(fmt_seconds(&serde_json::json!(3671)), "1:01:11");
        assert_eq!(fmt_seconds(&serde_json::json!("120")), "02:00");
    }

    #[test]
    fn test_sanitize() {
        assert_eq!(sanitize_filename("a/b:c*d?e", 120), "a_b_c_d_e");
        assert_eq!(sanitize_filename("   ", 120), "video");
    }

    #[test]
    fn test_absolutize() {
        let base = "https://x.com/a/b/main.m3u8";
        assert_eq!(absolutize(base, "seg.ts"), "https://x.com/a/b/seg.ts");
        assert_eq!(absolutize(base, "/z/seg.ts"), "https://x.com/z/seg.ts");
        assert_eq!(absolutize(base, "https://y.com/s.ts"), "https://y.com/s.ts");
        assert_eq!(absolutize(base, "//y.com/s.ts"), "https://y.com/s.ts");
    }
}
