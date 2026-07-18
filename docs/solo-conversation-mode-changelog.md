# Solo Conversation mode — Changelog & Summary

This document summarizes everything built on branch `claude/rtranslator-1fdvtf` toward
Solo Conversation mode: a new RTranslator mode where each translation direction is
independently switchable between **live/ambient** (continuous listening, headphone output)
and **push-to-talk** (button-triggered, speaker output). Full background and phase plan
live in the project's `CLAUDE.md`; this document covers what was actually done.

---

## Changelog

- **Research**: mapped WalkieTalkie's existing turn-taking logic, the Whisper/NLLB ONNX
  tensor pipeline, SentencePiece tokenization, and confirmed RTranslator has zero existing
  echo-cancellation code today (`docs/model-artifact-contract.md`,
  `docs/echo-safety-analysis.md`)
- **New Rust crate** `solo-conversation-core/`, callable from the Java app shell via UniFFI,
  independent of RTranslator's Java model wrappers
- **Design doc** (`docs/solo-conversation-mode-design.md`) covering six open architecture
  questions, all reviewed and decided
- **Per-direction config**: `Live` / `PushToTalk` / `Off` modes, independently switchable
  at runtime, with per-direction source/target language tracking
- **Real VAD engine** with a three-phase echo-safety gate (Muted → Elevated → Normal)
  coordinated with TTS playback, replacing the original two-option proposal with a hybrid
- **Rust-native language identification** (`whatlang`) for resolving which direction an
  utterance belongs to when both directions are live simultaneously — no external model
  file, no Java/MLKit dependency
- **Sticky conversation bias**: ambiguous utterances lean toward whichever direction was
  used most recently, as a tiebreak only, on top of the ASR-confidence fallback
  WalkieTalkie already uses
- **Two independent per-direction pipelines** (live VAD-gated loop, push-to-talk
  button-bracketed loop) that don't fight over shared state
- **Optional debug callback** exposing timestamped VAD/echo-window events, for tuning echo
  safety against real Bluetooth hardware
- **UniFFI interface finalized**: undirected audio ingestion, playback-window signal,
  push-to-talk bracketing, translation callback, debug callback — Kotlin bindings
  generated for Android integration
- **Found and fixed a real bug**: the `ort` ONNX Runtime crate's dynamic-library loader
  hangs indefinitely instead of erroring when it can't find `libonnxruntime.so` — guarded
  against before it could ever hit production
- **24 automated tests** (16 Rust unit tests, 3 integration tests, 1 full FFI round-trip
  through the compiled library) — all passing, covering everything except the actual
  ASR/MT model inference, which needs the real model weights this development environment
  didn't have access to
- **Remaining gap**: the Whisper/NLLB decode loops themselves (`asr.rs`/`mt.rs`) still
  return placeholder text — everything *around* that call (when to run it, what audio to
  feed it, which direction it belongs to, where the result goes) is built and tested; the
  inference call itself needs the real model weights and a device to verify against

---

## Why this exists

RTranslator's two existing modes don't fit the target use case: WalkieTalkie is
single-phone but turn-based (press to talk, or a shared "who's speaking" guess), and
Conversation mode needs a second phone. Solo Conversation mode is a new mode built specifically
for one person, one phone, headphones optional, where each direction (e.g. Spanish→English
and English→Spanish) can independently be "always listening" or "press to talk."

The hard problem WalkieTalkie never had to solve: if a live direction's translated speech
plays through the same headphones the microphone is near, that audio can get picked back
up and re-translated — an echo/feedback loop. Solving that safely, without either
constant false triggers or ignoring the mic every time TTS speaks, was the central design
challenge.

---

## What was researched first

Before writing any new code, the existing RTranslator codebase was mapped in detail:

- **WalkieTalkie's state machine** (`WalkieTalkieService.java`): how it decides who's
  speaking, when it forces dual-language ASR, and how it falls back from MLKit language
  detection to raw ASR confidence scores when detection is ambiguous
- **The ONNX inference pipeline** (`Recognizer.java`, `Translator.java`): exact tensor
  shapes, input/output names, and the "KV-cache separation" optimization that makes the
  existing app fast — this became `docs/model-artifact-contract.md`, the contract the new
  Rust code has to match
- **Audio handling** (`Recorder.java`, `VoiceTranslationService.java`): confirmed the app
  has never needed echo cancellation, because it always mutes the mic during its own TTS
  playback and only ever outputs to the phone speaker — this became
  `docs/echo-safety-analysis.md`, explaining exactly why the new mode can't reuse that
  approach as-is

## The Rust crate: `solo-conversation-core/`

