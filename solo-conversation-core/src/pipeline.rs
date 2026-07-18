//! Live-direction and push-to-talk-direction state machines
//! (`docs/solo-conversation-mode-design.md` §2, Option A: two independent
//! per-direction loops with Rust-side audio fan-out, rather than a single
//! shared loop). Also implements §6's sticky-language-bias tiebreak, folded
//! in here per that section's note that it "lives inside the disambiguation
//! step, doesn't change the UniFFI surface."
//!
//! Each direction gets its own `VadGate` (§4's echo-safety gating) and its
//! own push-to-talk armed/buffer state, independent of the other direction —
//! per Option A, a Live direction keeps listening even while the other
//! direction's push-to-talk button is held, and vice versa; nothing here
//! makes one direction's state depend on the other's *mode*, only (for the
//! both-Live case) on resolving *which* direction an utterance belongs to
//! after the fact.
//!
//! What this module does NOT do: run ASR or MT. `PipelineEvent::UtteranceReady`
//! /`AmbiguousUtteranceReady`/`PushToTalkReady` hand off captured PCM once a
//! VAD/button boundary is reached; wiring that PCM into `asr.rs`/`mt.rs`'s
//! (currently model-loading-only) Whisper/NLLB sessions is Phase 5 work this
//! session cannot verify without the real weight files
//! (`docs/model-artifact-contract.md` §1).

use crate::config::{Direction, DirectionMode, HybridConfig};
use crate::langid;
use crate::vad::{GateEvent, VadConfig, VadGate};

/// How close two ASR confidence scores must be to count as a genuine tie
/// (falling through to the sticky-bias tiebreak) rather than one candidate
/// clearly winning. Arbitrary starting point, not empirically tuned — real
/// ASR confidence distributions aren't available in this sandbox (no model
/// weights, `docs/model-artifact-contract.md` §1); revisit once Phase 5 has
/// real confidence scores to look at.
const CONFIDENCE_TIE_EPSILON: f64 = 0.05;

/// §6: tracks which direction most recently "won" a both-Live disambiguation,
/// as a tiebreak for the next ambiguous case — not an override of a
/// confident signal. See `resolve_ambiguous`.
#[derive(Debug, Default)]
pub struct ConversationState {
    last_direction: Option<Direction>,
}

impl ConversationState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, direction: Direction) {
        self.last_direction = Some(direction);
    }

    pub fn bias(&self) -> Option<Direction> {
        self.last_direction
    }
}

struct DirectionRuntime {
    live_gate: VadGate,
    live_buffer: Vec<f32>,
    ptt_armed: bool,
    ptt_buffer: Vec<f32>,
}

impl DirectionRuntime {
    fn new(vad_config: VadConfig) -> Self {
        Self {
            live_gate: VadGate::new(vad_config),
            live_buffer: Vec::new(),
            ptt_armed: false,
            ptt_buffer: Vec::new(),
        }
    }
}

/// A boundary event the caller (`ffi.rs`) should act on: hand `pcm` off to
/// ASR (once Phase 5 wires that in), or log a debug event (§7).
#[derive(Debug, Clone)]
pub enum PipelineEvent {
    /// Only one direction could plausibly own this utterance — either the
    /// other direction is PushToTalk/Off, or only one direction is Live.
    UtteranceReady { direction: Direction, pcm: Vec<f32> },
    /// Both directions are Live and both flagged the same utterance boundary
    /// (expected: both gates watch identical audio under identical config,
    /// so they move in lockstep — see `process_chunk`). Phase 5 must
    /// forced-decode ASR in both directions' source languages and call
    /// `resolve_ambiguous` with the results.
    AmbiguousUtteranceReady { pcm: Vec<f32> },
    /// A push-to-talk capture completed (button released).
    PushToTalkReady { direction: Direction, pcm: Vec<f32> },
    /// §7's field-instrumentation hook — surfaced to `ffi.rs`'s optional
    /// debug callback, no effect on the non-debug path.
    Debug {
        direction: Direction,
        event: GateEvent,
        elapsed_ms: u64,
    },
}

