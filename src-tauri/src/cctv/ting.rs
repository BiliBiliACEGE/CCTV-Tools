//! 听音专区（tv.cctv.com/ty/m/）—— 央视网音频节目浏览。
//!
//! 取数有两条路，自动选择：
//!
//! 1. 页面脚本里带 `var param = PAGE…` 的分区（历史 / 电视剧 / 全部 / 文化 /
//!    健康课堂 / 戏曲 / 听书社区）→ 走接口 `getVideoListByPageIdTvty`，直接返回 guid；
//! 2. 首页 / 热听榜是静态 HTML → 抓「标题 + 视频页地址」，再用
//!    `getVideoListByAlbumIdNew?serviceId=tvty` 把 VIDA id 换成 guid。
//!
//! **坑**：`getVideoListByPageIdTvty` 不接受 `p` 参数 —— 带 `&p=1` 会返回
//! `{"errcode":1001,"errmsg":"url error"}`（表现为「分区 0 条」），只能用 `n`
//! 控制条数，接口本身一次性给全量。

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::http;
use crate::model::Episode;
use crate::util::{json_str, strip_tags};

const TING_REFERER: &str = "https://tv.cctv.com/ty/m/index.shtml";

/// 听音分区：(展示名, 页面地址)。
pub const TING_SECTIONS: &[(&str, &str)] = &[
    ("首页", "https://tv.cctv.com/ty/m/index.shtml"),
    ("热听榜", "https://tv.cctv.com/ty/m/top/index.shtml"),
    ("历史", "https://tv.cctv.com/ty/m/whbk/index.shtml"),
    ("电视剧", "https://tv.cctv.com/ty/m/dianshiju/index.shtml"),
    ("全部", "https://tv.cctv.com/ty/m/sxy/index.shtml"),
    ("文化", "https://tv.cctv.com/ty/m/wenhua/index.shtml"),
    ("健康课堂", "https://tv.cctv.com/ty/m/jkkt/index.shtml"),
    ("戏曲", "https://tv.cctv.com/ty/m/xiqu/index.shtml"),
    ("听书社区", "https://tv.cctv.com/ty/m/txsq/index.shtml"),
];

/// 分区列表页里的 `var param = PAGE…`。
static PAGE_PARAM_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"var\s+param\s*=\s*['"]?(PAGE[A-Za-z0-9]{10,})"#).unwrap());

/// 视频页地址（听音条目指向普通视频页）。
static ANCHOR_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"(?i)<a\b[^>]*?href=["']((?:https?:)?//tv\.cctv\.com/\d{4}/\d{2}/\d{2}/(VID[A-Za-z0-9]+)\.shtml[^"']*)["'][^>]*>([\s\S]{0,900}?)</a>"#,
    )
    .unwrap()
});

static TITLE_AFTER_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)<p[^>]*class="[^"]*(?:tit|title|name)[^"]*"[^>]*>([\s\S]{0,120}?)</p>"#).unwrap()
});

static ATTR_TITLE_RES: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        Regex::new(r#"(?i)alt=["']([^"']{3,100})["']"#).unwrap(),
        Regex::new(r#"(?i)title=["']([^"']{3,100})["']"#).unwrap(),
        Regex::new(r#"(?i)<p[^>]*class="[^"]*(?:tit|title|name)[^"]*"[^>]*>([\s\S]{0,120}?)</p>"#)
            .unwrap(),
        Regex::new(r"(?i)<h\d[^>]*>([\s\S]{0,120}?)</h\d>").unwrap(),
    ]
});

static DURATION_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\d{1,2}:\d{2}(?::\d{2})?)").unwrap());
static SPACE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
static HEX32_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[0-9a-f]{32}$").unwrap());

fn clean(text: &str) -> String {
    SPACE_RE.replace_all(&strip_tags(text), " ").trim().to_string()
}

/// 听音分区列表返回体。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TingResult {
    pub section: String,
    pub page: i64,
    pub url: String,
    pub list: Vec<Episode>,
}

