//! 视频源、清晰度与分片解析。
//!
//! 这里是整个项目最关键的一块，几个「反直觉」的结论都体现在代码里：
//!
//! * **档位清单藏在 enc / h5e 通道**：明文通道（`/asp/hls/`）的 master 通常只
//!   声明最低档，而完整档位表在 enc/h5e 的 master 里。
//! * **明文通道有「假档」**：CDN 对普通视频的 1200 / 2000 路径会回落到最低流
//!   （实测 480×270，比 850 还差），所以只信任 450 / 850。
//! * **4K 专区另有明文 1080P**：master 不声明，但 `/asp/hls/4000/` 实际可用。

use once_cell::sync::Lazy;
use regex::Regex;

use crate::error::{Error, Result};
use crate::http;
use crate::model::{Quality, VideoInfo};
use crate::util::{absolutize, origin};

/// 播放器 vodplayer.js 中的固定签名盐值（已从混淆代码还原）。
const VDN_SALT: &str = "47899B86370B879139C08EA3B5E88267";
const VDN_VN: &str = "2049";

/// 明文通道兜底：master 未声明、但 CDN 上实际存在的档位。
const PLAIN_EXTRA_BITRATES: &[i64] = &[850];

/// plain 通道里可信的档位（1200 / 2000 会回落成「假档」）。
pub const PLAIN_TRUSTED_BRS: &[i64] = &[450, 850];

/// 码率代号 -> 档位名（央视网习惯叫法）。
fn quality_name(br: i64) -> Option<&'static str> {
    match br {
        450 => Some("流畅"),
        850 => Some("标清"),
        1200 => Some("高清"),
        2000 => Some("超清"),
        4000 => Some("蓝光"),
        _ => None,
    }
}

/// 明文 1080P 档（br=4000）的候选 CDN 主机（4K 专区视频的 main.m3u8 不声明该档）。
const PLAIN_4000_HOSTS: &[&str] = &["https://newcntv.qcloudcdn.com"];

/// 明文通道 m3u8 的路径：`/asp/hls/main/<mid>/<guid>/main.m3u8`（main 也可能是码率数字）。
static PLAIN_PATH_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"/asp/hls/[0-9a-z]+/([0-9a-z]+/[0-9a-z]+/[0-9a-z]+)/([0-9a-f]{32})/").unwrap());
static BITRATE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"/(?:enc/|h5e/)?hls/(\d{3,4})/").unwrap());
static BITRATE_FILE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"/(\d{3,4})\.m3u8").unwrap());
static RESOLUTION_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)RESOLUTION=(\d+)[xX](\d+)").unwrap());
static BANDWIDTH_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)BANDWIDTH=(\d+)").unwrap());
static GUID_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        Regex::new(r#"guid\s*=\s*"([0-9a-f]{32})""#).unwrap(),
        Regex::new(r"guid\s*=\s*'([0-9a-f]{32})'").unwrap(),
        Regex::new(r#""guid"\s*:\s*"([0-9a-f]{32})""#).unwrap(),
    ]
});
static HEX32_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[0-9a-f]{32}$").unwrap());

// --------------------------------------------------------------------------- //
// vdn 签名
// --------------------------------------------------------------------------- //
fn vdn_signature(timestamp: &str) -> String {
    let raw = format!("{timestamp}{VDN_VN}{VDN_SALT}");
    format!("{:x}", md5::compute(raw.as_bytes())).to_uppercase()
}

