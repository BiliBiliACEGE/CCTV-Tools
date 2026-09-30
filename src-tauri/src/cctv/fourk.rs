//! 4K 专区（tv.cctv.com/4K）。
//!
//! 该专区的视频是唯一存在**明文 1080P** 的地方（`/asp/hls/4000/`），
//! 详见 [`crate::cctv::video::list_qualities`]。

use crate::error::{Error, Result};
use crate::http;
use crate::model::{Album, Paged};
use crate::util::safe_int;

/// 4K 专区频道 ID（tv.cctv.com/4K 页面脚本中的固定值）。
pub const FOURK_CHANNEL_ID: &str = "CHAL1558416868484111";

/// 列出 4K 专区全部专辑（含每部最新一集，可直接取 guid）。
pub async fn list_4k_albums(page: i64, page_size: i64) -> Result<Paged<Album>> {
    let url = url::Url::parse_with_params(
        "https://api.cntv.cn/NewVideo/getLastVideoList4K",
        &[
            ("serviceId", "cctv4k"),
            ("cid", FOURK_CHANNEL_ID),
            ("p", &page.to_string()),
            ("n", &page_size.to_string()),
            ("t", "jsonp"),
            ("cb", "cb"),
        ],
    )
    .map_err(|exc| Error::parse(exc.to_string()))?;

    let text = http::get_text(url.as_str()).await?;
    let data = crate::util::jsonp(&text)?;
    let payload = data.get("data").cloned().unwrap_or(serde_json::Value::Null);

    let mut albums = Vec::new();
    if let Some(list) = payload.get("list").and_then(|v| v.as_array()) {
        for item in list {
            albums.push(Album::from_fourk_item(item));
        }
    }
    let total = safe_int(payload.get("total").unwrap_or(&serde_json::Value::Null));
    Ok(Paged {
        total: if total > 0 { total } else { albums.len() as i64 },
        list: albums,
    })
}
