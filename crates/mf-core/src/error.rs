//! 统一错误类型。

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("数据源错误 [{source_name}]: {detail}")]
    Source { source_name: String, detail: String },

    #[error("数据缺失: {0}")]
    MissingData(String),

    #[error("数据校验失败: {0}")]
    Validation(String),

    #[error("配置错误: {0}")]
    Config(String),

    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("其他错误: {0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn source_err(source: impl Into<String>, detail: impl Into<String>) -> Self {
        Error::Source { source_name: source.into(), detail: detail.into() }
    }
}
