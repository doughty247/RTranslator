//! Per-direction live/push-to-talk configuration.
//!
//! One `HybridConfig` covers both directions of a two-language session. Each
//! direction's mode is independently switchable at runtime (Phase 5 wires the
//! settings UI to this); nothing here restarts a session on its own.

use parking_lot::RwLock;

/// Which language a translation direction listens for.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// first configured language -> second configured language
    FirstToSecond,
    /// second configured language -> first configured language
    SecondToFirst,
}

/// How a direction acquires audio and where its output goes.
///
/// Live is continuous VAD-gated listening with headphone output; PushToTalk is
/// button-triggered with phone-speaker output. See
/// `docs/adaptive-hybrid-mode-design.md` for why these can't share a state
/// machine.
///
/// `Off` disables the direction entirely (no listening, no output) — added
/// per `docs/adaptive-hybrid-mode-design.md` §1a rather than left implicit,
/// since "not started" and "explicitly off" would otherwise be two different
/// things for the pipeline (Phase 4) to reconcile, and retrofitting a third
/// enum variant after the UniFFI surface and settings UI both bind to a
/// two-value enum is more disruptive than adding it now.
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectionMode {
    Live,
    PushToTalk,
    Off,
}

#[derive(Debug, Clone)]
struct DirectionConfig {
    mode: DirectionMode,
    source_language_code: String,
    target_language_code: String,
}

/// Session-wide config, safe to share across the Java/Rust boundary as a
/// UniFFI object (interior mutability via `RwLock`, so `set_direction_mode`
/// can be called at any time without recreating the session).
#[derive(uniffi::Object)]
pub struct HybridConfig {
    first_to_second: RwLock<DirectionConfig>,
    second_to_first: RwLock<DirectionConfig>,
}

#[uniffi::export]
impl HybridConfig {
    #[uniffi::constructor]
    pub fn new(
        first_language_code: String,
        second_language_code: String,
        first_to_second_mode: DirectionMode,
        second_to_first_mode: DirectionMode,
    ) -> Self {
        Self {
            first_to_second: RwLock::new(DirectionConfig {
                mode: first_to_second_mode,
                source_language_code: first_language_code.clone(),
                target_language_code: second_language_code.clone(),
            }),
            second_to_first: RwLock::new(DirectionConfig {
                mode: second_to_first_mode,
                source_language_code: second_language_code,
                target_language_code: first_language_code,
            }),
        }
    }

    pub fn mode(&self, direction: Direction) -> DirectionMode {
        self.slot(direction).read().mode
    }

    pub fn set_mode(&self, direction: Direction, mode: DirectionMode) {
        self.slot(direction).write().mode = mode;
    }

    /// Language code this direction listens for (feeds ASR forced-decoding
    /// and, for the both-Live case, `langid::disambiguate`'s candidate list).
    pub fn source_language(&self, direction: Direction) -> String {
        self.slot(direction).read().source_language_code.clone()
    }

    /// Target language code this direction translates into.
    pub fn target_language(&self, direction: Direction) -> String {
        self.slot(direction).read().target_language_code.clone()
    }
}

impl HybridConfig {
    fn slot(&self, direction: Direction) -> &RwLock<DirectionConfig> {
        match direction {
            Direction::FirstToSecond => &self.first_to_second,
            Direction::SecondToFirst => &self.second_to_first,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_one_direction_does_not_affect_the_other() {
        let config = HybridConfig::new(
            "en".into(),
            "es".into(),
            DirectionMode::Live,
            DirectionMode::PushToTalk,
        );

        assert_eq!(config.mode(Direction::FirstToSecond), DirectionMode::Live);
        assert_eq!(
            config.mode(Direction::SecondToFirst),
            DirectionMode::PushToTalk
        );

        config.set_mode(Direction::FirstToSecond, DirectionMode::PushToTalk);

        assert_eq!(
            config.mode(Direction::FirstToSecond),
            DirectionMode::PushToTalk
        );
        assert_eq!(
            config.mode(Direction::SecondToFirst),
            DirectionMode::PushToTalk
        );

        config.set_mode(Direction::SecondToFirst, DirectionMode::Off);
        assert_eq!(config.mode(Direction::SecondToFirst), DirectionMode::Off);
        assert_eq!(
            config.mode(Direction::FirstToSecond),
            DirectionMode::PushToTalk,
            "switching a direction to Off must not affect the other direction"
        );
    }
}
