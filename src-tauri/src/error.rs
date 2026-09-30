//! 统一错误类型。所有对外命令都返回 `Result<T, Error>`，Error 序列化为字符串给前端。

use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 央视网接口 / 数据解析层面的错误（对用户可见的业务错误）。
    #[error("{0}")]
    Cctv(String),

    /// 网络层错误。
    #[error("网络请求失败：{0}")]
    Http(String),

    /// 解析错误。
    #[error("解析失败：{0}")]
    Parse(String),

    /// 本地文件 / 进程错误。
    #[error("{0}")]
    Io(String),

    /// 用户取消。
    #[error("已取消")]
    Cancelled,
}

impl Error {
    pub fn cctv(msg: impl Into<String>) -> Self {
        Error::Cctv(msg.into())
    }
    pub fn http(msg: impl Into<String>) -> Self {
        Error::Http(msg.into())
    }
    pub fn parse(msg: impl Into<String>) -> Self {
        Error::Parse(msg.into())
    }
    pub fn io(msg: impl Into<String>) -> Self {
        Error::Io(msg.into())
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Error::Io(value.to_string())
    }
}

impl From<reqwest::Error> for Error {
    fn from(value: reqwest::Error) -> Self {
        if value.is_timeout() {
            Error::Http("请求超时".into())
        } else if value.is_decode() {
            Error::Parse(value.to_string())
        } else {
            Error::Http(value.to_string())
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Error::Parse(value.to_string())
    }
}

/// 让 `Error` 能直接作为 tauri 命令的错误类型返回。
impl Serialize for Error {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
