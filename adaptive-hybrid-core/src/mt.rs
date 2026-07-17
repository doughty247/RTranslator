//! NLLB MT wrapper, calling ONNX Runtime directly via `ort`.
//!
//! Tensor I/O here must match `docs/model-artifact-contract.md` §3. Greedy
//! decode only — RTranslator's own beam-search path is documented upstream as
//! producing random crashes and must not be ported (see §3, "do not port the
//! beam-search path").
//!
//! Like `asr.rs`, this only loads the graphs; the decode loop is Phase 4 work
//! gated on Phase 3 design sign-off.

use std::path::Path;

use crate::error::HybridError;

pub const DECODER_LAYERS: usize = 12;
pub const HEAD_DIM: usize = 64;

/// NLLB's model files, per `docs/model-artifact-contract.md` §3. Note
/// `embed_and_lm_head` is one session invoked twice per step with different
/// roles (`use_lm_head` true/false) — RTranslator's own code does the same,
/// it isn't two separate graphs.
pub struct NllbModel {
    pub encoder: ort::session::Session,
    pub cache_initializer: ort::session::Session,
    pub decoder: ort::session::Session,
    pub embed_and_lm_head: ort::session::Session,
}

impl NllbModel {
    pub fn load(model_dir: &Path) -> Result<Self, HybridError> {
        let session = |name: &str| -> Result<ort::session::Session, HybridError> {
            let mut builder = ort::session::Session::builder()
                .map_err(|e| HybridError::ModelLoad(e.to_string()))?;
            builder
                .commit_from_file(model_dir.join(name))
                .map_err(|e| HybridError::ModelLoad(format!("{name}: {e}")))
        };

        Ok(Self {
            encoder: session("NLLB_encoder.onnx")?,
            cache_initializer: session("NLLB_cache_initializer.onnx")?,
            decoder: session("NLLB_decoder.onnx")?,
            embed_and_lm_head: session("NLLB_embed_and_lm_head.onnx")?,
        })
    }
}

/// SentencePiece raw ID -> NLLB model input ID. `Tokenizer.java:48-74` is the
/// source of truth for the 1-3 special-case remap; this must be ported from
/// that method exactly, not re-derived, before Phase 4 relies on it.
pub fn sentencepiece_id_to_nllb_id(_sp_id: u32) -> u32 {
    unimplemented!(
        "port the exact remap table from Tokenizer.java:48-74 in Phase 4 — \
         do not guess the 1-3 special case, verify against the Java source"
    )
}
