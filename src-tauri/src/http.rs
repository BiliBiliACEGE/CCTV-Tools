//! HTTP 基础层。
//!
//! 两个「隐形杀手」都在这里处理（详见 docs/CCTV片库接口分析.md 第十章）：
//!
//! 1. **WAF 校验浏览器一致性请求头**：UA 声称 Chrome 却缺 `sec-ch-ua` /
//!    `Sec-Fetch-*` 时，`api.cntv.cn` 返回 302 且 `Location` 指向自身（等于拒绝）。
//!    这里统一带上整套头，并把 3xx 自指重定向当作「瞬时限流」按可重试错误处理。
//! 2. **Windows EcoQoS 降速**：自认为处于后台的进程，同一 HTTPS 请求实测从
//!    ~280ms 涨到 ~650ms（2.3 倍）。启动时显式关闭执行速度节流。

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use once_cell::sync::Lazy;
use reqwest::header::{HeaderMap, HeaderValue};

use crate::error::{Error, Result};

pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                      (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
pub const REFERER: &str = "https://tv.cctv.com/";

/// 浏览器一致性请求头。少一个都可能被 WAF 打成 302。
const HEADER_PAIRS: &[(&str, &str)] = &[
    ("Accept", "*/*"),
    ("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8"),
    (
        "sec-ch-ua",
        r#""Not_A Brand";v="8", "Chromium";v="120", "Google Chrome";v="120""#,
    ),
    ("sec-ch-ua-mobile", "?0"),
    ("sec-ch-ua-platform", r#""Windows""#),
    ("Sec-Fetch-Dest", "empty"),
    ("Sec-Fetch-Mode", "cors"),
    ("Sec-Fetch-Site", "same-site"),
    ("Origin", "https://tv.cctv.com"),
];

static BASE_HEADERS: Lazy<HeaderMap> = Lazy::new(|| {
    let mut map = HeaderMap::new();
    for (key, value) in HEADER_PAIRS {
        map.insert(
            reqwest::header::HeaderName::from_bytes(key.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
    }
    map
});

/// 全局复用的客户端：连接池 + 自动 gzip/deflate 解压。
static CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .user_agent(UA)
        .default_headers(BASE_HEADERS.clone())
        // 关掉自动跳转：自指 302 是 WAF 限流信号，需要显式识别后重试
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(32)
        .build()
        .expect("构建 HTTP 客户端失败")
});

/// 一个「干净」的客户端：不带浏览器伪装头、不重试，用于访问本机服务（如 CDP 调试端口）。
pub fn raw_client() -> reqwest::Client {
    static RAW: Lazy<reqwest::Client> = Lazy::new(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .build()
            .expect("构建本地 HTTP 客户端失败")
    });
    RAW.clone()
}

/// 带重试与浏览器头的 GET，返回原始字节。
pub async fn get_bytes_full(
    url: &str,
    referer: &str,
    timeout_secs: u64,
    retries: usize,
) -> Result<Vec<u8>> {
    let mut last: Option<Error> = None;
    for attempt in 1..=retries.max(1) {
        let result = CLIENT
            .get(url)
            .header("Referer", referer)
            .timeout(Duration::from_secs(timeout_secs))
            .send()
            .await;

        match result {
            Ok(resp) => {
                let status = resp.status();
                let code = status.as_u16();
                if status.is_success() {
                    match resp.bytes().await {
                        Ok(body) => return Ok(body.to_vec()),
                        Err(exc) => last = Some(exc.into()),
                    }
                } else if code == 429 || status.is_server_error() {
                    last = Some(Error::http(format!("HTTP {code}：{url}")));
                } else if status.is_redirection() {
                    // 自指 302：WAF 瞬时限流，退避后重试
                    last = Some(Error::cctv(format!(
                        "HTTP {code}（疑似被 WAF 限流，可稍后重试）：{url}"
                    )));
                } else {
                    return Err(Error::cctv(format!("HTTP {code}：{url}")));
                }
            }
            Err(exc) => last = Some(exc.into()),
        }

        if attempt < retries.max(1) {
            tokio::time::sleep(Duration::from_millis(900 * attempt as u64)).await;
        }
    }
    Err(last.unwrap_or_else(|| Error::http(format!("请求失败：{url}"))))
}

/// 默认参数（25s 超时、3 次重试）的 GET。
pub async fn get_bytes(url: &str) -> Result<Vec<u8>> {
    get_bytes_full(url, REFERER, 25, 3).await
}

/// GET 并解码为文本（UTF-8，非法字节替换）。
pub async fn get_text(url: &str) -> Result<String> {
    let raw = get_bytes(url).await?;
    Ok(String::from_utf8_lossy(&raw).into_owned())
}

/// 指定 Referer 的 GET 文本。
pub async fn get_text_with(url: &str, referer: &str, retries: usize) -> Result<String> {
    let raw = get_bytes_full(url, referer, 25, retries).await?;
    Ok(String::from_utf8_lossy(&raw).into_owned())
}

/// 一次性并发映射，**保持输入顺序**；单个任务失败记作 `None`，不拖垮整批。
///
/// 央视网单次请求约 190ms，串行探测十几个年份 / 档位就是数秒等待，
/// 所有「一次要打多个同构请求」的地方都走这里。
pub async fn parallel_map<A, B, F, Fut>(items: Vec<A>, workers: usize, f: F) -> Vec<Option<B>>
where
    A: Send + 'static,
    B: Send + 'static,
    F: Fn(A) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<B>> + Send,
{
    if items.is_empty() {
        return Vec::new();
    }
    let f = Arc::new(f);
    futures_util::stream::iter(items.into_iter().map(move |item| {
        let f = Arc::clone(&f);
        async move { f(item).await.ok() }
    }))
    .buffered(workers.max(1))
    .collect()
    .await
}

/// 取远端文件长度（HEAD），失败返回 None。
/// 探测远端文件长度（HEAD）。当前没有调用方，保留给「先看体积再决定下载」的场景。
#[allow(dead_code)]
pub async fn head_length(url: &str) -> Option<u64> {
    let resp = CLIENT
        .head(url)
        .header("Referer", REFERER)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.content_length()
}

/// 关闭 Windows 的「执行速度节流」(EcoQoS)，成功返回 true。
///
/// 直接声明 kernel32 符号，避免为一个调用引入额外的 sys crate。
#[cfg(windows)]
pub fn disable_power_throttling() -> bool {
    use std::ffi::c_void;

    #[repr(C)]
    struct ProcessPowerThrottlingState {
        version: u32,
        control_mask: u32,
        state_mask: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
        fn SetProcessInformation(
            process: *mut c_void,
            info_class: i32,
            info: *mut c_void,
            size: u32,
        ) -> i32;
    }

    const PROCESS_POWER_THROTTLING: i32 = 4;
    const PROCESS_POWER_THROTTLING_EXECUTION_SPEED: u32 = 0x1;

    // ControlMask 声明「我要管执行速度」，StateMask 为 0 表示「不要节流」
    let mut state = ProcessPowerThrottlingState {
        version: 1,
        control_mask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        state_mask: 0,
    };
    unsafe {
        SetProcessInformation(
            GetCurrentProcess(),
            PROCESS_POWER_THROTTLING,
            &mut state as *mut _ as *mut c_void,
            std::mem::size_of::<ProcessPowerThrottlingState>() as u32,
        ) != 0
    }
}

#[cfg(not(windows))]
pub fn disable_power_throttling() -> bool {
    false
}
