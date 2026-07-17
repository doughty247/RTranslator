//! The `.onnx` weight files aren't available in this sandbox (network
//! policy blocks the download host — see `docs/model-artifact-contract.md`
//! §1), and neither is `libonnxruntime.so` itself. That second gap matters
//! more than it sounds: `ort`'s `load-dynamic` backend (see `Cargo.toml`)
//! does not fail cleanly when it can't locate the runtime shared library —
//! it hangs indefinitely on a futex wait instead of returning an error, even
//! when told to look at a path that doesn't exist. Confirmed via `strace`
//! during Phase 2: the dlopen search exhausts every standard path, then the
//! calling thread blocks forever rather than surfacing the ENOENT.
//!
//! `WhisperModel::load`/`NllbModel::load` now call
//! `runtime_guard::ensure_available` first specifically to avoid ever
//! reaching that hang (see `src/runtime_guard.rs`), so both tests below
//! exercise the *documented* failure path (no dylib configured) rather than
//! risking the hang. Point them at a real `libonnxruntime.so` (e.g.
//! extracted from the app's existing native libs) to actually exercise
//! model loading end to end.

use adaptive_hybrid_core::asr::WhisperModel;
use adaptive_hybrid_core::mt::NllbModel;

#[test]
fn whisper_load_reports_missing_onnxruntime_instead_of_hanging() {
    let missing_dir = std::path::Path::new("/nonexistent/adaptive-hybrid-core-test");
    let err = match WhisperModel::load(missing_dir, Some("/nonexistent/libonnxruntime.so")) {
        Ok(_) => panic!("no onnxruntime dylib exists at this path, load should have failed"),
        Err(e) => e,
    };
    assert!(format!("{err}").contains("does not point at a file"));
}

#[test]
fn nllb_load_reports_missing_onnxruntime_instead_of_hanging() {
    let missing_dir = std::path::Path::new("/nonexistent/adaptive-hybrid-core-test");
    let err = match NllbModel::load(missing_dir, None) {
        Ok(_) => panic!("ORT_DYLIB_PATH is not set, load should have failed"),
        Err(e) => e,
    };
    assert!(format!("{err}").contains("ORT_DYLIB_PATH"));
}
