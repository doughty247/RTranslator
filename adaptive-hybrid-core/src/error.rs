#[derive(uniffi::Error, thiserror::Error, Debug)]
pub enum HybridError {
    #[error("failed to load model: {0}")]
    ModelLoad(String),
    #[error("session already running")]
    AlreadyRunning,
    #[error("session not running")]
    NotRunning,
}
