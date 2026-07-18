//! The UniFFI-exposed surface, now wired to the real §2/§4/§5/§6/§7
//! architecture from `docs/adaptive-hybrid-mode-design.md` once the
//! developer signed off on it. Kept intentionally minimal per `CLAUDE.md`
//! ("a small, stable UniFFI interface is what makes this portable later"):
//! session start/stop, push an undirected audio chunk (§3 — Rust does the
//! fan-out internally per §2 Option A, Java never has to pre-label which
//! direction a chunk belongs to), push-to-talk bracketing, config get/set,
//! the playback-window echo-safety signal (§4), and two callbacks — one for
//! translated text, one optional one for §7's field-instrumentation events.
//!
//! What's still a stub: `resolve_utterance` below produces placeholder text
//! instead of running real ASR/MT. `pipeline.rs`'s `PipelineManager` now
//! correctly gates *when* an utterance boundary happens (VAD, push-to-talk
//! bracketing, §5/§6 disambiguation for the both-Live case) — what's missing
//! is only the actual `asr.rs`/`mt.rs` inference call on the captured PCM,
//! which this sandbox cannot verify without the real Whisper/NLLB weight
//! files (`docs/model-artifact-contract.md` §1). Wiring that in is Phase 5's
//! remaining job; the shape of when/what it's called with is no longer a
//! stub.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::config::{Direction, DirectionMode, HybridConfig};
use crate::error::HybridError;
use crate::pipeline::{PipelineEvent, PipelineManager};
use crate::vad::{GateEvent, VadConfig};

/// Implemented on the Java/Kotlin side; Rust calls back into it when a
/// direction has translated text ready.
#[uniffi::export(with_foreign)]
pub trait TranslationListener: Send + Sync {
    fn on_translated_text(&self, direction: Direction, text: String);
}

/// §7: optional field-instrumentation hook. Off by default (no listener
/// registered => zero overhead beyond the event already being computed by
/// `pipeline.rs`, which happens regardless since `UtteranceStarted`/`Ended`
/// drive the real pipeline too, not just debugging).
#[uniffi::export(with_foreign)]
pub trait DebugListener: Send + Sync {
    fn on_debug_event(&self, direction: Direction, event: GateEvent, elapsed_ms: u64);
}

#[derive(Default)]
struct SessionState {
    running: bool,
}

/// One Adaptive Hybrid Mode session: owns the config and the per-direction
/// pipeline state (`PipelineManager` — §2's two independent loops, §4's echo
/// gating, §5/§6's disambiguation).
#[derive(uniffi::Object)]
pub struct HybridSession {
    config: Arc<HybridConfig>,
    state: Mutex<SessionState>,
    pipeline: Mutex<PipelineManager>,
    listener: Mutex<Option<Arc<dyn TranslationListener>>>,
    debug_listener: Mutex<Option<Arc<dyn DebugListener>>>,
}

#[uniffi::export]
impl HybridSession {
    #[uniffi::constructor]
    pub fn new(config: Arc<HybridConfig>) -> Self {
        Self {
            config,
            state: Mutex::new(SessionState::default()),
            pipeline: Mutex::new(PipelineManager::new(VadConfig::default())),
            listener: Mutex::new(None),
            debug_listener: Mutex::new(None),
        }
    }

    pub fn set_listener(&self, listener: Arc<dyn TranslationListener>) {
        *self.listener.lock() = Some(listener);
    }

    /// §7. Registering this has no effect on the translated-text path — it
    /// only adds visibility into VAD/echo-window transitions already
    /// happening internally.
    pub fn set_debug_listener(&self, listener: Arc<dyn DebugListener>) {
        *self.debug_listener.lock() = Some(listener);
    }

