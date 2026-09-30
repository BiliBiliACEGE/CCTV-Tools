//! 极简 Chrome DevTools Protocol 客户端。
//!
//! 只实现本项目用得到的部分：启动一个 headless Chromium（Edge / Chrome 均可），
//! 新建标签页、注入初始化脚本、导航、执行 JS 表达式取值。
//!
//! 之所以不引第三方 CDP 库：需要的命令只有 6 个，自己实现反而更可控，
//! 也避免为一个功能引入一整棵依赖树。

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use crate::error::{Error, Result};

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// 启动 headless 浏览器后，单个标签页的 CDP 会话。
pub struct CdpSession {
    browser: Child,
    profile_dir: PathBuf,
    ws: WsStream,
    next_id: u64,
    session_id: String,
    closed: bool,
}

/// 探测本机可用的 Chromium 内核浏览器。
pub fn find_browser() -> Option<String> {
    let mut candidates: Vec<String> = Vec::new();
    for name in ["chrome.exe", "msedge.exe"] {
        if let Some(found) = crate::ffmpeg::which(name) {
            candidates.push(found);
        }
    }
    for path in [
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    ] {
        candidates.push(path.to_string());
    }
    // playwright 下载的 chromium 兜底
    if let Some(home) = std::env::var_os("LOCALAPPDATA") {
        let base = PathBuf::from(home).join("ms-playwright");
        if let Ok(entries) = std::fs::read_dir(&base) {
            let mut dirs: Vec<PathBuf> = entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().starts_with("chromium-"))
                        .unwrap_or(false)
                })
                .collect();
            dirs.sort();
            for dir in dirs.into_iter().rev() {
                candidates.push(
                    dir.join("chrome-win")
                        .join("chrome.exe")
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }

    candidates
        .into_iter()
        .find(|path| std::path::Path::new(path).is_file())
}

