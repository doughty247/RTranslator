//! Whisper ASR wrapper, calling ONNX Runtime directly via `ort`.
//!
//! Tensor I/O here must match `docs/model-artifact-contract.md` §2 exactly
//! (input/output names, the cross-attention/self-attention cache split, the
//! special token IDs) — that document is the source of truth, reverse
//! engineered from `Recognizer.java`, not this file.
//!
//! Model loading is implemented and exercised by
//! `tests/session_loading.rs` against paths that don't exist in this sandbox
//! (the `.onnx` weights aren't available here — see the model artifact
//! contract's "not available in this environment" note), so that test
//! asserts the expected "file not found" failure mode rather than successful
//! inference. The full greedy-decode loop is Phase 4 work, gated on the
//! Phase 3 design doc's sign-off, and is deliberately not implemented yet.

use std::path::Path;

use crate::error::HybridError;

pub const DECODER_LAYERS: usize = 12;
pub const HEAD_DIM: usize = 64;
pub const START_TOKEN_ID: i64 = 50258;
pub const TRANSCRIBE_TOKEN_ID: i64 = 50359;
pub const NO_TIMESTAMPS_TOKEN_ID: i64 = 50363;
pub const EOS_TOKEN_ID: i64 = 50257;
pub const MAX_TOKENS: usize = 445;

/// Holds the four ONNX Runtime sessions one ASR pass needs. Construction only
/// loads the graphs; it does not run inference (see module docs — decode loop
/// is Phase 4).
pub struct WhisperModel {
    pub initializer: ort::session::Session,
    pub encoder: ort::session::Session,
    pub cache_initializer: ort::session::Session,
    pub decoder: ort::session::Session,
    pub detokenizer: ort::session::Session,
}

impl WhisperModel {
    /// `model_dir` is the directory the Java side already stages these files
    /// into (`Context.getFilesDir()` on-device) — Adaptive Hybrid Mode reuses
    /// that location rather than re-downloading or re-staging.
    pub fn load(model_dir: &Path) -> Result<Self, HybridError> {
        let session = |name: &str| -> Result<ort::session::Session, HybridError> {
            let mut builder = ort::session::Session::builder()
                .map_err(|e| HybridError::ModelLoad(e.to_string()))?;
            builder
                .commit_from_file(model_dir.join(name))
                .map_err(|e| HybridError::ModelLoad(format!("{name}: {e}")))
        };

        Ok(Self {
            initializer: session("Whisper_initializer.onnx")?,
            encoder: session("Whisper_encoder.onnx")?,
            cache_initializer: session("Whisper_cache_initializer.onnx")?,
            decoder: session("Whisper_decoder.onnx")?,
            detokenizer: session("Whisper_detokenizer.onnx")?,
        })
    }
}
