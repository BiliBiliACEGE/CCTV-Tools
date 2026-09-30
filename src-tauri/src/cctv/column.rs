//! 栏目页与栏目全量往期。
//!
//! 两个必须记住的坑（详见 docs/CCTV片库接口分析.md 第八、九章）：
//!
//! 1. `getVideoListByColumn` **不指定 `d=` 时深翻页有硬上限**（约 30 页），
//!    且 `total` 在超过 1000 时被封顶为 1000 —— 这就是「只有最近一年」的真相。
//!    要拿全量必须按 `d=YYYY` / `d=YYYYMM` 分块请求。
//! 2. 栏目页内联脚本里栏目 ID 的变量名各页面不统一，`lmtopId` / `topicID` /
//!    `topicId` / `topId` 都要匹配。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use once_cell::sync::Lazy;
use regex::Regex;

use crate::error::{Error, Result};
use crate::http;
use crate::model::Episode;
use crate::util::{safe_int, strip_tags};

const COLUMN_PAGE_SIZE: i64 = 100;
/// 深翻页硬上限：`p` 再大就返回空。
const COLUMN_MAX_PAGE: i64 = 10;
/// 单个时间窗口的条数上限（`total` 到 1000 即表示被封顶，服务端 `n` 固定 100、`p` ≤ 10）。
const COLUMN_CEIL: i64 = 1000;

/// 条数探测的并发度：探测是廉价请求，可以比分片下载更激进。
fn count_workers(workers: usize) -> usize {
    (workers * 4).clamp(8, 32)
}

/// 时间窗口粒度，对应 `d=` 的三种长度。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Grain {
    Year,
    Month,
    Day,
}

impl Grain {
    fn finer(self) -> Option<Grain> {
        match self {
            Grain::Year => Some(Grain::Month),
            Grain::Month => Some(Grain::Day),
            Grain::Day => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Grain::Year => "年",
            Grain::Month => "月",
            Grain::Day => "日",
        }
    }
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

/// 把窗口拆成下一级子窗口（年 → 12 个月；月 → 该月每一天）。
fn sub_blocks(grain: Grain, key: &str) -> Vec<String> {
    match grain {
        Grain::Year => (1..=12).map(|month| format!("{key}{month:02}")).collect(),
        Grain::Month => {
            let year: i64 = key.get(..4).and_then(|v| v.parse().ok()).unwrap_or(0);
            let month: i64 = key.get(4..6).and_then(|v| v.parse().ok()).unwrap_or(1);
            (1..=days_in_month(year, month))
                .map(|day| format!("{key}{day:02}"))
                .collect()
        }
        Grain::Day => Vec::new(),
    }
}

/// 细化结果：终端时间块（`日期 → 条数`）与仍然封顶的块数。
struct Expansion {
    terminal: Vec<(String, i64)>,
    truncated: usize,
}

/// 迭代细化时间窗口。
///
/// 服务端每个日期窗口（`d=YYYY` / `YYYYMM` / `YYYYMMDD`）最多只返回 1000 条，
/// 因此「年」窗口一旦封顶就必须拆成 12 个月，「月」窗口再封顶就拆成每一天，
/// 只有拆到真实条数 < 1000 的窗口才是完整的，随后按页拉全即可。
async fn expand_blocks(
    column_id: &str,
    years: &[i64],
    workers: usize,
    progress: Option<&ProgressFn>,
    cancel: Option<&Arc<AtomicBool>>,
) -> Expansion {
    let is_cancelled = || cancel.map(|flag| flag.load(Ordering::SeqCst)).unwrap_or(false);
    let mut level: Vec<(Grain, String)> = years
        .iter()
        .map(|year| (Grain::Year, year.to_string()))
        .collect();
    let mut terminal: Vec<(String, i64)> = Vec::new();
    let mut truncated = 0usize;

    while !level.is_empty() {
        if is_cancelled() {
            break;
        }
        let column = column_id.to_string();
        let batch = level.clone();
        let totals = http::parallel_map(batch.clone(), count_workers(workers), move |(_, key)| {
            let column = column.clone();
            async move { Ok(column_block_total(&column, &key).await) }
        })
        .await;

        let mut next: Vec<(Grain, String)> = Vec::new();
        for ((grain, key), total) in batch.into_iter().zip(totals.into_iter()) {
            let total = total.unwrap_or(0);
            if total <= 0 {
                continue;
            }
            if total < COLUMN_CEIL {
                terminal.push((key, total));
                continue;
            }
            match grain.finer() {
                Some(finer) => next.extend(sub_blocks(grain, &key).into_iter().map(|sub| (finer, sub))),
                None => {
                    // 单日都超过 1000 条，已经没有更细的窗口可用
                    truncated += 1;
                    terminal.push((key, total));
                }
            }
        }

        if let Some(callback) = progress {
            if !next.is_empty() {
                let grain = next[0].0;
                callback(
                    terminal.len(),
                    terminal.len() + next.len(),
                    format!(
                        "正在细化：已定位 {} 个完整时间块，下一层 {} 个{}窗口",
                        terminal.len(),
                        next.len(),
                        grain.label()
                    ),
                );
            }
        }
        level = next;
    }

    Expansion { terminal, truncated }
}

static COLUMN_ID_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"(?:lmtopId|topicID|topicId|topId|columnTopicId)\s*[=:]\s*['"](TOPC[A-Za-z0-9]+)['"]"#,
    )
    .unwrap()
});
static OG_TITLE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"<meta property="og:title" content="([^"]+)""#).unwrap());
static TITLE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"<title>([^<]+)</title>").unwrap());