    pub fn start(&self) -> Result<(), HybridError> {
        let mut state = self.state.lock();
        if state.running {
            return Err(HybridError::AlreadyRunning);
        }
        state.running = true;
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

    /// Push one raw PCM chunk (mono 16kHz `f32`, `[-1.0, 1.0]`, per
    /// `docs/model-artifact-contract.md` §2) — undirected; `PipelineManager`
    /// fans it out internally to whichever Live/PushToTalk loops are armed
    /// (§2 Option A, §3).
    ///
    /// `chunk_duration_ms` is the wall-clock duration of `pcm` as captured
    /// by Java's `AudioRecord` — used to advance the VAD gate's internal
    /// audio-domain clock (`vad.rs`'s module docs explain why that's
    /// deliberately not wall-clock `Instant`).
    pub fn push_audio_chunk(
        &self,
        pcm: Vec<f32>,
        chunk_duration_ms: u32,
    ) -> Result<(), HybridError> {
        if !self.state.lock().running {
            return Err(HybridError::NotRunning);
        }

        let events = self
            .pipeline
            .lock()
            .process_chunk(&self.config, &pcm, chunk_duration_ms);

        for event in events {
            self.handle_pipeline_event(event);
        }
        Ok(())
    }

    /// §4's playback-window signal. Java calls this around every
    /// `TextToSpeech` start/done callback for a Live direction.
    pub fn notify_playback_window(&self, direction: Direction, active: bool) {
        let event = self
            .pipeline
            .lock()
            .notify_playback_window(direction, active);
        self.handle_pipeline_event(event);
    }

    /// §2/§3: brackets push-to-talk audio for `direction`. Java calls this
    /// on button-down; subsequent `push_audio_chunk` calls accumulate into
    /// that direction's buffer until `end_push_to_talk`.
    pub fn begin_push_to_talk(&self, direction: Direction) -> Result<(), HybridError> {
        if !self.state.lock().running {
            return Err(HybridError::NotRunning);
        }
        self.pipeline.lock().begin_push_to_talk(direction);
        Ok(())
    }

    /// Java calls this on button-up. Emits translated text (once Phase 5
    /// wires in real ASR/MT) if any audio was captured while armed.
    pub fn end_push_to_talk(&self, direction: Direction) -> Result<(), HybridError> {
        if !self.state.lock().running {
            return Err(HybridError::NotRunning);
        }
        if let Some(event) = self.pipeline.lock().end_push_to_talk(direction) {
            self.handle_pipeline_event(event);
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
}

impl HybridSession {
    fn handle_pipeline_event(&self, event: PipelineEvent) {
        match event {
            PipelineEvent::UtteranceReady { direction, pcm } => {
                self.emit_translation(direction, self.resolve_utterance(direction, &pcm));
            }
            PipelineEvent::PushToTalkReady { direction, pcm } => {
                self.emit_translation(direction, self.resolve_utterance(direction, &pcm));
            }
            PipelineEvent::AmbiguousUtteranceReady { pcm } => {
                // §5/§6's disambiguation (pipeline.rs's resolve_ambiguous)
                // needs forced-decoded ASR text in both candidate languages
                // before it can run — that's real inference this sandbox
                // can't perform (docs/model-artifact-contract.md §1). Phase
                // 5 must forced-decode `pcm` in both directions' source
                // languages and call `PipelineManager::resolve_ambiguous`
                // with the results before emitting a translation here.
                // Deliberately not forwarded to either callback: it isn't a
                // VAD/playback event §7's listener expects, and emitting a
                // guessed direction to §3's listener would defeat the point
                // of disambiguating in the first place.
                let _ = pcm;
            }
            PipelineEvent::Debug {
                direction,
                event,
                elapsed_ms,
            } => {
                if let Some(listener) = self.debug_listener.lock().as_ref() {
                    listener.on_debug_event(direction, event, elapsed_ms);
                }
            }
        }
    }

    /// Placeholder for the real ASR -> MT call Phase 5 wires in
    /// (`asr.rs`/`mt.rs`'s model-loading is already in place; the decode
    /// loops themselves are the remaining, currently-unverifiable-here
    /// piece — see `docs/model-artifact-contract.md` §1-2).
    fn resolve_utterance(&self, direction: Direction, pcm: &[f32]) -> String {
        let target = self.config.target_language(direction);
        format!("[stub: {} samples captured for -> {target}]", pcm.len())
    }

    fn emit_translation(&self, direction: Direction, text: String) {
        if let Some(listener) = self.listener.lock().as_ref() {
            listener.on_translated_text(direction, text);
        }
    }
}
