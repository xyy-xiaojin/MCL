//! 统一错误类型。
//!
//! Tauri 的 `#[tauri::command]` 要求错误能序列化后回传给前端，
//! 所以这里手动实现 `Serialize`，前端拿到的就是一个可直接展示的字符串。
use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("网络请求失败：{0}")]
    Http(#[from] reqwest::Error),

    #[error("文件操作失败：{0}")]
    Io(#[from] std::io::Error),

    #[error("JSON 解析失败：{0}")]
    Json(#[from] serde_json::Error),

    #[error("压缩包处理失败：{0}")]
    Zip(#[from] zip::result::ZipError),

    /// 下载内容与清单里的 SHA-1 不一致。
    #[error("哈希校验失败：期望 {expected}，实际 {actual}")]
    Checksum { expected: String, actual: String },

    #[error("{0}")]
    Msg(String),
}

impl Error {
    pub fn msg(text: impl Into<String>) -> Self {
        Self::Msg(text.into())
    }
}

impl Serialize for Error {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
