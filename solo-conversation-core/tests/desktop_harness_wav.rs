//! Runs the actual compiled `desktop-harness` binary against a synthetic
//! WAV file, end to end -- this is the live verification for the desktop
//! (Linux/Windows/macOS) audio path that `bin/desktop-harness.rs`'s module
//! doc refers to. Unlike the `pipeline.rs`/`vad.rs` unit tests, this
//! exercises the actual binary a developer would run (`cargo run --bin
//! desktop-harness -- wav ...`), including argument parsing, WAV decoding,
//! resampling, and the real `HybridSession` FFI surface, via
//! `CARGO_BIN_EXE_desktop-harness` (Cargo builds the binary before running
//! this test and exposes its path through that env var automatically for
//! any integration test in the same package).

use std::f32::consts::PI;
use std::process::Command;

const SAMPLE_RATE: u32 = 16_000;

/// Silence, then a tone loud enough and long enough to cross `VadConfig`'s
/// default `amplitude_threshold` (0.061) and `min_voice_ms` (250ms), then
/// enough trailing silence to clear `speech_timeout_ms` (1300ms) so the
/// utterance actually closes before the harness exits.
fn write_test_wav(path: &std::path::Path) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("failed to create test wav");

    let lead_in_silence = (SAMPLE_RATE as f32 * 0.5) as usize; // 500ms
    for _ in 0..lead_in_silence {
        writer.write_sample(0i16).unwrap();
    }

    let tone_samples = (SAMPLE_RATE as f32 * 0.8) as usize; // 800ms > min_voice_ms
    let freq = 220.0_f32;
    let amplitude = 0.3 * i16::MAX as f32; // amplitude 0.3 >> 0.061 threshold
    for n in 0..tone_samples {
        let t = n as f32 / SAMPLE_RATE as f32;
        let sample = (amplitude * (2.0 * PI * freq * t).sin()) as i16;
        writer.write_sample(sample).unwrap();
    }

    let trailing_silence = (SAMPLE_RATE as f32 * 2.0) as usize; // 2000ms > speech_timeout_ms
    for _ in 0..trailing_silence {
        writer.write_sample(0i16).unwrap();
    }

    writer.finalize().expect("failed to finalize test wav");
}

#[test]
fn wav_mode_drives_a_full_utterance_cycle_through_the_real_binary() {
    let wav_path = std::env::temp_dir().join("solo_conversation_core_harness_test.wav");
    write_test_wav(&wav_path);

    let output = Command::new(env!("CARGO_BIN_EXE_desktop-harness"))
        .arg("wav")
        .arg(&wav_path)
        .arg("--first")
        .arg("en")
        .arg("--second")
        .arg("es")
        .output()
        .expect("failed to run desktop-harness binary");

    let _ = std::fs::remove_file(&wav_path);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "harness exited non-zero.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    assert!(
        stdout.contains("UtteranceStarted"),
        "expected a VAD UtteranceStarted debug event, got:\n{stdout}"
    );
    assert!(
        stdout.contains("UtteranceEnded"),
        "expected a VAD UtteranceEnded debug event, got:\n{stdout}"
    );
    assert!(
        stdout.contains("[translated] FirstToSecond:"),
        "expected a translated-text callback for the Live direction, got:\n{stdout}"
    );
    assert!(
        stdout.contains("stub:"),
        "expected ffi.rs's still-stubbed placeholder translation text, got:\n{stdout}"
    );
}
