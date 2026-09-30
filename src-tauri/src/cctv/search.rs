//! 全站关键词搜索（search.cctv.com）。

use crate::error::Result;
use crate::http;
use crate::model::{Paged, SearchItem};
use crate::util::{fmt_seconds, json_str, safe_int, strip_tags};

const SEARCH_REFERER: &str = "https://search.cctv.com/";

/// 搜索全站视频。
pub async fn search_videos(
    keyword: &str,
    page: i64,
    page_size: i64,
    sort: &str,
    datepid: i64,
    channel: &str,
    vtime: &str,
) -> Result<Paged<SearchItem>> {
    let url = url::Url::parse_with_params(
        "https://search.cctv.com/ifsearch.php",
        &[
            ("qtext", keyword),
            ("page", &page.to_string()),
            ("sort", sort),
            ("pageSize", &page_size.to_string()),
            ("type", "video"),
            ("datepid", &datepid.to_string()),
            ("channel", channel),
            ("vtime", vtime),
        ],
    )
    .map_err(|exc| crate::error::Error::parse(exc.to_string()))?;

    let text = http::get_text_with(url.as_str(), SEARCH_REFERER, 3).await?;
    let data = crate::util::jsonp(&text)?;

    let mut items = Vec::new();
    if let Some(list) = data.get("list").and_then(|v| v.as_array()) {
        for item in list {
            let mut channel_name = json_str(item, "channel");
            if channel_name.is_empty() {
                channel_name = json_str(item, "TV");
            }
            items.push(SearchItem {
                id: json_str(item, "id"),
                title: strip_tags(&json_str(item, "title")),
                all_title: strip_tags(&json_str(item, "all_title")),
                url: json_str(item, "urllink"),
                image: json_str(item, "imglink"),
                duration: fmt_seconds(item.get("durations").unwrap_or(&serde_json::Value::Null)),
                channel: channel_name,
                upload_time: json_str(item, "uploadtime"),
            });
        }
    }

    Ok(Paged {
        total: safe_int(data.get("total").unwrap_or(&serde_json::Value::Null)),
        list: items,
    })
}
