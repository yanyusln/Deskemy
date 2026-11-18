//! Crate-wide error type. Every command returns `Result<T, DeskemyError>`.
//! Serializes to `{ kind, message }` so the frontend can branch on `kind`
//! instead of parsing strings.

use serde::ser::{Serialize, SerializeStruct, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum DeskemyError {
    #[error("IO 错误：{0}")]
    Io(#[from] std::io::Error),

    #[error("数据库错误：{0}")]
    Database(#[from] rusqlite::Error),

    #[error("序列化错误：{0}")]
    Serde(#[from] serde_json::Error),

    #[error("压缩包错误：{0}")]
    Archive(#[from] zip::result::ZipError),

    #[error("配置错误：{0}")]
    Config(String),

    #[error("导入错误：{0}")]
    Import(String),

    #[error("解析错误：{0}")]
    Probe(String),

    #[error("播放器错误：{0}")]
    Player(String),

    /// A libmpv call that failed, with mpv's error code (`MPV_ERROR_*`).
    #[error("播放器错误：mpv 错误代码 {0}")]
    Mpv(i32),

    #[error("未找到：{0}")]
    NotFound(String),

    #[error("{0}")]
    Other(String),
}

impl DeskemyError {
    /// Stable machine-readable discriminant sent to the frontend.
    pub fn kind(&self) -> &'static str {
        match self {
            DeskemyError::Io(_) => "Io",
            DeskemyError::Database(_) => "Database",
            DeskemyError::Serde(_) => "Serde",
            DeskemyError::Archive(_) => "Archive",
            DeskemyError::Config(_) => "Config",
            DeskemyError::Import(_) => "Import",
            DeskemyError::Probe(_) => "Probe",
            DeskemyError::Player(_) | DeskemyError::Mpv(_) => "Player",
            DeskemyError::NotFound(_) => "NotFound",
            DeskemyError::Other(_) => "Other",
        }
    }
}

impl Serialize for DeskemyError {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut st = serializer.serialize_struct("DeskemyError", 2)?;
        st.serialize_field("kind", self.kind())?;
        st.serialize_field("message", &self.to_string())?;
        st.end()
    }
}

pub type Result<T> = std::result::Result<T, DeskemyError>;