/// 把分区名或地址统一成页面地址。
pub fn section_url(section: &str) -> Result<String> {
    let name = section.trim();
    if name.starts_with("http") {
        return Ok(name.to_string());
    }
    TING_SECTIONS
        .iter()
        .find(|(display, _)| *display == name)
        .map(|(_, url)| (*url).to_string())
        .ok_or_else(|| Error::cctv(format!("未知的听音分区：{name}")))
}

/// 按字节起点取一段子串（自动对齐 UTF-8 字符边界），最多 `len` 个字符。
fn slice_from(text: &str, start: usize, len: usize) -> String {
    let mut start = start.min(text.len());
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    text[start..].chars().take(len).collect()
}

/// 从静态 HTML 里抓条目（首页 / 热听榜走这条路）。
fn parse_static_items(html: &str) -> Vec<Episode> {
    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for caps in ANCHOR_RE.captures_iter(html) {
        let anchor_end = caps.get(0).map(|m| m.end()).unwrap_or(0);
        let mut url = caps[1].to_string();
        let video_id = caps[2].to_string();
        let body = caps.get(3).map(|m| m.as_str()).unwrap_or("");
        if !seen.insert(video_id.clone()) {
            continue;
        }
        if url.starts_with("//") {
            url = format!("https:{url}");
        }

        let mut title = String::new();
        for pattern in ATTR_TITLE_RES.iter() {
            if let Some(found) = pattern.captures(body) {
                let candidate = clean(&found[1]);
                let width = candidate.chars().count();
                if (2..=100).contains(&width) {
                    title = candidate;
                    break;
                }
            }
        }
        if title.is_empty() {
            // <a> 里没有标题：往后看一小段找标题元素
            let tail = slice_from(html, anchor_end, 600);
            if let Some(found) = TITLE_AFTER_RE.captures(&tail) {
                let candidate = clean(&found[1]);
                let width = candidate.chars().count();
                if (2..=100).contains(&width) {
                    title = candidate;
                }
            }
        }
        if title.is_empty() {
            continue;
        }

        let tail = slice_from(html, anchor_end, 400);
        let duration = DURATION_RE
            .captures(body)
            .or_else(|| DURATION_RE.captures(&tail))
            .map(|found| found[1].to_string())
            .unwrap_or_default();

        items.push(Episode {
            title,
            video_id,
            guid: String::new(),
            url,
            length: duration,
            ..Default::default()
        });
    }
    items
}

fn parse_api_items(payload: &[serde_json::Value]) -> Vec<Episode> {
    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in payload {
        let guid = json_str(item, "guid").trim().to_string();
        if guid.is_empty() || !seen.insert(guid.clone()) {
            continue;
        }
        let brief = json_str(item, "brief");
        items.push(Episode {
            title: json_str(item, "title").trim().to_string(),
            video_id: json_str(item, "id").trim().to_string(),
            guid,
            url: json_str(item, "url").trim().to_string(),
            image: json_str(item, "image").trim().to_string(),
            brief: brief.trim().chars().take(120).collect(),
            sc: json_str(item, "album_title").trim().to_string(),
            ..Default::default()
        });
    }
    items
}