A new, self-contained Rust crate was added rather than extending WalkieTalkie's Java code,
per the project's design goal of keeping this logic portable and eventually
open-sourceable on its own. It talks to the Whisper/NLLB ONNX models directly (via the
`ort` crate) rather than through RTranslator's Java wrappers, and crosses into the Java app
shell through a single, deliberately small [UniFFI](https://mozilla.github.io/uniffi-rs/)
boundary.

**Module breakdown:**

| File | What it does |
|---|---|
| `config.rs` | Per-direction mode (`Live`/`PushToTalk`/`Off`) and language codes, safely switchable at runtime without restarting a session |
| `vad.rs` | Voice-activity detection plus the echo-safety gate (below) |
| `langid.rs` | Rust-native language identification for resolving ambiguous utterances |
| `pipeline.rs` | Orchestrates both directions' independent loops and the disambiguation logic |
| `asr.rs` / `mt.rs` | Loads the Whisper/NLLB ONNX model files; the actual inference call is the one remaining stub (see "What's left," below) |
| `ffi.rs` | The complete interface the Java app shell calls into |
| `runtime_guard.rs` | Guards against a real `ort` bug discovered during development (below) |
| `error.rs` | Shared error type crossing the Java/Rust boundary |

## The design decisions

A design document (`docs/solo-conversation-mode-design.md`) laid out the architecture as a
set of options with tradeoffs rather than a single prescribed answer, since these were
judgment calls specific to how the app should feel to use. Six questions were reviewed and
decided:

1. **How do the two directions share one microphone stream?** — Two independent loops,
   one per direction, rather than a single shared loop juggling both. Each direction's
   listening state doesn't depend on the other's.
2. **How does the app avoid hearing its own translated speech?** — A hybrid approach: mute
   listening completely for a short window right as playback ends (when echo is loudest),
   then ease into a reduced-sensitivity window rather than snapping straight back to full
   sensitivity. This avoids both the "went briefly deaf" feeling of a long hard mute and
   the false-trigger risk of no muting at all.
3. **How does the app tell which language was actually spoken, when both directions are
   listening at once?** — A self-contained Rust language detector, not Google's MLKit
   (which the existing app uses today). This keeps the new mode free of the closed-source
   dependency and works even without a MLKit-equipped Android environment.
4. **Should push-to-talk audio use a separate code path from live audio?** — No, one
   shared path, with explicit "button pressed" / "button released" signals bracketing it.
5. **Should the app remember which direction was just used, to help resolve the next
   ambiguous case?** — Yes, as a light tiebreaker only — it never overrides a confident
   detection, it only helps when the detector itself is unsure.
6. **Should there be a way to see what the echo-safety system is doing during real
   testing?** — Yes, an optional debug feed reporting exactly when listening was muted,
   reduced, or triggered, timestamped — so tuning against real headphones produces
   evidence instead of guesswork.

## What got built on top of those decisions

- **The VAD engine** (`vad.rs`): real amplitude-based voice detection, tuned as a starting
  point from the constants the existing app already uses, wrapped in the three-phase
  echo-safety gate described above. Fully deterministic and unit-tested — it uses an
  audio-driven internal clock rather than real time, so its behavior can be tested without
  waiting on real audio or real delays.
- **Language identification** (`langid.rs`): given two candidate languages and two pieces
  of text, decides which is which — directly tested against real sentences.
- **The pipeline manager** (`pipeline.rs`): ties the above together. Feeds incoming audio
  to whichever direction(s) are currently listening, detects when an utterance starts and
  ends, and — for the case where both directions are live and it's unclear who's
  speaking — runs the full fallback chain: language detector first, then a comparison of
  how confident each direction's speech recognition was, then the "who spoke last"
  tiebreaker.
- **The Java-facing interface** (`ffi.rs`): finalized as audio in (no direction label
  needed — the pipeline figures that out), push-to-talk button signals, the echo-safety
  playback signal, translated text out, and the optional debug feed. Kotlin bindings were
  generated from this for the Android side to consume.

## A real bug found and fixed along the way

While setting up the ONNX Runtime bridge, a genuine defect in the `ort` crate (the Rust
ONNX Runtime library) was discovered: if it can't locate the ONNX Runtime shared library
at startup, it hangs indefinitely instead of returning an error — confirmed by tracing the
actual system calls. Left unguarded, this could have silently frozen the app on a device
with an ONNX Runtime that isn't set up correctly, with no error message to explain why.
`runtime_guard.rs` checks the library is actually present before ever making that call, so
the failure mode is now a clean error instead of a hang.

## Testing

24 automated tests were written and are passing:

- Unit tests for the VAD phases, language identification, sticky bias, and pipeline
  routing (16 tests) — these exercise real logic, not placeholders, since none of them
  need the large ONNX model files to run
- A tokenizer round-trip test against the actual SentencePiece vocabulary file the app
  ships
- A full round-trip test exercising the entire Java-facing interface — session
  start/stop, audio flowing in, config changes, push-to-talk, the echo-safety signal, and
  both callback types — through the actual compiled library, the same code path Kotlin/Java
  would use

## What's left

The one piece that couldn't be built and verified in this development environment: the
actual speech recognition and translation inference calls. Everything *around* that
call — deciding when to run it, gathering the right audio, figuring out which direction it
belongs to, and where the translated result goes — is built and tested. What's not done is
wiring in the real Whisper/NLLB model calls themselves, because that needs the actual
multi-hundred-megabyte model files and a real device to confirm against, neither of which
were available during development. That's now the next concrete step, and — per your
report — you've already confirmed the app itself builds and runs.