/// 时间块条数缓存：`(column_id, date) -> total`。
///
/// 同一个年份会被「定位最新年」「二分最早年」「组装时间块」反复问到，
/// 缓存后能把请求数直接砍掉一半以上。
static TOTAL_CACHE: Lazy<Mutex<HashMap<(String, String), i64>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// 栏目页解析结果。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnPage {
    pub column_id: String,
    pub title: String,
    pub url: String,
}

/// 栏目全量往期的拉取结果。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnAllResult {
    pub total: i64,
    pub list: Vec<Episode>,
    pub years: (Option<i64>, Option<i64>),
    pub blocks: usize,
    /// 仍然被封顶的时间块数（单日超过 1000 条时才会出现）
    pub truncated: usize,
}

/// 进度回调签名：`(已完成块, 总块数, 文本)`。
pub type ProgressFn = Arc<dyn Fn(usize, usize, String) + Send + Sync>;

/// 从栏目页 HTML 中解析栏目 ID 与栏目名。
pub async fn parse_column_page(page_url: &str) -> Result<ColumnPage> {
    let html = http::get_text(page_url).await?;
    let caps = COLUMN_ID_RE.captures(&html).ok_or_else(|| {
        Error::cctv(format!("不是栏目页（未找到栏目 ID）：{page_url}"))
    })?;
    let column_id = caps[1].to_string();

    let mut title = String::new();
    if let Some(found) = OG_TITLE_RE.captures(&html).or_else(|| TITLE_RE.captures(&html)) {
        let raw = strip_tags(&found[1]);
        title = raw.split('_').next().unwrap_or("").trim().to_string();
    }
    if title.is_empty() {
        // 退而求其次：用内置清单按栏目页地址反查
        let target = page_url.trim_end_matches('/');
        for column in crate::cctv::columns_dir::load_builtin_columns() {
            if !column.site.is_empty() && column.site.trim_end_matches('/') == target {
                title = column.name.clone();
                break;
            }
        }
    }
    Ok(ColumnPage {
        column_id,
        title,
        url: page_url.to_string(),
    })
}

