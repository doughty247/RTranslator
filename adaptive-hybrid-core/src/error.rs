#[derive(uniffi::Error, thiserror::Error, Debug)]
pub enum HybridError {
    #[error("failed to load model: {0}")]
    ModelLoad(String),
    #[error("session already running")]
    AlreadyRunning,
    #[error("session not running")]
    NotRunning,
    /// `ORT_DYLIB_PATH` isn't set or doesn't point at a real file. Returned
    /// instead of ever calling into `ort::session::Session::builder()` —
    /// see `asr.rs`/`mt.rs` module docs: that call hangs rather than erroring
    /// when the ONNX Runtime shared library can't be resolved, so callers
    /// must not reach it without this check passing first.
    #[error("ONNX Runtime shared library not available: {0}")]
    OrtRuntimeUnavailable(String),
}
