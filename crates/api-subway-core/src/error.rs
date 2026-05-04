use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("failed to read configuration at {path}: {source}")]
    ConfigRead {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("configuration must be a regular file: {0}")]
    ConfigNotFile(PathBuf),
    #[error("configuration exceeds the 1 MiB input budget: {0}")]
    ConfigBudget(PathBuf),
    #[error("failed to parse configuration at {path}: {source}")]
    ConfigParse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("unsupported configuration version {0}; expected version 1")]
    UnsupportedConfigVersion(u32),
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
}