/// Owns both directions' independent loop state for one `HybridSession`.
pub struct PipelineManager {
    first_to_second: DirectionRuntime,
    second_to_first: DirectionRuntime,
    conversation: ConversationState,
}

impl PipelineManager {
    pub fn new(vad_config: VadConfig) -> Self {
        Self {
            first_to_second: DirectionRuntime::new(vad_config),
            second_to_first: DirectionRuntime::new(vad_config),
            conversation: ConversationState::new(),
        }
    }

    fn runtime_mut(&mut self, direction: Direction) -> &mut DirectionRuntime {
        match direction {
            Direction::FirstToSecond => &mut self.first_to_second,
            Direction::SecondToFirst => &mut self.second_to_first,
        }
    }

    /// §4's `notify_playback_window`, routed to the given direction's gate.
    pub fn notify_playback_window(&mut self, direction: Direction, active: bool) -> PipelineEvent {
        let gate = &mut self.runtime_mut(direction).live_gate;
        let event = gate.notify_playback_window(active);
        PipelineEvent::Debug {
            direction,
            event,
            elapsed_ms: gate.elapsed_ms(),
        }
    }

    pub fn begin_push_to_talk(&mut self, direction: Direction) {
        let runtime = self.runtime_mut(direction);
        runtime.ptt_armed = true;
        runtime.ptt_buffer.clear();
    }

    /// Returns the captured utterance, if any audio arrived while armed.
    pub fn end_push_to_talk(&mut self, direction: Direction) -> Option<PipelineEvent> {
        let runtime = self.runtime_mut(direction);
        runtime.ptt_armed = false;
        if runtime.ptt_buffer.is_empty() {
            return None;
        }
        let pcm = std::mem::take(&mut runtime.ptt_buffer);
        Some(PipelineEvent::PushToTalkReady { direction, pcm })
    }

    /// Fan one raw PCM chunk out to whichever loops are currently armed:
    /// every `Live` direction's VAD gate (buffering audio only once that
    /// gate is mid-utterance — the ~`min_voice_ms` ramp-up before
    /// `UtteranceStarted` fires isn't captured; acceptable for Phase 5's
    /// first cut, revisit if losing that pre-roll hurts ASR accuracy), and
    /// any `PushToTalk` direction currently between `begin_push_to_talk` and
    /// `end_push_to_talk`. `Off` directions are skipped entirely, not just
    /// muted after the fact (`docs/solo-conversation-mode-design.md` §1a).
    pub fn process_chunk(
        &mut self,
        config: &HybridConfig,
        pcm: &[f32],
        chunk_duration_ms: u32,
    ) -> Vec<PipelineEvent> {
        let mut events = Vec::new();
        let mut live_utterance_ended: Vec<Direction> = Vec::new();

        for direction in [Direction::FirstToSecond, Direction::SecondToFirst] {
            match config.mode(direction) {
                DirectionMode::Live => {
                    let runtime = self.runtime_mut(direction);
                    let gate_event = runtime.live_gate.process_chunk(pcm, chunk_duration_ms);
                    if runtime.live_gate.is_in_utterance() {
                        runtime.live_buffer.extend_from_slice(pcm);
                    }
                    if let Some(event) = gate_event {
                        events.push(PipelineEvent::Debug {
                            direction,
                            event,
                            elapsed_ms: runtime.live_gate.elapsed_ms(),
                        });
                        if event == GateEvent::UtteranceEnded {
                            live_utterance_ended.push(direction);
                        }
                    }
                }
                DirectionMode::PushToTalk => {
                    let runtime = self.runtime_mut(direction);
                    if runtime.ptt_armed {
                        runtime.ptt_buffer.extend_from_slice(pcm);
                    }
                }
                DirectionMode::Off => {}
            }
        }

        if live_utterance_ended.len() == 2 {
            // Both directions Live, both ended on this chunk — expected,
            // since identical audio through identical VadConfig keeps the
            // two gates in lockstep (see struct docs). One ambiguous event,
            // not two, so `ffi.rs` doesn't have to deduplicate.
            let pcm = std::mem::take(&mut self.first_to_second.live_buffer);
            self.second_to_first.live_buffer.clear();
            events.push(PipelineEvent::AmbiguousUtteranceReady { pcm });
        } else {
            for direction in live_utterance_ended {
                let pcm = std::mem::take(&mut self.runtime_mut(direction).live_buffer);
                events.push(PipelineEvent::UtteranceReady { direction, pcm });
            }
        }

        events
    }

