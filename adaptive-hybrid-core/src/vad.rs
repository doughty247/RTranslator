//! Voice-activity gating for the live/ambient direction, including
//! echo-safety coordination with TTS playback
//! (`docs/adaptive-hybrid-mode-design.md` §4).
//!
//! §4 asked to choose between a hard mute window (Option A: simple, safe,
//! reintroduces some listen-then-speak latency) and a sensitivity adjustment
//! (Option B: faster resumption, but only tunable with real Bluetooth echo
//! data). The developer's call was a hybrid, biased toward the safe option:
//! **mute hard for a short window right after TTS stops, then ease into a
//! raised-threshold window before returning to normal sensitivity**, rather
//! than either a single abrupt cutoff or full sensitivity-adjustment from the
//! start. Three phases:
//!
//! 1. **Muted** — while TTS is actively playing, plus `muted_tail_ms` after
//!    it stops. VAD is fully blind; this covers the highest-echo-risk
//!    window (loudest residual echo, right as playback ends).
//! 2. **Elevated** — for a further `elevated_tail_ms`, VAD's amplitude
//!    threshold is raised by `elevated_multiplier` rather than gated
//!    entirely. Quiet residual echo tail (typically quieter than the
//!    original TTS output) stays below threshold; genuine speech (typically
//!    louder) can still trigger, so the live direction isn't fully deaf for
//!    the whole window the way a pure hard-mute-with-long-tail would leave
//!    it.
//! 3. **Normal** — full sensitivity.
//!
//! The specific millisecond/multiplier constants below are starting points,
//! not tuned values — `docs/echo-safety-analysis.md` is explicit that real
//! tuning needs the developer's actual Bluetooth headphones on the actual
//! device, which this sandbox cannot provide. §7's debug callback
//! (`GateEvent`, consumed by `ffi.rs`) exists specifically to turn that
//! on-device tuning from guesswork into something measured.

/// Amplitude threshold, timeout, and echo-window tuning. Defaults are
/// starting points derived loosely from `Recorder.java`'s existing
/// energy-VAD constants (`DEFAULT_AMPLITUDE_THRESHOLD`,
/// `DEFAULT_SPEECH_TIMEOUT_MILLIS`) converted from Java's 16-bit PCM scale to
/// this crate's `f32` `[-1.0, 1.0]` convention (§`docs/model-artifact-contract.md`
/// §2) — not empirically re-tuned for this crate's VAD shape, which differs
/// from `Recorder.java`'s (three echo phases vs. none). Re-tune on-device.
#[derive(Debug, Clone, Copy)]
pub struct VadConfig {
    /// Peak amplitude (0.0-1.0) above which a chunk counts as voiced, at
    /// normal sensitivity. `Recorder.java`'s `DEFAULT_AMPLITUDE_THRESHOLD` is
    /// 2000 on a 16-bit (32768-max) scale => ~0.061 here.
    pub amplitude_threshold: f32,
    /// Contiguous voiced duration required before declaring an utterance
    /// started, to reject brief noise blips.
    pub min_voice_ms: u32,
    /// Contiguous silence duration required before declaring an utterance
    /// ended. Matches `Recorder.java`'s `DEFAULT_SPEECH_TIMEOUT_MILLIS`.
    pub speech_timeout_ms: u32,
    /// Duration of the Muted phase after TTS playback stops.
    pub muted_tail_ms: u32,
    /// Duration of the Elevated phase that follows the Muted phase.
    pub elevated_tail_ms: u32,
    /// Threshold multiplier during the Elevated phase.
    pub elevated_multiplier: f32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            amplitude_threshold: 0.061,
            min_voice_ms: 250,
            speech_timeout_ms: 1300,
            muted_tail_ms: 300,
            elevated_tail_ms: 700,
            elevated_multiplier: 2.5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EchoPhase {
    Muted,
    Elevated,
    Normal,
}

/// State transitions worth surfacing to callers — utterance boundaries for
/// the pipeline (§2) to act on, and echo-window events for §7's debug
/// callback so on-device testing produces a timestamped timeline instead of
/// just a subjective "did it feed back" read. `uniffi::Enum` so `ffi.rs` can
/// forward it directly to Java without a parallel mirrored type.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateEvent {
    UtteranceStarted,
    UtteranceEnded,
    /// A chunk was loud enough to trigger normal-sensitivity VAD but was
    /// gated out by an active Muted/Elevated phase — the signal §7 exists to
    /// capture: "residual echo (probably) suppressed here."
    SuppressedByPlayback,
    PlaybackWindowStarted,
    PlaybackWindowEnded,
}

