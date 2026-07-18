//! Rust-native language identification for the both-Live disambiguation case
//! (`docs/solo-conversation-mode-design.md` §5, Option B — chosen over reusing
//! Java's MLKit call specifically to keep the crate free of an Android/Java
//! dependency for its standalone-extraction goal).
//!
//! Uses `whatlang`, a pure-Rust trigram-frequency classifier with no external
//! model file to fetch or bundle — unlike MLKit (closed-source, Java-only) or
//! a fastText/lid.176-style `ort` model (would need its own weight file on
//! top of Whisper/NLLB), this needed nothing this sandbox couldn't already
//! provide, so unlike `asr.rs`/`mt.rs` it's fully exercised by
//! `tests/langid.rs` here, not just structurally written.
//!
//! Mirrors `Translator.java`'s `detectLanguage` shape: given forced-decode
//! ASR output for each of the two expected languages, decide which one the
//! speaker actually used. `whatlang::Detector::with_allowlist` restricted to
//! exactly the session's two configured languages is the direct analog of
//! MLKit's confidence-threshold call — asking "which of these two candidates
//! is it" rather than open-vocabulary detection across everything `whatlang`
//! knows.

use whatlang::{Detector, Lang};

/// `HybridConfig` stores language codes as ISO 639-1 (2-letter, e.g. "en",
/// "es") — the convention used across the rest of this crate's public API
/// and tests. `whatlang::Lang::from_code` expects ISO 639-3 (3-letter).
/// This table only needs to cover languages a real deployment would
/// configure; unrecognized codes return `None` and the caller falls back to
/// §2's ASR-confidence tiebreak, per the design doc — this is a best-effort
/// aid, not a required dependency for disambiguation to function at all.
fn iso639_1_to_lang(code: &str) -> Option<Lang> {
    let iso3 = match code.to_ascii_lowercase().as_str() {
        "en" => "eng",
        "es" => "spa",
        "fr" => "fra",
        "de" => "deu",
        "it" => "ita",
        "pt" => "por",
        "nl" => "nld",
        "ru" => "rus",
        "zh" => "cmn",
        "ja" => "jpn",
        "ko" => "kor",
        "ar" => "arb",
        "hi" => "hin",
        "pl" => "pol",
        "tr" => "tur",
        "vi" => "vie",
        _ => return None,
    };
    Lang::from_code(iso3)
}

/// Result of forcing a choice between exactly two candidate languages.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisambiguationResult {
    /// `true` if `text_a` was judged to be in `lang_a`; `false` if `text_b`
    /// was judged to be in `lang_b`. Ambiguous/failed cases return `None`
    /// from `disambiguate` entirely rather than a low-confidence guess here
    /// — see that function's docs.
    pub favors_a: bool,
    pub confidence: f64,
}

/// Given ASR output forced-decoded in each of two candidate languages, judge
/// which one the speaker actually used — the Rust-native replacement for
/// `Translator.java`'s `detectLanguage(first, second, ...)` two-text path.
///
/// Returns `None` when neither candidate's detected language matches its
/// expected language confidently (mirrors MLKit's `onFailure` /
/// `BOTH_RESULTS_FAIL` path in `WalkieTalkieService.compareResults()`) — the
/// caller (§2's disambiguation step in `pipeline.rs`) is expected to fall
/// back to ASR confidence-score comparison, then to the sticky-bias tiebreak
/// (§6), in that order, exactly as `WalkieTalkieService.compareResults()`
/// falls back to `compareResultsConfidence()` today.
pub fn disambiguate(
    text_a: &str,
    lang_a_code: &str,
    text_b: &str,
    lang_b_code: &str,
) -> Option<DisambiguationResult> {
    let lang_a = iso639_1_to_lang(lang_a_code)?;
    let lang_b = iso639_1_to_lang(lang_b_code)?;
    let detector = Detector::with_allowlist(vec![lang_a, lang_b]);

    let info_a = detector.detect(text_a);
    let info_b = detector.detect(text_b);

    let a_matches = info_a
        .as_ref()
        .is_some_and(|i| i.lang() == lang_a && i.is_reliable());
    let b_matches = info_b
        .as_ref()
        .is_some_and(|i| i.lang() == lang_b && i.is_reliable());

    match (a_matches, b_matches) {
        (true, false) => Some(DisambiguationResult {
            favors_a: true,
            confidence: info_a.unwrap().confidence(),
        }),
        (false, true) => Some(DisambiguationResult {
            favors_a: false,
            confidence: info_b.unwrap().confidence(),
        }),
        // Both matched or neither did: not a confident single answer either
        // way — let the caller fall back rather than guess here.
        (true, true) | (false, false) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_slots_matching_falls_back_rather_than_guessing() {
        // text_a is genuinely English, text_b is genuinely Spanish, matching
        // their respective candidate slots — this is the "clean win" shape
        // WalkieTalkieService.compareResults() calls case 2 (both detected
        // languages match their expected slot).
        let result = disambiguate(
            "Hello, how are you doing today? I hope everything is going well.",
            "en",
            "Hola, como estas hoy? Espero que todo vaya muy bien.",
            "es",
        );
        // Both slots match their expected language here, which per
        // WalkieTalkieService's own logic falls through to the confidence
        // tiebreak rather than whatlang alone — assert we get that documented
        // "ambiguous, fall back" outcome rather than silently picking one.
        assert_eq!(result, None);
    }

    #[test]
    fn resolves_when_only_one_slot_is_correct() {
        // text_a is Spanish sitting in the "en" slot (wrong), text_b is
        // genuinely Spanish in the "es" slot (right) — only b should match.
        let result = disambiguate(
            "Hola, como estas hoy? Espero que todo vaya muy bien.",
            "en",
            "Hola, como estas hoy? Espero que todo vaya muy bien.",
            "es",
        );
        let result = result.expect("exactly one slot should match its expected language");
        assert!(!result.favors_a);
    }

    #[test]
    fn unrecognized_language_code_falls_back_gracefully() {
        assert_eq!(disambiguate("hello", "xx", "hola", "es"), None);
    }
}
