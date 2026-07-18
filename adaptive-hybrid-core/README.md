# adaptive-hybrid-core

The Rust core of RTranslator's Adaptive Hybrid Mode: per-direction, independently
switchable **live/ambient** (continuous VAD-gated listening) or **push-to-talk**
(button-triggered) speech translation, exposed to the app's Java/Kotlin shell via
[UniFFI](https://mozilla.github.io/uniffi-rs/).

This crate is written to be extractable into a standalone repository later. Its only
coupling to the rest of RTranslator is (a) the bundled Whisper/NLLB ONNX model files and
their tensor format, documented in [`../docs/model-artifact-contract.md`](../docs/model-artifact-contract.md),
and (b) the UniFFI boundary itself — it does not call into RTranslator's Java model
wrappers and has no other Java-side dependency.

## Status

The Phase 3 design doc (`../docs/adaptive-hybrid-mode-design.md`) is **signed off** — all
six open questions were decided by the developer and are implemented below. What's left is
Phase 5's real ASR/MT inference, not a design question. What's here:

- `config.rs` — per-direction (`FirstToSecond` / `SecondToFirst`) mode config
  (`Live`/`PushToTalk`/`Off`), safely mutable at runtime, plus source/target language
  codes per direction.
- `vad.rs` — real energy-based VAD with a three-phase echo-safety gate (Muted -> Elevated
  -> Normal after TTS playback stops) — the developer's hybrid pick over the design doc's
  Option A/B split. Deterministically unit-tested (audio-domain virtual clock, no real
  sleeps needed).
- `langid.rs` — Rust-native language disambiguation for the both-Live case, via `whatlang`
  (no external model file). Unit-tested against real text.
- `pipeline.rs` — `PipelineManager`: two independent per-direction loops (Live's VAD-gated
  buffer, PushToTalk's button-bracketed buffer), the both-Live ambiguous-utterance path,
  and `resolve_ambiguous` (langid -> ASR-confidence -> sticky-bias fallback chain). Unit
  tested, including the both-Live "one ambiguous event, not two" behavior.
- `asr.rs` / `mt.rs` — ONNX Runtime session **loading** (via `ort`) for the Whisper and
  NLLB model files, matching the file names and roles in the model artifact contract. The
  actual autoregressive decode loops are **not implemented** — this is the one remaining
  piece of real work, and it's specifically blocked on the real `.onnx` weight files this
  sandbox doesn't have (`../docs/model-artifact-contract.md` §1), not on any open design
  question.
- `ffi.rs` — the UniFFI-exported surface: session start/stop, undirected
  `push_audio_chunk` (Rust fans it out internally — see the design doc §3), push-to-talk
  bracketing, `notify_playback_window` (echo-safety), config get/set, and two callbacks
  (`TranslationListener` for translated text, optional `DebugListener` for VAD/echo-window
  field instrumentation). Fully wired to `pipeline.rs`; `resolve_utterance`'s placeholder
  text is the only remaining stub, standing in for the real ASR/MT call.
- `error.rs` — the shared error type crossing the boundary.

## Why no `.udl` file

`CLAUDE.md`'s original repo sketch listed `adaptive-hybrid-core.udl`. This crate instead
uses UniFFI's newer proc-macro API (`#[uniffi::export]`, `uniffi::setup_scaffolding!()`),
which is now the documented preferred approach and needs no separate interface-definition
file — the Rust source is the single source of truth for the FFI surface. Noting the
deviation here since the project plan named UDL explicitly.

## Building

```sh
cargo build                        # host build; the desktop/CLI test harness described in CLAUDE.md Phase 2
cargo test                         # unit tests + integration tests (see tests/, and caveats below)
bash tests/run_ffi_roundtrip.sh    # builds the cdylib, generates Python bindings, runs the FFI round trip
```

Regenerate the Kotlin bindings committed under `bindings/kotlin/` (copied into
`app/src/main/java/uniffi/adaptive_hybrid_core/` for the app to actually use — that copy
needs to be refreshed by hand after regenerating, this crate doesn't symlink or build
against the app module) with:

```sh
cargo build --lib
cargo run --bin uniffi-bindgen -- generate --library target/debug/libadaptive_hybrid_core.so \
    --language kotlin --out-dir bindings/kotlin
cp bindings/kotlin/uniffi/adaptive_hybrid_core/adaptive_hybrid_core.kt \
    ../app/src/main/java/uniffi/adaptive_hybrid_core/adaptive_hybrid_core.kt
```

### Building the Android native library

Cross-compiling for Android (`aarch64-linux-android`, matching the app's `arm64-v8a`
`abiFilters`) needs the Android NDK, which was not available in the environment this crate
was built in — **this step has never been run or verified**. Once the NDK is installed:

```sh
cargo install cargo-ndk   # one-time
rustup target add aarch64-linux-android   # one-time
cargo ndk -t arm64-v8a -o ../app/src/main/jniLibs build --release
```

This should produce `../app/src/main/jniLibs/arm64-v8a/libadaptive_hybrid_core.so` — the
generated Kotlin bindings load it via JNA (`Native.load("adaptive_hybrid_core", ...)`,
see `adaptive_hybrid_core.kt`'s `findLibraryName`), which resolves the `lib`/`.so`
naming convention automatically from that `jniLibs/<abi>/` layout, the same convention
Android's own `System.loadLibrary` uses. `ORT_DYLIB_PATH` (see the `ort` linking section
below) also needs to be set to wherever the app's existing ONNX Runtime `.so` lands at
runtime — likely something derived from `Context.getApplicationInfo().nativeLibraryDir`,
not a build-time constant, since that path is only known once the APK is installed.

### `ort` / ONNX Runtime linking

`ort` is configured with the `load-dynamic` feature, so this crate does **not** need
`libonnxruntime` present at build time (`cargo build` succeeds with no ONNX Runtime
installed). At **run** time, `ort::init_from(path)` (or the `ORT_DYLIB_PATH` env var) must
point at a `libonnxruntime.so` — on Android, the same shared library RTranslator's existing
Java/JNI path already bundles is the natural candidate to reuse, rather than adding a
second copy of ONNX Runtime to the APK; confirm that's viable before Phase 4.

**Known footgun (see `../docs/model-artifact-contract.md` §5):** if that library can't be
resolved, `ort` 2.0.0-rc.12's `load-dynamic` backend hangs instead of erroring — confirmed
via `strace`. `asr.rs`/`mt.rs` guard against this via `runtime_guard::ensure_available()`,
which must run before any `ort` call reaches `Session::builder()`. Don't remove that guard
without re-verifying the hang is fixed in whatever `ort` version is in use at the time.

### What's untested here vs. what needs the developer's machine / device

This crate was built and tested inside a network-restricted sandbox with **no physical
Android device and no access to the Whisper/NLLB `.onnx` weight files** (multi-hundred-MB
downloads from a host the environment's network policy blocks). Concretely:

- `src/vad.rs`, `src/langid.rs`, `src/pipeline.rs` unit tests — **actually run**, and
  actually exercise real logic (VAD phase transitions, language disambiguation against
  real sentences, sticky bias, the both-Live "one ambiguous event" fan-in). None of this
  needs the ONNX weight files, so unlike `asr.rs`/`mt.rs` it isn't just structurally
  written — it's verified.
- `tests/tokenizer_roundtrip.rs` — **actually run**, against the real
  `../app/src/main/assets/sentencepiece_bpe.model` file. This is the one part of "confirm
  inference works standalone" (`CLAUDE.md` Phase 2) that was verifiable here.
- `tests/session_loading.rs` — verifies `WhisperModel::load` / `NllbModel::load` fail with
  the expected, specific error when the model directory doesn't exist. It does **not**
  verify successful loading or correct tensor I/O against the real weights — that needs the
  actual `.onnx` files.
- `tests/ffi_roundtrip.py` (run via `tests/run_ffi_roundtrip.sh`) — exercises
  `HybridSession`'s full wired-up surface — lifecycle, config, undirected
  `push_audio_chunk` with VAD-gated (not per-chunk) translation callbacks, the
  playback-window echo-safety signal, push-to-talk bracketing, the both-Live "no
  premature guess" behavior, and the `DebugListener` callback — directly through the
  compiled `cdylib`'s UniFFI-generated Python bindings, not through Kotlin/Java — there was
  no Android SDK/NDK in this environment to run a JVM+Kotlin smoke test. This exercises the
  same generated extern "C" scaffolding Kotlin bindings would call through, so it's a
  meaningful proxy for "the boundary works," but it is not a substitute for an actual
  Java-side round trip on-device.
- Kotlin bindings generation (`uniffi-bindgen generate --language kotlin`) was run and its
  output committed under `bindings/kotlin/` for review, but has not been compiled or run
  from a real Android/Kotlin toolchain.

**Before wiring real ASR/MT into `asr.rs`/`mt.rs`'s decode loops, re-run against the real
model files (sideloaded per `../Sideloading.md`) and confirm on the Pixel 9 Pro XL.** The
pipeline/VAD/langid logic those decode loops plug into is already implemented and tested
above — this is the one remaining gap, not a design gap.

## Licensing

Apache-2.0, matching RTranslator (see `../LICENSE.txt`).

**NLLB is non-commercial-use-only.** This crate calls into NLLB model weights RTranslator
already ships; that restriction travels with the model files regardless of how this crate
itself is licensed or packaged (PR back upstream vs. standalone repo — see `CLAUDE.md`
Phase 6). Anyone reusing this code for commercial purposes needs their own NLLB-license-
compliant translation model.