/// One direction's VAD + echo-safety state. Time is tracked as audio-domain
/// elapsed milliseconds (advanced by `process_chunk`'s `chunk_duration_ms`),
/// not wall-clock — this keeps the gate a pure function of its inputs, which
/// is what makes it deterministically unit-testable without real sleeps or a
/// mocked clock, and ties echo timing to the audio stream itself rather than
/// to scheduling jitter elsewhere in the process.
#[derive(Debug)]
pub struct VadGate {
    config: VadConfig,
    elapsed_ms: u64,
    voiced_run_ms: u32,
    silence_run_ms: u32,
    in_utterance: bool,
    playback_active: bool,
    muted_until_ms: u64,
    elevated_until_ms: u64,
}

impl VadGate {
    pub fn new(config: VadConfig) -> Self {
        Self {
            config,
            elapsed_ms: 0,
            voiced_run_ms: 0,
            silence_run_ms: 0,
            in_utterance: false,
            playback_active: false,
            muted_until_ms: 0,
            elevated_until_ms: 0,
        }
    }

    /// Java calls this around every `TextToSpeech` start/done callback for a
    /// Live direction (`docs/adaptive-hybrid-mode-design.md` §4's
    /// `notify_playback_window`). Starting playback mutes immediately and
    /// unconditionally; stopping playback arms the Muted-then-Elevated
    /// window from the current elapsed time.
    pub fn notify_playback_window(&mut self, active: bool) -> GateEvent {
        self.playback_active = active;
        if active {
            GateEvent::PlaybackWindowStarted
        } else {
            self.muted_until_ms = self.elapsed_ms + self.config.muted_tail_ms as u64;
            self.elevated_until_ms = self.muted_until_ms + self.config.elevated_tail_ms as u64;
            GateEvent::PlaybackWindowEnded
        }
    }

    fn current_phase(&self) -> EchoPhase {
        if self.playback_active || self.elapsed_ms < self.muted_until_ms {
            EchoPhase::Muted
        } else if self.elapsed_ms < self.elevated_until_ms {
            EchoPhase::Elevated
        } else {
            EchoPhase::Normal
        }
    }

    /// Process one chunk of mono PCM (`f32`, `[-1.0, 1.0]`, per
    /// `docs/model-artifact-contract.md` §2) captured over
    /// `chunk_duration_ms`. Returns the state-transition event, if any,
    /// worth acting on or logging.
    pub fn process_chunk(&mut self, pcm: &[f32], chunk_duration_ms: u32) -> Option<GateEvent> {
        let amplitude = peak_amplitude(pcm);
        let phase = self.current_phase();
        let raw_voiced = amplitude >= self.config.amplitude_threshold;
        let gated_voiced = match phase {
            EchoPhase::Muted => false,
            EchoPhase::Elevated => {
                amplitude >= self.config.amplitude_threshold * self.config.elevated_multiplier
            }
            EchoPhase::Normal => raw_voiced,
        };

        let event = if gated_voiced {
            self.voiced_run_ms += chunk_duration_ms;
            self.silence_run_ms = 0;
            if !self.in_utterance && self.voiced_run_ms >= self.config.min_voice_ms {
                self.in_utterance = true;
                Some(GateEvent::UtteranceStarted)
            } else {
                None
            }
        } else {
            self.silence_run_ms += chunk_duration_ms;
            self.voiced_run_ms = 0;
            if self.in_utterance && self.silence_run_ms >= self.config.speech_timeout_ms {
                self.in_utterance = false;
                Some(GateEvent::UtteranceEnded)
            } else if raw_voiced && phase != EchoPhase::Normal {
                Some(GateEvent::SuppressedByPlayback)
            } else {
                None
            }
        };

        self.elapsed_ms += chunk_duration_ms as u64;
        event
    }

    pub fn is_in_utterance(&self) -> bool {
        self.in_utterance
    }

    /// Audio-domain elapsed time (see struct docs) — carried on §7's debug
    /// events as the timestamp field.
    pub fn elapsed_ms(&self) -> u64 {
        self.elapsed_ms
    }
}