/// 拉取带 PAGE id 的分区列表。**注意不带 `p` 参数**（带了会 errcode 1001）。
async fn fetch_page_items(page_id: &str, page: i64, page_size: i64) -> Result<Vec<Episode>> {
    let want = (page_size.max(50) * page.max(1)).min(500);
    let url = url::Url::parse_with_params(
        "https://api.cntv.cn/newVideo/getVideoListByPageIdTvty",
        &[
            ("serviceId", "tvty"),
            ("id", page_id),
            ("n", &want.to_string()),
            ("t", "jsonp"),
            ("cb", "cb"),
        ],
    )
    .map_err(|exc| Error::parse(exc.to_string()))?;

    let raw = http::get_text_with(url.as_str(), TING_REFERER, 3).await?;
    let data = crate::util::jsonp(&raw)?;
    let payload: Vec<serde_json::Value> = data
        .get("data")
        .and_then(|d| d.get("list"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // 接口一次性给全量，本地按页切片
    let all = parse_api_items(&payload);
    let start = ((page.max(1) - 1) * page_size) as usize;
    Ok(all.into_iter().skip(start).take(page_size as usize).collect())
}

/// 列出听音某个分区的节目。
///
/// 返回的 `Episode.guid` 可能为空（首页 / 热听榜需要后续用 [`resolve_guid`] 解析）。
pub async fn list_ting_items(section: &str, page: i64, page_size: i64) -> Result<TingResult> {
    let url = section_url(section)?;
    let html = http::get_text_with(&url, TING_REFERER, 3).await?;

    let mut items = Vec::new();
    if let Some(found) = PAGE_PARAM_RE.captures(&html) {
        items = fetch_page_items(&found[1], page, page_size)
            .await
            .unwrap_or_default();
    }
    if items.is_empty() {
        items = parse_static_items(&html);
        if page > 1 {
            // 静态页只有一页内容
            items.clear();
        }
    }

    Ok(TingResult {
        section: section.to_string(),
        page,
        url,
        list: items,
    })
}

/// 把听音条目的视频页 ID（VIDA…）换成 32 位播放 guid。失败返回空串。
pub async fn resolve_guid(video_id: &str) -> String {
    let vid = video_id.trim();
    if vid.is_empty() {
        return String::new();
    }
    if HEX32_RE.is_match(vid) {
        return vid.to_string();
    }

    let url = match url::Url::parse_with_params(
        "https://api.cntv.cn/NewVideo/getVideoListByAlbumIdNew",
        &[
            ("id", vid),
            ("serviceId", "tvty"),
            ("pub", "2"),
            ("mode", "2"),
            ("p", "1"),
            ("n", "20"),
            ("sort", "asc"),
        ],
    ) {
        Ok(url) => url,
        Err(_) => return String::new(),
    };

    let raw = match http::get_text_with(url.as_str(), TING_REFERER, 2).await {
        Ok(text) => text,
        Err(_) => return String::new(),
    };
    let data = match crate::util::jsonp(&raw) {
        Ok(value) => value,
        Err(_) => return String::new(),
    };

    if let Some(list) = data
        .get("data")
        .and_then(|d| d.get("list"))
        .and_then(|v| v.as_array())
    {
        for item in list {
            let guid = json_str(item, "guid");
            if !guid.trim().is_empty() {
                return guid.trim().to_string();
            }
        }
    }
    String::new()
}

/// **并发**补齐一批条目的 guid，返回成功补齐的条数。
///
/// 首页 / 热听榜是静态页，只能拿到视频页 ID，每换一个 guid 就是一次请求；
/// 逐条串行的话 90 条要等 17 秒以上，这里统一并发处理（约 2 秒）。
pub async fn resolve_guids(episodes: &mut [Episode], workers: usize) -> usize {
    let pending: Vec<(usize, String)> = episodes
        .iter()
        .enumerate()
        .filter(|(_, ep)| ep.guid.is_empty())
        .map(|(index, ep)| {
            let key = if ep.video_id.is_empty() {
                ep.url.clone()
            } else {
                ep.video_id.clone()
            };
            (index, key)
        })
        .collect();
    if pending.is_empty() {
        return 0;
    }

    // parallel_map 保序：结果与 pending 一一对应
    let queries = pending.clone();
    let results = http::parallel_map(queries, workers, |(_, key)| async move {
        Ok(resolve_guid(&key).await)
    })
    .await;

    let mut filled = 0;
    for ((index, _), guid) in pending.into_iter().zip(results.into_iter()) {
        if let Some(guid) = guid {
            if !guid.is_empty() {
                if let Some(episode) = episodes.get_mut(index) {
                    episode.guid = guid;
                    filled += 1;
                }
            }
        }
    }
    filled
}
