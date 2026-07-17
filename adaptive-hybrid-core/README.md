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

This is a **Phase 2 skeleton**, not a working translation pipeline yet. What's here:

- `config.rs` — per-direction (`FirstToSecond` / `SecondToFirst`) live-vs-push-to-talk
  config, safely mutable at runtime.
- `asr.rs` / `mt.rs` — ONNX Runtime session loading (via `ort`) for the Whisper and NLLB
  model files, matching the file names and roles in the model artifact contract. The
  actual decode loops are **not implemented** — that's Phase 4/5 work, gated on the Phase 3
  design doc being reviewed and signed off by the developer (see repo root `CLAUDE.md`).
- `vad.rs` / `pipeline.rs` — intentionally empty. The live-direction/push-to-talk state
  machine and VAD gating are the project's core design contribution and are not something
  this session should guess at ahead of that review.
- `ffi.rs` — the UniFFI-exported surface: session start/stop, `push_audio_chunk`, config
  get/set, and a `TranslationListener` callback. `push_audio_chunk` is currently a stub — it
  proves data flows across the boundary in both directions (see `tests/`) but does not run
  real inference.
- `error.rs` — the shared error type crossing the boundary.

## Why no `.udl` file

`CLAUDE.md`'s original repo sketch listed `adaptive-hybrid-core.udl`. This crate instead
uses UniFFI's newer proc-macro API (`#[uniffi::export]`, `uniffi::setup_scaffolding!()`),
which is now the documented preferred approach and needs no separate interface-definition
file — the Rust source is the single source of truth for the FFI surface. Noting the
deviation here since the project plan named UDL explicitly.

## Building

```sh
cargo build          # host build; used for the desktop/CLI test harness described in CLAUDE.md Phase 2
cargo test           # unit tests + integration tests (see tests/, and caveats below)
```

Cross-compiling for Android (`aarch64-linux-android`, matching the app's `arm64-v8a`
`abiFilters`) needs the Android NDK, which was not available in the environment this
skeleton was built in — that step is unverified and should be set up and documented before
Phase 4 implementation begins in earnest.

### `ort` / ONNX Runtime linking

`ort` is configured with the `load-dynamic` feature, so this crate does **not** need
`libonnxruntime` present at build time (`cargo build` succeeds with no ONNX Runtime
installed). At **run** time, `ort::init_from(path)` (or the `ORT_DYLIB_PATH` env var) must
point at a `libonnxruntime.so` — on Android, the same shared library RTranslator's existing
Java/JNI path already bundles is the natural candidate to reuse, rather than adding a
second copy of ONNX Runtime to the APK; confirm that's viable before Phase 4.

### What's untested here vs. what needs the developer's machine / device

This crate was built and tested inside a network-restricted sandbox with **no physical
Android device and no access to the Whisper/NLLB `.onnx` weight files** (multi-hundred-MB
downloads from a host the environment's network policy blocks). Concretely:

- `tests/tokenizer_roundtrip.rs` — **actually run**, against the real
  `../app/src/main/assets/sentencepiece_bpe.model` file. This is the one part of "confirm
  inference works standalone" (`CLAUDE.md` Phase 2) that was verifiable here.
- `tests/session_loading.rs` — verifies `WhisperModel::load` / `NllbModel::load` fail with
  the expected, specific error when the model directory doesn't exist. It does **not**
  verify successful loading or correct tensor I/O against the real weights — that needs the
  actual `.onnx` files.
- `tests/ffi_roundtrip.rs` — exercises `HybridSession`'s lifecycle, config, and the
  `TranslationListener` callback directly through the compiled `cdylib`'s UniFFI-generated
  Python bindings (via `uniffi-bindgen generate --language python`), not through
  Kotlin/Java — there was no Android SDK/NDK in this environment to run a JVM+Kotlin
  smoke test. This exercises the same generated extern "C" scaffolding Kotlin bindings
  would call through, so it's a meaningful proxy for "the boundary works," but it is not a
  substitute for an actual Java-side round trip on-device.
- Kotlin bindings generation (`uniffi-bindgen generate --language kotlin`) was run and its
  output committed under `bindings/kotlin/` for review, but has not been compiled or run
  from a real Android/Kotlin toolchain.

**Before Phase 4 implementation locks in the tensor I/O in `asr.rs`/`mt.rs`, re-run against
the real model files (sideloaded per `../Sideloading.md`) and confirm on the Pixel 9 Pro
XL.**

## Licensing

Apache-2.0, matching RTranslator (see `../LICENSE.txt`).

**NLLB is non-commercial-use-only.** This crate calls into NLLB model weights RTranslator
already ships; that restriction travels with the model files regardless of how this crate
itself is licensed or packaged (PR back upstream vs. standalone repo — see `CLAUDE.md`
Phase 6). Anyone reusing this code for commercial purposes needs their own NLLB-license-
compliant translation model.