    /// Resolves an `AmbiguousUtteranceReady` once Phase 5 has forced-decoded
    /// ASR text in both candidate languages. Mirrors
    /// `WalkieTalkieService.compareResults()`'s own fallback chain — MLKit
    /// (here, `langid::disambiguate`) first, then ASR confidence comparison
    /// — with §6's sticky bias added as a final tiebreak instead of an
    /// arbitrary default.
    pub fn resolve_ambiguous(
        &mut self,
        config: &HybridConfig,
        text_first_to_second: &str,
        text_second_to_first: &str,
        asr_confidence_first_to_second: f64,
        asr_confidence_second_to_first: f64,
    ) -> Direction {
        let lang_a = config.source_language(Direction::FirstToSecond);
        let lang_b = config.source_language(Direction::SecondToFirst);

        let resolved = if let Some(result) =
            langid::disambiguate(text_first_to_second, &lang_a, text_second_to_first, &lang_b)
        {
            if result.favors_a {
                Direction::FirstToSecond
            } else {
                Direction::SecondToFirst
            }
        } else if (asr_confidence_first_to_second - asr_confidence_second_to_first).abs()
            > CONFIDENCE_TIE_EPSILON
        {
            if asr_confidence_first_to_second > asr_confidence_second_to_first {
                Direction::FirstToSecond
            } else {
                Direction::SecondToFirst
            }
        } else {
            self.conversation.bias().unwrap_or(Direction::FirstToSecond)
        };

        self.conversation.record(resolved);
        resolved
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DirectionMode;

    fn loud_chunk(len: usize) -> Vec<f32> {
        vec![0.5; len]
    }

    fn silent_chunk(len: usize) -> Vec<f32> {
        vec![0.0; len]
    }

    fn config(a: DirectionMode, b: DirectionMode) -> HybridConfig {
        HybridConfig::new("en".into(), "es".into(), a, b)
    }

    #[test]
    fn single_live_direction_reports_an_unambiguous_utterance() {
        let config = config(DirectionMode::Live, DirectionMode::Off);
        let mut pipeline = PipelineManager::new(VadConfig::default());

        let mut saw_ready = false;
        for _ in 0..20 {
            let events = pipeline.process_chunk(&config, &loud_chunk(160), 100);
            for event in events {
                assert!(
                    !matches!(event, PipelineEvent::AmbiguousUtteranceReady { .. }),
                    "only one direction is Live, there's nothing to disambiguate"
                );
                if let PipelineEvent::UtteranceReady { direction, pcm } = event {
                    assert_eq!(direction, Direction::FirstToSecond);
                    assert!(!pcm.is_empty());
                    saw_ready = true;
                }
            }
        }
        for _ in 0..15 {
            let events = pipeline.process_chunk(&config, &silent_chunk(160), 100);
            for event in events {
                if let PipelineEvent::UtteranceReady { .. } = event {
                    saw_ready = true;
                }
            }
        }
        assert!(
            saw_ready,
            "expected an UtteranceReady after voice then silence"
        );
    }

    #[test]
    fn both_live_directions_report_one_ambiguous_event_not_two() {
        let config = config(DirectionMode::Live, DirectionMode::Live);
        let mut pipeline = PipelineManager::new(VadConfig::default());

        let mut ambiguous_count = 0;
        let mut unambiguous_count = 0;
        for _ in 0..35 {
            for event in pipeline.process_chunk(&config, &loud_chunk(160), 100) {
                match event {
                    PipelineEvent::AmbiguousUtteranceReady { .. } => ambiguous_count += 1,
                    PipelineEvent::UtteranceReady { .. } => unambiguous_count += 1,
                    _ => {}
                }
            }
        }
        for _ in 0..15 {
            for event in pipeline.process_chunk(&config, &silent_chunk(160), 100) {
                match event {
                    PipelineEvent::AmbiguousUtteranceReady { .. } => ambiguous_count += 1,
                    PipelineEvent::UtteranceReady { .. } => unambiguous_count += 1,
                    _ => {}
                }
            }
        }
        assert_eq!(ambiguous_count, 1);
        assert_eq!(unambiguous_count, 0);
    }

    #[test]
    fn off_direction_never_buffers_or_reports() {
        let config = config(DirectionMode::Live, DirectionMode::Off);
        let mut pipeline = PipelineManager::new(VadConfig::default());

        for _ in 0..20 {
            for event in pipeline.process_chunk(&config, &loud_chunk(160), 100) {
                if let PipelineEvent::Debug { direction, .. } = event {
                    assert_ne!(
                        direction,
                        Direction::SecondToFirst,
                        "Off direction must never be touched"
                    );
                }
            }
        }
    }

    #[test]
    fn push_to_talk_only_buffers_while_armed() {
        let config = config(DirectionMode::Off, DirectionMode::PushToTalk);
        let mut pipeline = PipelineManager::new(VadConfig::default());

        // Audio arriving before begin_push_to_talk is discarded.
        pipeline.process_chunk(&config, &loud_chunk(160), 100);
        pipeline.begin_push_to_talk(Direction::SecondToFirst);
        pipeline.process_chunk(&config, &loud_chunk(160), 100);
        pipeline.process_chunk(&config, &loud_chunk(160), 100);
        let event = pipeline.end_push_to_talk(Direction::SecondToFirst);

        match event {
            Some(PipelineEvent::PushToTalkReady { direction, pcm }) => {
                assert_eq!(direction, Direction::SecondToFirst);
                assert_eq!(
                    pcm.len(),
                    320,
                    "should only contain the two chunks captured while armed"
                );
            }
            other => panic!("expected PushToTalkReady, got {other:?}"),
        }
    }

    #[test]
    fn end_push_to_talk_with_no_audio_returns_none() {
        let mut pipeline = PipelineManager::new(VadConfig::default());
        pipeline.begin_push_to_talk(Direction::FirstToSecond);
        assert!(pipeline
            .end_push_to_talk(Direction::FirstToSecond)
            .is_none());
    }

    #[test]
    fn resolve_ambiguous_prefers_langid_over_confidence_and_bias() {
        let config = config(DirectionMode::Live, DirectionMode::Live);
        let mut pipeline = PipelineManager::new(VadConfig::default());

        let resolved = pipeline.resolve_ambiguous(
            &config,
            "Hola, como estas hoy? Espero que todo vaya muy bien.", // Spanish text in the "en" slot
            "Hola, como estas hoy? Espero que todo vaya muy bien.", // Spanish text in the "es" slot (correct)
            0.5,
            0.5, // tied confidence — langid should still resolve this, no fallback needed
        );
        assert_eq!(resolved, Direction::SecondToFirst);
    }

    #[test]
    fn resolve_ambiguous_falls_back_to_confidence_then_sticky_bias() {
        let config = config(DirectionMode::Live, DirectionMode::Live);
        let mut pipeline = PipelineManager::new(VadConfig::default());

        // Gibberish neither langid call can confidently resolve, but a's
        // ASR confidence clearly wins.
        let resolved = pipeline.resolve_ambiguous(&config, "xyz123", "xyz123", 0.9, 0.1);
        assert_eq!(resolved, Direction::FirstToSecond);

        // Now confidences are tied too — should fall back to the bias just
        // recorded above (FirstToSecond).
        let resolved = pipeline.resolve_ambiguous(&config, "xyz123", "xyz123", 0.5, 0.5);
        assert_eq!(resolved, Direction::FirstToSecond);
    }

    #[test]
    fn resolve_ambiguous_with_no_prior_bias_defaults_to_first_to_second() {
        let config = config(DirectionMode::Live, DirectionMode::Live);
        let mut pipeline = PipelineManager::new(VadConfig::default());
        let resolved = pipeline.resolve_ambiguous(&config, "xyz123", "xyz123", 0.5, 0.5);
        assert_eq!(resolved, Direction::FirstToSecond);
    }
}