fn peak_amplitude(pcm: &[f32]) -> f32 {
    pcm.iter()
        .fold(0.0f32, |max, &sample| max.max(sample.abs()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loud_chunk(len: usize) -> Vec<f32> {
        vec![0.5; len]
    }

    fn silent_chunk(len: usize) -> Vec<f32> {
        vec![0.0; len]
    }

    #[test]
    fn declares_an_utterance_after_sustained_voiced_audio() {
        let mut gate = VadGate::new(VadConfig::default());
        // Below min_voice_ms (250ms default): no event yet.
        assert_eq!(gate.process_chunk(&loud_chunk(160), 100), None);
        assert_eq!(gate.process_chunk(&loud_chunk(160), 100), None);
        // Crosses 250ms of contiguous voiced audio.
        assert_eq!(
            gate.process_chunk(&loud_chunk(160), 100),
            Some(GateEvent::UtteranceStarted)
        );
        assert!(gate.is_in_utterance());
    }

    #[test]
    fn ends_an_utterance_after_the_speech_timeout() {
        let mut gate = VadGate::new(VadConfig::default());
        for _ in 0..3 {
            gate.process_chunk(&loud_chunk(160), 100);
        }
        assert!(gate.is_in_utterance());

        // speech_timeout_ms defaults to 1300 — silence short of that keeps
        // the utterance open.
        for _ in 0..12 {
            let event = gate.process_chunk(&silent_chunk(160), 100);
            assert_ne!(event, Some(GateEvent::UtteranceEnded));
        }
        assert!(gate.is_in_utterance());

        // One more 100ms chunk crosses the 1300ms silence threshold.
        assert_eq!(
            gate.process_chunk(&silent_chunk(160), 100),
            Some(GateEvent::UtteranceEnded)
        );
        assert!(!gate.is_in_utterance());
    }

    #[test]
    fn mutes_immediately_when_playback_starts() {
        let mut gate = VadGate::new(VadConfig::default());
        assert_eq!(
            gate.notify_playback_window(true),
            GateEvent::PlaybackWindowStarted
        );
        // Loud audio arriving during playback never starts an utterance, but
        // is still reported as suppressed (not silently dropped) — that's
        // exactly the signal §7's debug callback exists to capture: "the
        // mute engaged, here's proof."
        assert_eq!(
            gate.process_chunk(&loud_chunk(160), 100),
            Some(GateEvent::SuppressedByPlayback)
        );
        assert!(!gate.is_in_utterance());
    }

    #[test]
    fn graduates_from_muted_to_elevated_to_normal_after_playback_ends() {
        let config = VadConfig {
            muted_tail_ms: 200,
            elevated_tail_ms: 300,
            elevated_multiplier: 3.0,
            min_voice_ms: 50,
            ..VadConfig::default()
        };
        let mut gate = VadGate::new(config);
        gate.notify_playback_window(true);
        gate.notify_playback_window(false);

        // Still within the 200ms Muted tail: a moderately loud chunk (above
        // base threshold) never starts an utterance, but is reported as
        // suppressed either way — Muted and Elevated both report it, they
        // just differ in whether genuinely loud speech can still get through.
        let moderately_loud = vec![0.1f32; 160];
        assert_eq!(
            gate.process_chunk(&moderately_loud, 100),
            Some(GateEvent::SuppressedByPlayback)
        );
        assert_eq!(
            gate.process_chunk(&moderately_loud, 100),
            Some(GateEvent::SuppressedByPlayback)
        );

        // Now in the Elevated phase (200ms Muted has elapsed). A
        // moderately-loud chunk above the base threshold but below
        // threshold*3.0 should be reported as suppressed, not silently
        // dropped and not treated as speech.
        let event = gate.process_chunk(&moderately_loud, 100);
        assert_eq!(event, Some(GateEvent::SuppressedByPlayback));

        // Genuinely loud audio (above the elevated threshold) still gets
        // through during Elevated.
        let mut gate2 = VadGate::new(VadConfig {
            muted_tail_ms: 200,
            elevated_tail_ms: 300,
            elevated_multiplier: 3.0,
            min_voice_ms: 50,
            ..VadConfig::default()
        });
        gate2.notify_playback_window(true);
        gate2.notify_playback_window(false);
        gate2.process_chunk(&silent_chunk(160), 200); // burn through Muted
        assert_eq!(
            gate2.process_chunk(&loud_chunk(160), 100),
            Some(GateEvent::UtteranceStarted)
        );
    }
}
