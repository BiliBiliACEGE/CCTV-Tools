//! 片库分类浏览。

use crate::error::{Error, Result};
use crate::http;
use crate::model::{Album, Paged};
use crate::util::safe_int;

/// 浏览央视网片库专辑列表。
///
/// * `fc`     分类名：电视剧 / 动画片 / 纪录片 / 特别节目
/// * `sc`     类型（题材），如 都市、谍战、悬疑
/// * `area`   地区，如 内地（大陆）
/// * `year`   年份，如 2026
/// * `letter` 首字母
pub async fn list_albums(
    fc: &str,
    sc: &str,
    area: &str,
    year: &str,
    letter: &str,
    channelid: &str,
    page: i64,
    page_size: i64,
) -> Result<Paged<Album>> {
    let url = url::Url::parse_with_params(
        "https://api.cntv.cn/list/getVideoAlbumList",
        &[
            ("channelid", channelid),
            ("area", area),
            ("sc", sc),
            ("fc", fc),
            ("year", year),
            ("letter", letter),
            ("p", &page.to_string()),
            ("n", &page_size.to_string()),
            ("serviceId", "tvcctv"),
            ("topv", "1"),
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
        return Err(Error::cctv(format!("片库接口返回错误：{data}")));
    }

    let payload = data.get("data").cloned().unwrap_or(serde_json::Value::Null);
    let mut albums = Vec::new();
    if let Some(list) = payload.get("list").and_then(|v| v.as_array()) {
        for item in list {
            albums.push(Album::from_library_item(item));
        }
    }
    let total = safe_int(payload.get("total").unwrap_or(&serde_json::Value::Null));
    Ok(Paged {
        total: if total > 0 { total } else { albums.len() as i64 },
        list: albums,
    })
}