/// 栏目往期列表的单页请求（`date` 可为 `YYYY` 或 `YYYYMM`）。
async fn column_page(column_id: &str, page: i64, page_size: i64, date: &str) -> Result<(i64, Vec<Episode>)> {
    let mut params: Vec<(&str, String)> = vec![
        ("id", column_id.to_string()),
        ("n", page_size.to_string()),
        ("sort", "desc".to_string()),
        ("p", page.to_string()),
        ("serviceId", "tvcctv".to_string()),
        ("t", "jsonp".to_string()),
        ("cb", "cb".to_string()),
    ];
    if !date.is_empty() {
        params.push(("d", date.to_string()));
    }
    let borrowed: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let url = url::Url::parse_with_params("https://api.cntv.cn/NewVideo/getVideoListByColumn", &borrowed)
        .map_err(|exc| Error::parse(exc.to_string()))?;

    let text = http::get_text(url.as_str()).await?;
    let data = crate::util::jsonp(&text)?;
    let payload = data.get("data").cloned().unwrap_or(serde_json::Value::Null);

    let mut episodes = Vec::new();
    if let Some(list) = payload.get("list").and_then(|v| v.as_array()) {
        for item in list {
            episodes.push(Episode::from_column_item(item));
        }
    }
    let total = safe_int(payload.get("total").unwrap_or(&serde_json::Value::Null));
    Ok((
        if total > 0 { total } else { episodes.len() as i64 },
        episodes,
    ))
}

/// 取某个时间块（年或年月）的条数；失败返回 0。结果会缓存。
async fn column_block_total(column_id: &str, date: &str) -> i64 {
    let key = (column_id.to_string(), date.to_string());
    let cached = TOTAL_CACHE.lock().map(|guard| guard.get(&key).copied()).unwrap_or(None);
    if let Some(value) = cached {
        return value;
    }

    let total = match column_page(column_id, 1, 1, date).await {
        Ok((total, _)) => total,
        Err(_) => 0,
    };

    if let Ok(mut guard) = TOTAL_CACHE.lock() {
        if guard.len() > 4000 {
            guard.clear();
        }
        guard.insert(key, total);
    }
    total
}

/// 从 `start_year` 往回找第一个有往期的年份。
///
/// 年份按「每批 workers 个」并发探测，取批内最大的命中年份，
/// 避免逐年串行（老栏目最坏要串行探十几年）。
async fn latest_year(column_id: &str, start_year: i64, workers: usize) -> Option<i64> {
    let years: Vec<i64> = (1990..=start_year).rev().collect();
    for batch in years.chunks(workers.max(1)) {
        let column = column_id.to_string();
        let batch_vec = batch.to_vec();
        let results = http::parallel_map(batch_vec.clone(), batch_vec.len(), move |year| {
            let column = column.clone();
            async move { Ok(column_block_total(&column, &year.to_string()).await) }
        })
        .await;
        for (year, total) in batch_vec.iter().zip(results.iter()) {
            if total.unwrap_or(0) > 0 {
                return Some(*year);
            }
        }
    }
    None
}

