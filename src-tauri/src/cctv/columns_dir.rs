//! 栏目大全与内置栏目清单。
//!
//! `columnSearch` 接口只返回 EPGM id 与 `column_website`，**没有往期用的 TOPC id**，
//! 所以命中内置清单时顺带带上 `topic_id`，否则调用 [`resolve_column_topic`] 解析。
//!
//! 「影视」分类在接口里叫 `电影电视剧`，这是个必须记住的坑。

use once_cell::sync::Lazy;
use regex::Regex;

use crate::error::{Error, Result};
use crate::http;
use crate::model::{Column, Paged};
use crate::util::{json_str, safe_int};

/// 内置栏目清单快照（346 个栏目，244 个带 TOPC id）。
static BUILTIN_JSON: &str = include_str!("../../data/columns.json");

static BUILTIN_COLUMNS: Lazy<Vec<Column>> = Lazy::new(|| {
    let data: serde_json::Value =
        serde_json::from_str(BUILTIN_JSON).unwrap_or(serde_json::Value::Null);
    let mut columns = Vec::new();
    if let Some(list) = data.get("columns").and_then(|v| v.as_array()) {
        for item in list {
            columns.push(Column {
                name: json_str(item, "name"),
                topic_id: json_str(item, "topic"),
                site: json_str(item, "site"),
                channel: json_str(item, "channel"),
                fc: json_str(item, "fc"),
                sc: json_str(item, "sc"),
            });
        }
    }
    columns
});

static KEY_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^https?://").unwrap());

/// 读取内置栏目清单（离线快照）。
pub fn load_builtin_columns() -> &'static [Column] {
    &BUILTIN_COLUMNS
}

/// 在内置清单里按关键字搜索栏目（栏目名 / 频道 / 分类）。
pub fn search_columns(keyword: &str, only_usable: bool) -> Vec<Column> {
    let kw = keyword.trim().to_lowercase();
    let mut result = Vec::new();
    for column in load_builtin_columns() {
        if only_usable && !column.usable() {
            continue;
        }
        if kw.is_empty() {
            result.push(column.clone());
            continue;
        }
        let haystack = format!("{} {} {} {}", column.name, column.channel, column.fc, column.sc)
            .to_lowercase();
        if haystack.contains(&kw) {
            result.push(column.clone());
        }
    }
    result
}

/// 栏目大全页「分类」筛选项。展示名 -> 接口值（**影视 在接口里叫 电影电视剧**）。
pub const COLUMN_CATEGORIES: &[(&str, &str)] = &[
    ("全部", ""),
    ("新闻", "新闻"),
    ("体育", "体育"),
    ("综艺", "综艺"),
    ("健康", "健康"),
    ("生活", "生活"),
    ("科教", "科教"),
    ("经济", "经济"),
    ("农业", "农业"),
    ("法治", "法治"),
    ("军事", "军事"),
    ("少儿", "少儿"),
    ("动画", "动画"),
    ("纪实", "纪实"),
    ("戏曲", "戏曲"),
    ("音乐", "音乐"),
    ("影视", "电影电视剧"),
];

/// 栏目大全页「播出频道」筛选项（抓自页面 `datacid`）。
pub const COLUMN_CHANNELS: &[(&str, &str)] = &[
    ("全部", ""),
    ("CCTV-1 综合", "EPGC1386744804340101"),
    ("CCTV-2 财经", "EPGC1386744804340102"),
    ("CCTV-3 综艺", "EPGC1386744804340103"),
    ("CCTV-4 中文国际", "EPGC1386744804340104"),
    ("CCTV-5 体育", "EPGC1386744804340107"),
    ("CCTV-5+ 体育赛事", "EPGC1468294755566101"),
    ("CCTV-6 电影", "EPGC1386744804340108"),
    ("CCTV-7 国防军事", "EPGC1386744804340109"),
    ("CCTV-8 电视剧", "EPGC1386744804340110"),
    ("CCTV-9 纪录", "EPGC1386744804340112"),
    ("CCTV-10 科教", "EPGC1386744804340113"),
    ("CCTV-11 戏曲", "EPGC1386744804340114"),
    ("CCTV-12 社会与法", "EPGC1386744804340115"),
    ("CCTV-13 新闻", "EPGC1386744804340116"),
    ("CCTV-14 少儿", "EPGC1386744804340117"),
    ("CCTV-15 音乐", "EPGC1386744804340118"),
    ("CCTV-16 奥林匹克", "EPGC1634630207058998"),
    ("CCTV-17 农业农村", "EPGC1563932742616872"),
];

/// 栏目匹配键：优先用栏目页地址（去协议、去末尾斜杠），否则退回名字。
fn column_key(site: &str, name: &str) -> String {
    let key = KEY_RE.replace(site.trim(), "").trim_end_matches('/').to_lowercase();
    if key.is_empty() {
        format!("name:{}", name.trim())
    } else {
        key
    }
}