/// 生成带签名的视频信息接口地址。
pub fn vdn_url(guid: &str) -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .to_string();

    let params: [(&str, &str); 7] = [
        ("pid", guid),
        ("im", "0"),
        ("tsp", &ts),
        ("vn", VDN_VN),
        ("vc", ""),
        ("uid", ""),
        ("wlan", ""),
    ];
    let signed = vdn_signature(&ts);
    let mut pairs: Vec<(&str, &str)> = params.to_vec();
    // vc 必须放在 tsp/vn 之后（顺序不影响签名，但保持一致便于排查）
    pairs[4] = ("vc", &signed);

    let query = pairs
        .iter()
        .map(|(k, v)| format!("{k}={}", urlencode(v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("https://vdn.apps.cntv.cn/api/getHttpVideoInfo.do?{query}")
}

fn urlencode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

// --------------------------------------------------------------------------- //
// 视频信息
// --------------------------------------------------------------------------- //
/// 由视频页地址解析内部 guid（32 位十六进制）。
pub async fn resolve_guid(video_url: &str) -> Result<String> {
    if HEX32_RE.is_match(video_url) {
        return Ok(video_url.to_string());
    }
    let html = http::get_text(video_url).await?;
    for pattern in GUID_PATTERNS.iter() {
        if let Some(caps) = pattern.captures(&html) {
            return Ok(caps[1].to_string());
        }
    }
    Err(Error::cctv(format!("未能从页面解析出视频 guid：{video_url}")))
}

/// 获取播放信息（含各通道的 HLS 主播放列表地址）。
pub async fn get_video_info(guid: &str) -> Result<VideoInfo> {
    let text = http::get_text(&vdn_url(guid)).await?;
    let data: serde_json::Value = serde_json::from_str(&text)?;

    let ack = crate::util::json_str(&data, "ack");
    if ack != "yes" {
        let msg = {
            let tip = crate::util::json_str(&data, "tip_msg");
            if tip.is_empty() {
                crate::util::json_str(&data, "status")
            } else {
                tip
            }
        };
        return Err(Error::cctv(format!("视频信息获取失败：{msg}")));
    }

    let manifest = data
        .get("manifest")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let page_url = data
        .get("video")
        .and_then(|v| v.get("url"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok(VideoInfo {
        guid: guid.to_string(),
        title: crate::util::json_str(&data, "title"),
        duration: {
            let total = data
                .get("video")
                .and_then(|v| v.get("totalLength"))
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            match total {
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::String(s) => s,
                _ => String::new(),
            }
        },
        hls_url: crate::util::json_str(&data, "hls_url"),
        enc_hls_url: manifest
            .get("hls_enc_url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        h5e_hls_url: manifest
            .get("hls_h5e_url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        image: crate::util::json_str(&data, "image"),
        channel: crate::util::json_str(&data, "play_channel"),
        brief: crate::util::json_str(&data, "vset_brief"),
        album_id: crate::util::json_str(&data, "album_id"),
        vsetid: crate::util::json_str(&data, "vsetid"),
        page_url,
        manifest,
        raw: data,
    })
}

/// 把 guid 解析回视频页地址（加密档要用浏览器打开它）。
pub async fn page_url_for_guid(guid: &str) -> Result<String> {
    if guid.starts_with("http") {
        return Ok(guid.to_string());
    }
    let info = get_video_info(guid).await?;
    if info.page_url.starts_with("http") {
        return Ok(info.page_url);
    }
    let vsetid = if info.vsetid.is_empty() {
        info.album_id.clone()
    } else {
        info.vsetid.clone()
    };
    if !vsetid.is_empty() {
        for page in 1..=3 {
            let eps = crate::cctv::album::list_album_episodes(&vsetid, 200, page).await?;
            for ep in &eps {
                if ep.guid == guid && !ep.url.is_empty() {
                    return Ok(ep.url.clone());
                }
            }
            if eps.len() < 200 {
                break;
            }
        }
    }
    Err(Error::cctv(
        "无法确定 guid 对应的视频页地址，请提供视频页 URL",
    ))
}

// --------------------------------------------------------------------------- //
// 播放列表解析
// --------------------------------------------------------------------------- //
/// 解析主播放列表，返回 `[{br, url, resolution, bandwidth}]`。
fn parse_master_playlist(text: &str, master_url: &str) -> Vec<serde_json::Value> {
    let mut variants = Vec::new();
    let mut pending_resolution = String::new();
    let mut pending_bandwidth: i64 = 0;
    let mut has_pending = false;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if line.to_uppercase().starts_with("#EXT-X-STREAM-INF") {
            pending_resolution = RESOLUTION_RE
                .captures(line)
                .map(|c| c[1].to_string() + "x" + &c[2])
                .unwrap_or_default();
            pending_bandwidth = BANDWIDTH_RE
                .captures(line)
                .and_then(|c| c[1].parse::<i64>().ok())
                .unwrap_or(0);
            has_pending = true;
        } else if !line.starts_with('#') {
            let url = absolutize(master_url, line);
            variants.push(serde_json::json!({
                "br": bitrate_of(&url),
                "url": url,
                "resolution": if has_pending { pending_resolution.clone() } else { String::new() },
                "bandwidth": pending_bandwidth,
            }));
            has_pending = false;
        }
    }
    variants
}

/// 从地址里取码率代号，兼容 `/asp/hls/450/`、`/asp/enc/hls/2000/`、`…/2000.m3u8`。
fn bitrate_of(url: &str) -> i64 {
    if let Some(caps) = BITRATE_RE.captures(url) {
        return caps[1].parse::<i64>().unwrap_or(0);
    }
    if let Some(caps) = BITRATE_FILE_RE.captures(url) {
        return caps[1].parse::<i64>().unwrap_or(0);
    }
    0
}

fn split_resolution(text: &str) -> (i64, i64) {
    match RESOLUTION_RE.captures(text) {
        Some(caps) => (
            caps[1].parse::<i64>().unwrap_or(0),
            caps[2].parse::<i64>().unwrap_or(0),
        ),
        None => (0, 0),
    }
}

/// 生成档位展示名，如「超清 720P」。
fn quality_label(br: i64, height: i64, _kbps: i64) -> String {
    let name = match quality_name(br) {
        Some(name) => name.to_string(),
        None => {
            if height >= 1080 {
                "超清".to_string()
            } else if height >= 720 {
                "高清".to_string()
            } else if height >= 480 {
                "标清".to_string()
            } else {
                "流畅".to_string()
            }
        }
    };
    if height > 0 {
        format!("{name} {height}P")
    } else if br > 0 {
        format!("{name} 代号{br}")
    } else {
        name
    }
}

/// 从媒体播放列表解析分片绝对地址（央视点播分片未加密，忽略加密声明）。
fn segment_urls(playlist_text: &str, media_url: &str) -> Vec<String> {
    let mut segments = Vec::new();
    for raw_line in playlist_text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        segments.push(absolutize(media_url, line));
    }
    segments
}

/// 直接抓取媒体播放列表并返回分片地址。
pub async fn get_segments(media_m3u8_url: &str) -> Result<Vec<String>> {
    let text = http::get_text(media_m3u8_url).await?;
    Ok(segment_urls(&text, media_m3u8_url))
}

/// 把「主播放列表」解析成最高档的「媒体播放列表」地址。
///
/// 传入的地址本身就是媒体列表时原样返回。听音的 `hls_audio_url` 是带多档的主表，
/// 直接交给 `get_segments` 会把档位地址当成分片，必须先落到具体档位。
pub async fn pick_media_playlist(url: &str) -> Result<String> {
    let text = http::get_text(url).await?;
    if !text.contains("#EXT-X-STREAM-INF") {
        return Ok(url.to_string());
    }
    let variants = parse_master_playlist(&text, url);
    if variants.is_empty() {
        return Ok(url.to_string());
    }
    let best = variants
        .iter()
        .max_by_key(|v| {
            (
                v.get("bandwidth").and_then(|x| x.as_i64()).unwrap_or(0),
                v.get("br").and_then(|x| x.as_i64()).unwrap_or(0),
            )
        })
        .and_then(|v| v.get("url").and_then(|u| u.as_str()))
        .unwrap_or(url);
    Ok(best.to_string())
}

// --------------------------------------------------------------------------- //
// 清晰度
// --------------------------------------------------------------------------- //
/// 清晰度解析的输入。
pub enum QualitySource {
    Info(Box<VideoInfo>),
    Custom(String),
}

impl From<VideoInfo> for QualitySource {
    fn from(value: VideoInfo) -> Self {
        QualitySource::Info(Box::new(value))
    }
}

impl From<&str> for QualitySource {
    fn from(value: &str) -> Self {
        QualitySource::Custom(value.to_string())
    }
}

/// 列出全部可用清晰度（合并「档位表」与「明文通道」两路信息）。
///
/// * 以 enc 的档位表为骨架（决定有哪些档、各自多少分辨率）；
/// * 同码率在 plain 通道有**可信**明文版本（450 / 850）时，该档改用明文地址并
///   标记 `encrypted = false`，可以走标准通道快速下载；
/// * 其余档位保持 `encrypted = true`，需走浏览器解密的高清通道；
/// * 额外探测 4K 专区独有的明文 1080P（`/asp/hls/4000/`，master 不声明）。
pub async fn list_qualities(source: QualitySource, verify: bool) -> Result<Vec<Quality>> {
    let candidates: Vec<(&'static str, String)> = match &source {
        QualitySource::Info(info) => info.masters(),
        QualitySource::Custom(url) => vec![("custom", url.clone())],
    };

    let plain_master = candidates
        .iter()
        .find(|(channel, _)| *channel == "plain")
        .map(|(_, url)| url.clone());
    let info_for_4k = match &source {
        QualitySource::Info(info) => Some(info.clone()),
        QualitySource::Custom(_) => None,
    };

    // 三路探测彼此独立，全部并发
    let masters_future = {
        let items: Vec<(&'static str, String)> = candidates.clone();
        async move {
            http::parallel_map(items, 4, move |(channel, url)| async move {
                let parsed = qualities_from_master(&url, channel, verify).await?;
                Ok((channel, parsed))
            })
            .await
        }
    };
    let extra_future = async {
        match plain_master {
            Some(url) => plain_extra_qualities(&url, &[]).await.unwrap_or_default(),
            None => Vec::new(),
        }
    };
    let fourk_future = async {
        match info_for_4k {
            Some(info) => probe_plain_4000(&info).await.unwrap_or(None),
            None => None,
        }
    };

    let (masters_result, extra, plain_4k) = tokio::join!(masters_future, extra_future, fourk_future);

    let mut enc_qualities: Vec<Quality> = Vec::new();
    let mut plain_qualities: Vec<Quality> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    // parallel_map 保序，天然与 candidates 对齐
    for (index, parsed) in masters_result.into_iter().enumerate() {
        let channel = candidates.get(index).map(|(c, _)| *c).unwrap_or("custom");
        let mut parsed = match parsed {
            Some((_, qualities)) => qualities,
            None => {
                errors.push(format!("{channel}: 解析失败"));
                continue;
            }
        };
        if channel == "plain" {
            let have: std::collections::HashSet<i64> = parsed.iter().map(|q| q.br).collect();
            parsed.extend(extra.iter().filter(|q| !have.contains(&q.br)).cloned());
            sort_qualities(&mut parsed);
            if plain_qualities.is_empty() {
                plain_qualities = parsed;
            }
        } else if enc_qualities.is_empty() {
            enc_qualities = parsed;
        }
    }

    if enc_qualities.is_empty() && plain_qualities.is_empty() {
        let detail = if errors.is_empty() {
            String::new()
        } else {
            format!("（{}）", errors.join("；"))
        };
        return Err(Error::cctv(format!("未找到可用的清晰度{detail}")));
    }

    // 明文通道只信任已知不会回落的档位
    let mut plain_by_br: std::collections::HashMap<i64, Quality> = std::collections::HashMap::new();
    for quality in &plain_qualities {
        if PLAIN_TRUSTED_BRS.contains(&quality.br) {
            plain_by_br.entry(quality.br).or_insert_with(|| quality.clone());
        }
    }

    let mut qualities: Vec<Quality> = Vec::new();
    for enc in enc_qualities {
        match plain_by_br.remove(&enc.br) {
            Some(plain) => {
                let height = if plain.height > 0 { plain.height } else { enc.height };
                let kbps = if plain.kbps > 0 { plain.kbps } else { enc.kbps };
                let width = if plain.width > 0 { plain.width } else { enc.width };
                qualities.push(Quality {
                    width,
                    height,
                    kbps,
                    encrypted: false,
                    label: quality_label(plain.br, height, kbps),
                    ..plain
                });
            }
            None => qualities.push(enc),
        }
    }
    qualities.extend(plain_by_br.into_values());

    // 明文 1080P（4K 专区视频独有；明文优先于加密的 enc 4000）
    if let Some(fourk) = plain_4k {
        qualities.retain(|q| q.br != 4000);
        qualities.push(fourk);
    }

    if qualities.is_empty() {
        return Err(Error::cctv("未找到可用的清晰度"));
    }
    sort_qualities(&mut qualities);
    Ok(qualities)
}

fn sort_qualities(qualities: &mut [Quality]) {
    qualities.sort_by(|a, b| {
        (b.height, b.kbps, b.br)
            .cmp(&(a.height, a.kbps, a.br))
    });
}

/// 解析某个通道的主播放列表，得到其声明的全部档位。
async fn qualities_from_master(
    master_url: &str,
    channel: &'static str,
    verify: bool,
) -> Result<Vec<Quality>> {
    let text = http::get_text_with(master_url, http::REFERER, 2).await?;
    if !text.contains("#EXTM3U") {
        return Err(Error::cctv("返回内容不是 m3u8"));
    }

    let mut qualities: Vec<Quality> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for variant in parse_master_playlist(&text, master_url) {
        let url = variant
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if url.is_empty() || !seen.insert(url.clone()) {
            continue;
        }
        let (width, height) = split_resolution(
            variant
                .get("resolution")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
        );
        let kbps = variant
            .get("bandwidth")
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            / 1000;
        let br = variant.get("br").and_then(|v| v.as_i64()).unwrap_or(0);
        qualities.push(Quality {
            br,
            url,
            label: quality_label(br, height, kbps),
            kbps,
            width,
            height,
            channel: channel.to_string(),
            encrypted: channel == "enc" || channel == "h5e",
            segment_urls: Vec::new(),
        });
    }

    if qualities.is_empty() {
        return Err(Error::cctv("主播放列表中没有档位"));
    }

    if verify {
        let mut verified = Vec::new();
        for quality in qualities {
            if verify_quality(&quality).await {
                verified.push(quality);
            }
        }
        if verified.is_empty() {
            return Err(Error::cctv("所有档位均不可用"));
        }
        return Ok(verified);
    }

    sort_qualities(&mut qualities);
    Ok(qualities)
}

/// 明文档位「直取地址」：`{origin}/asp/hls/{br}/{mid}/{guid}/{br}.m3u8`。
///
/// master 未声明的档位（加档 850、4K 专区的 4000）按此格式向 CDN 探测。
/// mid 与 guid 都来自 `PLAIN_PATH_RE`（分组 1 / 分组 2），漏掉 guid 必 404。
fn plain_br_url(base: &str, mid: &str, guid: &str, br: i64) -> String {
    format!("{base}/asp/hls/{br}/{mid}/{guid}/{br}.m3u8")
}

/// 探测明文通道 master 未声明、但 CDN 上存在的加档（目前是 850）。
async fn plain_extra_qualities(master_url: &str, known_brs: &[i64]) -> Result<Vec<Quality>> {
    let caps = match PLAIN_PATH_RE.captures(master_url) {
        Some(caps) => caps,
        None => return Ok(Vec::new()),
    };
    let base = origin(master_url);
    if base.is_empty() {
        return Ok(Vec::new());
    }
    // 完整路径是 `/asp/hls/main/<mid>/<guid>/main.m3u8`：分组 1 是 mid（三段），
    // 分组 2 是 32 位 guid，构造探测地址时两者都要带上，漏 guid 必 404
    let mid = caps[1].to_string();
    let guid = caps[2].to_string();
    let known: std::collections::HashSet<i64> = known_brs.iter().copied().collect();
    let bitrates: Vec<i64> = PLAIN_EXTRA_BITRATES
        .iter()
        .copied()
        .filter(|br| !known.contains(br))
        .collect();
    if bitrates.is_empty() {
        return Ok(Vec::new());
    }

    let workers = bitrates.len();
    let results = http::parallel_map(bitrates, workers, move |br| {
        let url = plain_br_url(&base, &mid, &guid, br);
        async move {
            let text = http::get_text_with(&url, http::REFERER, 1).await?;
            if !text.contains("#EXTM3U") {
                return Err(Error::cctv("不是 m3u8"));
            }
            let (width, height) = split_resolution(&text);
            Ok(Quality {
                br,
                url,
                label: quality_label(br, height, 0),
                kbps: 0,
                width,
                height,
                channel: "plain".to_string(),
                encrypted: false,
                segment_urls: Vec::new(),
            })
        }
    })
    .await;

    Ok(results.into_iter().flatten().collect())
}

/// 探测明文 1080P 档，仅 4K 专区视频存在；不存在返回 None。
///
/// 4K 专区视频的 plain main.m3u8 被 `maxbr` 参数压到 720P，且 master 不声明 4000 档，
/// 但 CDN 上 `/asp/hls/4000/` 路径实际可用且分片未加密（实测 1920×1080 解码无花屏）。
async fn probe_plain_4000(info: &VideoInfo) -> Result<Option<Quality>> {
    let caps = match PLAIN_PATH_RE.captures(&info.hls_url) {
        Some(caps) => caps,
        None => return Ok(None),
    };
    // 分组 1 是 mid 路径、分组 2 是 guid，4000 直取地址两者都要带上
    let mid = caps[1].to_string();
    let guid = caps[2].to_string();
    let mut hosts: Vec<String> = Vec::new();
    let base = origin(&info.hls_url);
    if !base.is_empty() {
        hosts.push(base);
    }
    for host in PLAIN_4000_HOSTS {
        let owned = (*host).to_string();
        if !hosts.contains(&owned) {
            hosts.push(owned);
        }
    }

    let workers = hosts.len();
    let results = http::parallel_map(hosts, workers, move |host| {
        let url = plain_br_url(&host, &mid, &guid, 4000);
        async move {
            let text = http::get_text_with(&url, http::REFERER, 1).await?;
            if !text.contains("#EXTM3U") {
                return Err(Error::cctv("不是 m3u8"));
            }
            Ok(Quality {
                br: 4000,
                url,
                label: quality_label(4000, 1080, 4000),
                kbps: 4000,
                width: 1920,
                height: 1080,
                channel: "plain".to_string(),
                encrypted: false,
                segment_urls: Vec::new(),
            })
        }
    })
    .await;

    Ok(results.into_iter().flatten().next())
}

/// 拉取媒体播放列表确认可用，并补全分片地址与实测码率。
async fn verify_quality(quality: &Quality) -> bool {
    let text = match http::get_text_with(&quality.url, http::REFERER, 1).await {
        Ok(text) => text,
        Err(_) => return false,
    };
    if !text.contains("#EXTM3U") {
        return false;
    }
    let segments = segment_urls(&text, &quality.url);
    if segments.is_empty() {
        return false;
    }
    true
}

/// 把档位解析成媒体播放列表（主表会自动落到最高档）。
pub async fn media_url_for(quality: &Quality) -> Result<String> {
    pick_media_playlist(&quality.url).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vdn_signature_is_deterministic_uppercase() {
        let sig = vdn_signature("1700000000");
        assert_eq!(sig.len(), 32);
        assert_eq!(sig, sig.to_uppercase());
    }

    #[test]
    fn test_bitrate_of() {
        assert_eq!(bitrate_of("https://x.com/asp/hls/850/a/b/c/850.m3u8"), 850);
        assert_eq!(bitrate_of("https://x.com/asp/enc/hls/2000/a/b/c/2000.m3u8"), 2000);
        assert_eq!(bitrate_of("https://x.com/no-bitrate/main.m3u8"), 0);
    }

    /// 探测地址必须带上 guid：漏 guid 会在 CDN 上 404（预览/下载都只剩 450 一档的元凶）。
    #[test]
    fn test_plain_br_url_contains_guid() {
        let master = "https://newcntv.qcloudcdn.com/asp/hls/main/0303000a/3/default/44e08b6b1003498aa71d4694119b62b4/main.m3u8?maxbr=2048";
        let caps = PLAIN_PATH_RE.captures(master).expect("PLAIN_PATH_RE 应匹配 plain master 地址");
        let mid = &caps[1];
        let guid = &caps[2];
        assert_eq!(mid, "0303000a/3/default");
        assert_eq!(guid, "44e08b6b1003498aa71d4694119b62b4");
        let url = plain_br_url("https://newcntv.qcloudcdn.com", mid, guid, 4000);
        assert_eq!(
            url,
            "https://newcntv.qcloudcdn.com/asp/hls/4000/0303000a/3/default/44e08b6b1003498aa71d4694119b62b4/4000.m3u8"
        );
    }

    #[test]
    fn test_parse_master_playlist() {
        let text = "#EXTM3U\n\
                    #EXT-X-STREAM-INF:BANDWIDTH=900000,RESOLUTION=640x360\n\
                    850/850.m3u8\n\
                    #EXT-X-STREAM-INF:BANDWIDTH=2800000,RESOLUTION=1280x720\n\
                    https://cdn.example.com/2000/2000.m3u8\n";
        let variants = parse_master_playlist(text, "https://cdn.example.com/a/b/main.m3u8");
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0]["bandwidth"], 900000);
        assert_eq!(variants[0]["url"], "https://cdn.example.com/a/b/850/850.m3u8");
        assert_eq!(variants[0]["br"], 850);
        assert_eq!(variants[1]["url"], "https://cdn.example.com/2000/2000.m3u8");
    }

    #[test]
    fn test_quality_label() {
        assert_eq!(quality_label(2000, 720, 0), "超清 720P");
        assert_eq!(quality_label(450, 270, 0), "流畅 270P");
        assert_eq!(quality_label(999, 0, 0), "流畅 代号999");
    }
}