impl CdpSession {
    /// 启动浏览器并打开一个 about:blank 标签页。
    pub async fn launch() -> Result<Self> {
        let exe = find_browser()
            .ok_or_else(|| Error::cctv("未找到 Chromium 内核浏览器（Edge / Chrome 都没有）"))?;

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let profile_dir = std::env::temp_dir().join(format!("cctv_cdp_{}_{}", std::process::id(), nonce));
        tokio::fs::create_dir_all(&profile_dir)
            .await
            .map_err(|exc| Error::io(format!("创建浏览器配置目录失败：{exc}")))?;

        let port_file = profile_dir.join("DevToolsActivePort");
        let mut command = Command::new(&exe);
        command.args([
            "--headless=new",
            "--remote-debugging-port=0",
            &format!("--user-data-dir={}", profile_dir.display()),
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-gpu",
            "--disable-dev-shm-usage",
            "--mute-audio",
            "--no-sandbox",
            "--autoplay-policy=no-user-gesture-required",
            "--remote-allow-origins=*",
            "--window-size=1280,720",
            "about:blank",
        ]);
        command.stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW

        let mut browser = command
            .spawn()
            .map_err(|exc| Error::io(format!("启动浏览器失败：{exc}")))?;

        // 等待 DevToolsActivePort 出现（第一行是端口号）
        let mut port: u16 = 0;
        for _ in 0..100 {
            if let Ok(text) = tokio::fs::read_to_string(&port_file).await {
                if let Some(first) = text.lines().next() {
                    if let Ok(parsed) = first.trim().parse::<u16>() {
                        port = parsed;
                        break;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        if port == 0 {
            let _ = browser.kill().await;
            let _ = tokio::fs::remove_dir_all(&profile_dir).await;
            return Err(Error::cctv("浏览器启动超时（未拿到调试端口）"));
        }

        // 取 browser 级 WebSocket 地址
        let version_url = format!("http://127.0.0.1:{port}/json/version");
        let client = crate::http::raw_client();
        let version: Value = client
            .get(&version_url)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|exc| Error::http(format!("连接调试端口失败：{exc}")))?
            .json()
            .await
            .map_err(|exc| Error::http(format!("解析调试端口响应失败：{exc}")))?;
        let ws_url = version
            .get("webSocketDebuggerUrl")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::cctv("调试端口未返回 WebSocket 地址"))?
            .to_string();

        let (ws, _) = connect_async(&ws_url)
            .await
            .map_err(|exc| Error::http(format!("连接 CDP WebSocket 失败：{exc}")))?;

        let mut session = CdpSession {
            browser,
            profile_dir,
            ws,
            next_id: 1,
            session_id: String::new(),
            closed: false,
        };

        // 新建标签页并 attach（flatten 后命令带 sessionId 即可）
        let created = session
            .call_browser(
                "Target.createTarget",
                json!({ "url": "about:blank" }),
            )
            .await?;
        let target_id = created
            .get("targetId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::cctv("创建标签页失败"))?
            .to_string();
        let attached = session
            .call_browser(
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
            )
            .await?;
        session.session_id = attached
            .get("sessionId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::cctv("附加标签页失败"))?
            .to_string();

        Ok(session)
    }

    /// 发送一条 browser 级命令（不带 sessionId）。
    async fn call_browser(&mut self, method: &str, params: Value) -> Result<Value> {
        self.call_inner(method, params, None).await
    }

    /// 发送一条页面级命令。
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let session = self.session_id.clone();
        self.call_inner(method, params, Some(session)).await
    }

    async fn call_inner(
        &mut self,
        method: &str,
        params: Value,
        session_id: Option<String>,
    ) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;

        let mut payload = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session_id {
            payload["sessionId"] = Value::String(session);
        }
        let text = serde_json::to_string(&payload)?;
        self.ws
            .send(Message::text(text))
            .await
            .map_err(|exc| Error::http(format!("发送 CDP 命令失败：{exc}")))?;

        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(Error::cctv(format!("CDP 命令超时：{method}")));
            }
            let message = match tokio::time::timeout(remaining, self.ws.next()).await {
                Ok(Some(Ok(message))) => message,
                Ok(Some(Err(exc))) => {
                    return Err(Error::http(format!("CDP 连接中断：{exc}")));
                }
                Ok(None) => return Err(Error::http("CDP 连接已关闭")),
                Err(_) => return Err(Error::cctv(format!("CDP 命令超时：{method}"))),
            };

            let text = match message {
                Message::Text(text) => text.to_string(),
                Message::Binary(_) => continue,
                Message::Close(_) => return Err(Error::http("CDP 连接被关闭")),
                _ => continue,
            };
            let value: Value = match serde_json::from_str(&text) {
                Ok(value) => value,
                Err(_) => continue,
            };
            // 忽略事件（没有 id 的消息）
            if value.get("id").and_then(|v| v.as_u64()) != Some(id) {
                continue;
            }
            if let Some(error) = value.get("error") {
                let message = error
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("未知错误");
                return Err(Error::cctv(format!("CDP {method} 失败：{message}")));
            }
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// 执行 JS 表达式并取回值（`returnByValue`）。
    pub async fn evaluate(&mut self, expression: &str) -> Result<Value> {
        let result = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": true,
                    "awaitPromise": true,
                    "userGesture": true,
                }),
            )
            .await?;

        if let Some(exception) = result.get("exceptionDetails") {
            let text = exception
                .get("exception")
                .and_then(|e| e.get("description"))
                .and_then(|v| v.as_str())
                .unwrap_or("脚本执行异常");
            return Err(Error::cctv(format!("脚本执行异常：{text}")));
        }
        Ok(result
            .get("result")
            .and_then(|r| r.get("value"))
            .cloned()
            .unwrap_or(Value::Null))
    }

    /// 导航到指定页面。
    pub async fn navigate(&mut self, url: &str) -> Result<()> {
        self.call("Page.navigate", json!({ "url": url })).await?;
        Ok(())
    }

    /// 关闭浏览器并清理临时配置目录。
    pub async fn shutdown(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        let _ = self.ws.close(None).await;
        let _ = self.browser.kill().await;
        let _ = tokio::fs::remove_dir_all(&self.profile_dir).await;
    }
}

impl Drop for CdpSession {
    fn drop(&mut self) {
        let _ = self.browser.start_kill();
    }
}