/// 浏览栏目大全。
///
/// `category` 传 [`COLUMN_CATEGORIES`] 的键（如「新闻」），`channel` 传
/// [`COLUMN_CHANNELS`] 的键（如「CCTV-13 新闻」），都留空则列出全部栏目。
pub async fn browse_columns(
    category: &str,
    channel: &str,
    page: i64,
    page_size: i64,
) -> Result<Paged<Column>> {
    let fc = COLUMN_CATEGORIES
        .iter()
        .find(|(name, _)| *name == category)
        .map(|(_, value)| *value)
        .unwrap_or(category);
    let cid = COLUMN_CHANNELS
        .iter()
        .find(|(name, _)| *name == channel)
        .map(|(_, value)| *value)
        .unwrap_or(channel);

    let mut params: Vec<(&str, String)> = vec![
        ("serviceId", "tvcctv".to_string()),
        ("p", page.to_string()),
        ("n", page_size.to_string()),
    ];
    if !fc.is_empty() {
        params.push(("fc", fc.to_string()));
    }
    if !cid.is_empty() {
        params.push(("cid", cid.to_string()));
    }
    let borrowed: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let url = url::Url::parse_with_params("https://api.cntv.cn/lanmu/columnSearch", &borrowed)
        .map_err(|exc| Error::parse(exc.to_string()))?;

    let text = http::get_text_with(
        url.as_str(),
        "https://tv.cctv.com/lm/index.shtml",
        3,
    )
    .await?;
    let data: serde_json::Value = serde_json::from_str(&text)
        .map_err(|exc| Error::parse(format!("栏目大全返回格式异常：{exc}")))?;

    let response = data.get("response").cloned().unwrap_or(serde_json::Value::Null);
    let mut builtin: std::collections::HashMap<String, &Column> = std::collections::HashMap::new();
    for column in load_builtin_columns() {
        builtin.insert(column_key(&column.site, &column.name), column);
    }

    let mut columns = Vec::new();
    if let Some(docs) = response.get("docs").and_then(|v| v.as_array()) {
        for item in docs {
            let site = json_str(item, "column_website");
            let name = json_str(item, "column_name");
            let known = builtin.get(&column_key(&site, &name));
            let mut fc_value = json_str(item, "column_firstclass");
            if fc_value.is_empty() {
                fc_value = json_str(item, "column_type");
            }
            columns.push(Column {
                name,
                topic_id: known.map(|c| c.topic_id.clone()).unwrap_or_default(),
                site,
                channel: json_str(item, "channel_name"),
                fc: fc_value,
                sc: json_str(item, "column_secondclass"),
            });
        }
    }

    // 内置快照没命中的栏目补一次在线解析（栏目页 → TOPC id）。
    // 不做这一步的话，用户双击栏目大全里的栏目会因为 topic_id 为空而拉不到往期。
    let pending: Vec<(usize, String, String)> = columns
        .iter()
        .enumerate()
        .filter(|(_, column)| column.topic_id.is_empty() && !column.site.is_empty())
        .map(|(index, column)| (index, column.site.clone(), column.name.clone()))
        .collect();
    if !pending.is_empty() {
        let resolved = http::parallel_map(pending, 6, |(index, site, name)| async move {
            Ok((index, resolve_column_topic(&site, &name).await))
        })
        .await;
        for (index, topic_id) in resolved.into_iter().flatten() {
            if let Some(slot) = columns.get_mut(index) {
                if slot.topic_id.is_empty() {
                    slot.topic_id = topic_id;
                }
            }
        }
    }

    let total = safe_int(response.get("numFound").unwrap_or(&serde_json::Value::Null));
    Ok(Paged {
        total: if total > 0 { total } else { columns.len() as i64 },
        list: columns,
    })
}

/// 把栏目解析成 TOPC 往期 ID：先查内置清单，再回落到解析栏目页。
pub async fn resolve_column_topic(site: &str, name: &str) -> String {
    let key = column_key(site, name);
    let columns = load_builtin_columns();
    for column in columns {
        if column.usable() && column_key(&column.site, &column.name) == key {
            return column.topic_id.clone();
        }
    }
    if name.is_empty() == false && site.is_empty() {
        for column in columns {
            if column.usable() && column.name == name {
                return column.topic_id.clone();
            }
        }
    }
    if !site.is_empty() {
        if let Ok(page) = crate::cctv::column::parse_column_page(site).await {
            return page.column_id;
        }
    }
    String::new()
}

/// 由 TOPC id 反查内置清单里的栏目。
pub fn find_column_by_topic(topic_id: &str) -> Option<Column> {
    load_builtin_columns()
        .iter()
        .find(|column| !column.topic_id.is_empty() && column.topic_id == topic_id)
        .cloned()
}
