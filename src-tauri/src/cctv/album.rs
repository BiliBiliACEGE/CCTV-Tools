//! 专辑剧集列表。

use crate::error::{Error, Result};
use crate::http;
use crate::model::Episode;

/// 查询专辑下的全部条目（含短看点），调用方用 `Episode::is_main` 过滤正片。
pub async fn list_album_episodes(album_id: &str, page_size: i64, page: i64) -> Result<Vec<Episode>> {
    let url = url::Url::parse_with_params(
        "https://api.cntv.cn/NewVideo/getVideoListByAlbumIdNew",
        &[
            ("id", album_id),
            ("serviceId", "tvcctv"),
            ("n", &page_size.to_string()),
            ("p", &page.to_string()),
            ("t", "jsonp"),
            ("cb", "cb"),
        ],
    )
    .map_err(|exc| Error::parse(exc.to_string()))?;

    let text = http::get_text(url.as_str()).await?;
    let data = crate::util::jsonp(&text)?;

    let errcode = data
        .get("errcode")
        .map(|v| v.to_string().trim_matches('"').to_string())
        .unwrap_or_default();
    if !errcode.is_empty() && errcode != "0" {
        let msg = crate::util::json_str(&data, "msg");
        return Err(Error::cctv(format!(
            "剧集接口返回错误：{}",
            if msg.is_empty() { data.to_string() } else { msg }
        )));
    }

    let mut episodes = Vec::new();
    if let Some(list) = data
        .get("data")
        .and_then(|d| d.get("list"))
        .and_then(|v| v.as_array())
    {
        for item in list {
            episodes.push(Episode::from_album_item(item));
        }
    }
    Ok(episodes)
}

/// 只取正片剧集，按 `part` 升序排列。
///
/// 第 1 页先取（多数专辑一页就够），确认还有后续页时再并发取剩余页。
/// 兜底：4K 专区等专辑的条目没有 `part` 字段，此时按接口返回顺序编号为 1..n。
pub async fn list_album_main_episodes(album_id: &str, max_pages: i64) -> Result<Vec<Episode>> {
    const PAGE_SIZE: i64 = 100;
    const WORKERS: usize = 6;

    let mut pages: Vec<Vec<Episode>> = Vec::new();
    let first = list_album_episodes(album_id, PAGE_SIZE, 1).await?;
    let first_len = first.len() as i64;
    pages.push(first);

    if first_len >= PAGE_SIZE && max_pages > 1 {
        let album = album_id.to_string();
        let rest: Vec<i64> = (2..=max_pages).collect();
        let results = http::parallel_map(rest, WORKERS, move |page| {
            let album = album.clone();
            async move { list_album_episodes(&album, PAGE_SIZE, page).await }
        })
        .await;
        for item in results {
            if let Some(batch) = item {
                pages.push(batch);
            }
        }
    }

    let mut mains: Vec<Episode> = Vec::new();
    let mut others: Vec<Episode> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for batch in &pages {
        if batch.is_empty() {
            break;
        }
        for ep in batch {
            if ep.guid.is_empty() || !seen.insert(ep.guid.clone()) {
                continue;
            }
            if ep.is_main() {
                mains.push(ep.clone());
            } else {
                others.push(ep.clone());
            }
        }
        if (batch.len() as i64) < PAGE_SIZE {
            break;
        }
    }

    if mains.is_empty() && !others.is_empty() {
        mains = others
            .into_iter()
            .enumerate()
            .map(|(index, mut ep)| {
                ep.part = index as i64 + 1;
                ep.part_fabricated = true;
                ep
            })
            .collect();
    }
    mains.sort_by_key(|ep| ep.part);
    Ok(mains)
}
