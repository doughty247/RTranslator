//! Voice-activity gating for the live/ambient direction.
//!
//! Deliberately not implemented yet. `docs/adaptive-hybrid-mode-design.md`
//! (Phase 3) needs to settle two things this module depends on before real
//! logic goes here:
//!
//! 1. The playback-window coordination signal from Java (see
//!    `docs/echo-safety-analysis.md`) — VAD gating must suppress triggers
//!    during/just after TTS playback, and the shape of that signal (hard
//!    mute vs. sensitivity adjustment, timing) isn't decided yet.
//! 2. Whether VAD runs on raw energy thresholds (RTranslator's existing
//!    `Recorder.java` approach: `DEFAULT_AMPLITUDE_THRESHOLD`,
//!    `DEFAULT_SPEECH_TIMEOUT_MILLIS`) or something more robust — an
//!    open design question, not a foregone conclusion.
//!
//! Implementing this before that sign-off would mean building on an
//! unconfirmed interface. See `CLAUDE.md`'s "What Requires Developer
//! Sign-Off Before Proceeding".

/// Placeholder gate result. Real VAD state machine lands in Phase 4.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateState {
    Listening,
    Suppressed,
}