/// 二分定位最早有往期的年份（央视网栏目数据最早到 2008 左右）。
async fn earliest_year(column_id: &str, latest: i64, floor: i64) -> i64 {
    let (mut lo, mut hi) = (floor, latest);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if column_block_total(column_id, &mid.to_string()).await > 0 {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    // 个别栏目中间停播过，二分结果可能偏晚，再往回并发多探几年
    let mut probe = lo - 1;
    while probe >= floor {
        let batch: Vec<i64> = (0..4)
            .map(|offset| probe - offset)
            .filter(|year| *year >= floor)
            .collect();
        if batch.is_empty() {
            break;
        }
        let column = column_id.to_string();
        let batch_vec = batch.clone();
        let results = http::parallel_map(batch_vec.clone(), batch_vec.len(), move |year| {
            let column = column.clone();
            async move { Ok(column_block_total(&column, &year.to_string()).await > 0) }
        })
        .await;
        let hit = batch_vec
            .iter()
            .zip(results.iter())
            .find(|(_, hit)| hit.unwrap_or(false))
            .map(|(year, _)| *year);
        match hit {
            Some(year) => {
                lo = year;
                probe = lo - 1;
            }
            None => break,
        }
    }
    lo
}

/// 拉完一个时间块的全部页。
///
/// 第 1 页先单独取（拿到 total），其余页按 total 估算页数并发取。
async fn pull_column_block(
    column_id: &str,
    date: &str,
    cancel: Option<&Arc<AtomicBool>>,
    total_hint: i64,
) -> Vec<Episode> {
    let is_cancelled = || cancel.map(|flag| flag.load(Ordering::SeqCst)).unwrap_or(false);
    if is_cancelled() {
        return Vec::new();
    }

    let first = match column_page(column_id, 1, COLUMN_PAGE_SIZE, date).await {
        Ok((_, list)) => list,
        Err(_) => return Vec::new(),
    };
    if first.is_empty() {
        return Vec::new();
    }
    let mut out = first.clone();
    if (first.len() as i64) < COLUMN_PAGE_SIZE {
        return out;
    }

    let mut total = total_hint;
    if total == 0 {
        total = column_page(column_id, 1, 1, date)
            .await
            .map(|(total, _)| total)
            .unwrap_or(0);
    }
    let pages_needed = if total > 0 && total < COLUMN_CEIL {
        COLUMN_MAX_PAGE.min((total + COLUMN_PAGE_SIZE - 1) / COLUMN_PAGE_SIZE)
    } else {
        COLUMN_MAX_PAGE
    };

    let rest: Vec<i64> = (2..=pages_needed.max(2)).collect();
    let column = column_id.to_string();
    let date_owned = date.to_string();
    let results = http::parallel_map(rest, 6, move |page| {
        let column = column.clone();
        let date = date_owned.clone();
        async move { column_page(&column, page, COLUMN_PAGE_SIZE, &date).await }
    })
    .await;

    for chunk in results.into_iter().flatten() {
        let (_, list) = chunk;
        if list.is_empty() {
            break;
        }
        let short = (list.len() as i64) < COLUMN_PAGE_SIZE;
        out.extend(list);
        if short {
            break;
        }
    }
    out
}

/// 拉取栏目的**全部往期**（可跨十余年）。
///
/// 策略：
/// 1. 先并发探年定位最新 / 最早年；
/// 2. 自适应细化时间窗口：年窗口封顶（1000 条）就拆成 12 个月，月窗口再封顶就拆成每一天，
///    直到每个窗口的真实条数都 < 1000——这样才不会被单窗口 1000 条的上限截断；
/// 3. 各终端窗口并发按页拉全，最后按 guid 去重、按播出时间倒序。
pub async fn list_column_all_videos(
    column_id: &str,
    since: &str,
    until: &str,
    workers: usize,
    progress: Option<ProgressFn>,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<ColumnAllResult> {
    use futures_util::StreamExt;

    let workers = workers.max(1);
    let now_year = current_year();
    let end_year = if until.trim().parse::<i64>().is_ok() {
        until.trim().parse::<i64>().unwrap()
    } else {
        now_year
    };

    let latest = match latest_year(column_id, end_year, workers).await {
        Some(year) => year,
        None => return Ok(ColumnAllResult::default()),
    };

    let start_year = if since.trim().parse::<i64>().is_ok() {
        let parsed = since.trim().parse::<i64>().unwrap().max(1990);
        parsed.min(latest)
    } else {
        earliest_year(column_id, latest, 1995).await
    };

    // 逐级细化时间窗口（命中缓存时几乎零成本）
    let year_list: Vec<i64> = (start_year..=end_year.min(latest)).collect();
    if let Some(callback) = &progress {
        callback(0, 1, format!("{start_year}-{latest}：正在划分时间块…"));
    }
    let expansion = expand_blocks(
        column_id,
        &year_list,
        workers,
        progress.as_ref(),
        cancel.as_ref(),
    )
    .await;
    let blocks: Vec<String> = expansion.terminal.iter().map(|(key, _)| key.clone()).collect();
    let block_hints: HashMap<String, i64> = expansion.terminal.iter().cloned().collect();

    let total_blocks = blocks.len();
    if let Some(callback) = &progress {
        callback(
            0,
            total_blocks,
            format!("{start_year}-{latest} 共 {total_blocks} 个时间块，开始拉取"),
        );
    }
    if total_blocks == 0 {
        return Ok(ColumnAllResult {
            years: (Some(start_year), Some(latest)),
            ..Default::default()
        });
    }

    // 各时间块并发拉取
    let column_for_blocks = column_id.to_string();
    let cancel_for_blocks = cancel.clone();
    let progress_for_blocks = progress.clone();
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let blocks_for_stream = blocks.clone();
    let hints = block_hints.clone();

    let stream = futures_util::stream::iter(blocks_for_stream.into_iter().map(move |block| {
        let column = column_for_blocks.clone();
        let cancel = cancel_for_blocks.clone();
        let progress = progress_for_blocks.clone();
        let counter = Arc::clone(&counter);
        let hint = hints.get(&block).copied().unwrap_or(0);
        async move {
            let result = pull_column_block(&column, &block, cancel.as_ref(), hint).await;
            let done = counter.fetch_add(1, Ordering::SeqCst) + 1;
            if let Some(callback) = &progress {
                callback(done, total_blocks, format!("已取 {block}"));
            }
            result
        }
    }))
    .buffered(workers);

    let mut collected: Vec<Episode> = Vec::new();
    let mut stream = Box::pin(stream);
    while let Some(chunk) = stream.next().await {
        collected.extend(chunk);
        if cancel
            .as_ref()
            .map(|flag| flag.load(Ordering::SeqCst))
            .unwrap_or(false)
        {
            break;
        }
    }

    let mut seen = std::collections::HashSet::new();
    let mut unique: Vec<Episode> = Vec::new();
    for episode in collected {
        if episode.guid.is_empty() || !seen.insert(episode.guid.clone()) {
            continue;
        }
        unique.push(episode);
    }
    unique.sort_by(|a, b| b.date.cmp(&a.date));

    Ok(ColumnAllResult {
        total: unique.len() as i64,
        list: unique,
        years: (Some(start_year), Some(latest)),
        blocks: total_blocks,
        truncated: expansion.truncated,
    })
}

fn current_year() -> i64 {
    // 不引入 chrono：用标准库把当前时间转成本地年份
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    1970 + now / 31_556_952
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_year_is_reasonable() {
        let year = current_year();
        assert!(year > 2020 && year < 2100, "unexpected year {year}");
    }

    #[test]
    fn test_days_in_month() {
        assert_eq!(days_in_month(2026, 1), 31);
        assert_eq!(days_in_month(2026, 4), 30);
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(days_in_month(1900, 2), 28);
    }

    /// 年窗口封顶后必须能拆到「日」，否则单窗口 1000 条的上限会截断往期。
    #[test]
    fn test_sub_blocks_refines_to_days() {
        let months = sub_blocks(Grain::Year, "2026");
        assert_eq!(months.len(), 12);
        assert_eq!(months[0], "202601");
        assert_eq!(months[11], "202612");

        let days = sub_blocks(Grain::Month, "202602");
        assert_eq!(days.len(), 28);
        assert_eq!(days[0], "20260201");
        assert_eq!(days[27], "20260228");

        assert_eq!(sub_blocks(Grain::Month, "202402").len(), 29);
        assert!(sub_blocks(Grain::Day, "20260201").is_empty());
        assert_eq!(Grain::Month.finer(), Some(Grain::Day));
        assert_eq!(Grain::Day.finer(), None);
    }
}
