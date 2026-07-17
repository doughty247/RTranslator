//! The UniFFI-exposed surface. This is the Phase 2 deliverable: a minimal,
//! trivial round trip proving the Java<->Rust boundary works, before any real
//! pipeline logic (Phase 3/4/5) is built on top of it.
//!
//! Deliberately small per `CLAUDE.md`'s "minimize the surface" guidance:
//! session start/stop, push an audio chunk, config get/set, and a
//! translated-text callback — nothing else crosses the boundary. Real ASR/MT
//! is not wired in yet (`push_audio_chunk` below is a stub that proves data
//! flows both ways, it does not run inference); that follows Phase 3 sign-off.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::config::{Direction, DirectionMode, HybridConfig};
use crate::error::HybridError;

/// Implemented on the Java/Kotlin side; Rust calls back into it when a
/// direction has translated text ready.
#[uniffi::export(with_foreign)]
pub trait TranslationListener: Send + Sync {
    fn on_translated_text(&self, direction: Direction, text: String);
}

#[derive(Default)]
struct SessionState {
    running: bool,
    /// Count of audio chunks received since start(), for round-trip
    /// verification only (see tests/ffi_roundtrip.rs) — not a real pipeline.
    chunks_received: u64,
}

/// One Adaptive Hybrid Mode session. Owns the config and (once Phase 4/5 land)
/// will own the live/push-to-talk pipelines; today it only proves the
/// lifecycle and callback plumbing work.
#[derive(uniffi::Object)]
pub struct HybridSession {
    config: Arc<HybridConfig>,
    state: Mutex<SessionState>,
    listener: Mutex<Option<Arc<dyn TranslationListener>>>,
}

#[uniffi::export]
impl HybridSession {
    #[uniffi::constructor]
    pub fn new(config: Arc<HybridConfig>) -> Self {
        Self {
            config,
            state: Mutex::new(SessionState::default()),
            listener: Mutex::new(None),
        }
    }

    pub fn set_listener(&self, listener: Arc<dyn TranslationListener>) {
        *self.listener.lock() = Some(listener);
    }

    pub fn start(&self) -> Result<(), HybridError> {
        let mut state = self.state.lock();
        if state.running {
            return Err(HybridError::AlreadyRunning);
        }
        state.running = true;
        state.chunks_received = 0;
        Ok(())
    }

    pub fn stop(&self) -> Result<(), HybridError> {
        let mut state = self.state.lock();
        if !state.running {
            return Err(HybridError::NotRunning);
        }
        state.running = false;
        Ok(())
    }

    /// Push a raw PCM chunk (mono 16kHz per
    /// `docs/model-artifact-contract.md` §2) for the given direction.
    ///
    /// Stub for now: does not run VAD/ASR/MT (Phase 4/5). It records receipt
    /// and, if a listener is registered, echoes a placeholder string back
    /// through the callback so the full Java -> Rust -> Java path can be
    /// exercised end to end before real inference exists.
    pub fn push_audio_chunk(
        &self,
        direction: Direction,
        pcm: Vec<f32>,
    ) -> Result<(), HybridError> {
        {
            let mut state = self.state.lock();
            if !state.running {
                return Err(HybridError::NotRunning);
            }
            state.chunks_received += 1;
        }

        if let Some(listener) = self.listener.lock().as_ref() {
            let target = self.config.target_language(direction);
            listener.on_translated_text(
                direction,
                format!("[stub: {} samples queued for -> {target}]", pcm.len()),
            );
        }
        Ok(())
    }

    pub fn mode(&self, direction: Direction) -> DirectionMode {
        self.config.mode(direction)
    }

    pub fn set_mode(&self, direction: Direction, mode: DirectionMode) {
        self.config.set_mode(direction, mode);
    }

    pub fn is_running(&self) -> bool {
        self.state.lock().running
    }

    /// Chunks received since the last `start()` — exposed only so the round
    /// trip test / Java smoke test can assert data actually crossed the
    /// boundary, not a real API surface.
    pub fn chunks_received(&self) -> u64 {
        self.state.lock().chunks_received
    }
}
