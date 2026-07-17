//! The `.onnx` weight files aren't available in this sandbox (network
//! policy blocks the download host — see `docs/model-artifact-contract.md`
//! §1), so this asserts the *documented* failure mode for a missing model
//! directory rather than successful inference. Re-run against a real
//! populated `Context.getFilesDir()`-equivalent directory (e.g. sideloaded
//! per `Sideloading.md`) before trusting the tensor I/O in `asr.rs`/`mt.rs`.

use adaptive_hybrid_core::asr::WhisperModel;
use adaptive_hybrid_core::mt::NllbModel;

#[test]
fn whisper_load_fails_cleanly_without_model_files() {
    let missing_dir = std::path::Path::new("/nonexistent/adaptive-hybrid-core-test");
    let err = match WhisperModel::load(missing_dir) {
        Ok(_) => panic!("no models exist at this path, load should have failed"),
        Err(e) => e,
    };
    assert!(format!("{err}").contains("Whisper_initializer.onnx"));
}

#[test]
fn nllb_load_fails_cleanly_without_model_files() {
    let missing_dir = std::path::Path::new("/nonexistent/adaptive-hybrid-core-test");
    let err = match NllbModel::load(missing_dir) {
        Ok(_) => panic!("no models exist at this path, load should have failed"),
        Err(e) => e,
    };
    assert!(format!("{err}").contains("NLLB_encoder.onnx"));
}
