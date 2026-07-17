//! Guards against a confirmed `ort` 2.0.0-rc.12 footgun: with the
//! `load-dynamic` feature (see `Cargo.toml`), calling
//! `ort::session::Session::builder()` before ONNX Runtime's shared library
//! can be resolved does not return an error — it hangs the calling thread
//! indefinitely on a futex wait, even when told to look at a path that
//! plainly doesn't exist. Confirmed with `strace` during Phase 2 (see
//! `tests/session_loading.rs`).
//!
//! `asr.rs` and `mt.rs` call `ensure_available()` before touching `ort` at
//! all, so a missing/misconfigured runtime library gets a clean
//! `HybridError` instead of hanging the app. Re-check whether this is still
//! necessary against whatever `ort` version Phase 4 actually ships with —
//! this may be fixed upstream by then.
//!
//! `dylib_path` is `None` for the desktop/CLI test harness (Phase 2), which
//! resolves the library via the `ORT_DYLIB_PATH` env var — the mechanism
//! `ort` itself honors when no explicit init call is made. On Android,
//! Phase 4 should instead pass the bundled `libonnxruntime.so` path
//! explicitly (e.g. derived from the app's native library directory, passed
//! in across the UniFFI boundary at session construction) and call
//! `ort::init_from(path).commit()` once at startup — env vars aren't a
//! natural fit for an Android process. This function accepts an explicit
//! path for that reason, so the same guard covers both cases.

use crate::error::HybridError;

pub fn ensure_available(dylib_path: Option<&str>) -> Result<(), HybridError> {
    let path = match dylib_path {
        Some(p) => p.to_string(),
        None => std::env::var("ORT_DYLIB_PATH").map_err(|_| {
            HybridError::OrtRuntimeUnavailable("ORT_DYLIB_PATH is not set".to_string())
        })?,
    };

    if std::path::Path::new(&path).is_file() {
        Ok(())
    } else {
        Err(HybridError::OrtRuntimeUnavailable(format!(
            "{path} does not point at a file"
        )))
    }
}
