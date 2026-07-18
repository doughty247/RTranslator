//! Solo Conversation mode core: per-direction live/push-to-talk speech
//! translation, called from RTranslator's Java app shell via UniFFI.
//!
//! See `README.md` for scope, licensing (NLLB is non-commercial-only, see
//! there), and current build status. See `docs/model-artifact-contract.md`
//! and `docs/solo-conversation-mode-design.md` (repo root `docs/`) for the
//! design this crate implements.

pub mod asr;
pub mod config;
pub mod error;
pub mod ffi;
pub mod langid;
pub mod mt;
pub mod pipeline;
pub mod runtime_guard;
pub mod vad;

uniffi::setup_scaffolding!();
