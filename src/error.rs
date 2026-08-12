use thiserror::Error;

pub type Result<T> = std::result::Result<T, VoxError>;

#[derive(Debug, Error)]
pub enum VoxError {
    #[error("audio error: {0}")]
    Audio(String),

    #[error("transcription error: {0}")]
    Transcription(String),

    #[error("processing error: {0}")]
    Processing(String),

    #[error("injection error: {0}")]
    Injection(String),

    #[error("detection error: {0}")]
    Detection(String),

    #[error("privacy blocked: {0}")]
    PrivacyBlocked(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

impl From<anyhow::Error> for VoxError {
    fn from(err: anyhow::Error) -> Self {
        VoxError::Other(err.to_string())
    }
}

impl From<serde_yaml::Error> for VoxError {
    fn from(err: serde_yaml::Error) -> Self {
        VoxError::Config(err.to_string())
    }
}

impl From<serde_json::Error> for VoxError {
    fn from(err: serde_json::Error) -> Self {
        VoxError::Other(err.to_string())
    }
}
