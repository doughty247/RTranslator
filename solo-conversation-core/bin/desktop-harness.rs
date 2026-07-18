//! Desktop CLI demo for `solo-conversation-core` (task: "start desktop
//! Linux/Windows/macOS support"). Exercises the exact same
//! `HybridSession::push_audio_chunk` surface Android's `AudioRecord` capture
//! loop calls through (`SoloConversationService.kt`), fed either from a real
//! microphone via `cpal` or from a WAV file.
//!
//! ```sh
//! cargo run --bin desktop-harness -- wav <path/to/file.wav> [--first en] [--second es]
//! cargo run --bin desktop-harness -- mic [--first en] [--second es]
//! ```
//!
//! **What's actually verified here vs. not:** `wav` mode needs no audio
//! hardware and *has* been run end-to-end in this sandbox against a
//! synthetic tone-burst WAV (see `tests/desktop_harness_wav.rs`), proving
//! `push_audio_chunk` -> `PipelineManager` -> VAD phase transitions ->
//! `DebugListener`/`TranslationListener` callbacks all work through a real
//! (if synthetic) audio signal, not just the pipeline unit tests. `mic` mode
//! is written against `cpal`'s documented API but genuinely **not run** --
//! this container has no `/dev/snd` and no ALSA card at all (confirmed via
//! `cat /proc/asound/cards`), so there is no input device here to test
//! against. Confirm `mic` mode on a real machine before relying on it.
//!
//! Translated text is still `ffi.rs`'s `resolve_utterance` placeholder
//! (`"[stub: N samples captured for -> <lang>]"`) -- this harness proves the
//! audio-in/event-out plumbing, not real ASR/MT output, which is still
//! blocked on real Whisper/NLLB weights per `docs/model-artifact-contract.md`.

use std::env;
use std::process::exit;
use std::sync::Arc;
use std::time::Duration;

use solo_conversation_core::config::{Direction, DirectionMode, HybridConfig};
use solo_conversation_core::ffi::{DebugListener, HybridSession, TranslationListener};
use solo_conversation_core::vad::GateEvent;

pub const SAMPLE_RATE: u32 = 16_000;
pub const CHUNK_MS: u32 = 100;
pub const CHUNK_SAMPLES: usize = (SAMPLE_RATE as usize * CHUNK_MS as usize) / 1000;

pub struct PrintingListener;

impl TranslationListener for PrintingListener {
    fn on_translated_text(&self, direction: Direction, text: String) {
        println!("[translated] {direction:?}: {text}");
    }
}

impl DebugListener for PrintingListener {
    fn on_debug_event(&self, direction: Direction, event: GateEvent, elapsed_ms: u64) {
        println!("[debug +{elapsed_ms}ms] {direction:?}: {event:?}");
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        print_usage();
        exit(1);
    }

    let (first, second) = parse_languages(&args);
    let session = build_session(&first, &second);

    match args[1].as_str() {
        "wav" => {
            let path = match args.get(2) {
                Some(p) => p.clone(),
                None => {
                    eprintln!("wav mode needs a file path: desktop-harness wav <path.wav>");
                    exit(1);
                }
            };
            run_wav(&session, &path);
        }
        "mic" => run_mic(&session),
        _ => {
            print_usage();
            exit(1);
        }
    }
}

fn print_usage() {
    eprintln!(
        "Usage:\n  \
         desktop-harness wav <path.wav> [--first en] [--second es]\n  \
         desktop-harness mic [--first en] [--second es]"
    );
}

fn parse_languages(args: &[String]) -> (String, String) {
    let mut first = "en".to_string();
    let mut second = "es".to_string();
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--first" if i + 1 < args.len() => {
                first = args[i + 1].clone();
                i += 2;
            }
            "--second" if i + 1 < args.len() => {
                second = args[i + 1].clone();
                i += 2;
            }
            _ => i += 1,
        }
    }
    (first, second)
}

/// FirstToSecond is Live, SecondToFirst is Off -- deliberately not both
/// Live. This harness has one mic feed, and `pipeline.rs`'s both-Live path
/// treats simultaneous Live directions as *one ambiguous speaker* needing
/// language disambiguation (`AmbiguousUtteranceReady`), which `ffi.rs`
/// intentionally does not forward to `TranslationListener` until Phase 5's
/// real ASR can disambiguate it (see `ffi.rs`'s `handle_pipeline_event`).
/// Two-way disambiguation is Android's scenario (two people, one phone);
/// this single-direction default is what actually demonstrates
/// `push_audio_chunk` -> VAD -> `UtteranceReady` -> translated-text callback
/// end to end. Use `--first`/`--second` to pick the language pair.
fn build_session(first: &str, second: &str) -> Arc<HybridSession> {
    let config = Arc::new(HybridConfig::new(
        first.to_string(),
        second.to_string(),
        DirectionMode::Live,
        DirectionMode::Off,
    ));
    let session = Arc::new(HybridSession::new(config));
    let listener = Arc::new(PrintingListener);
    session.set_listener(listener.clone());
    session.set_debug_listener(listener);
    session
        .start()
        .expect("a freshly constructed session should always start cleanly");
    println!("Session started: {first} -> {second} (Live), {second} -> {first} (Off).");
    session
}

/// Reads a WAV file of any channel count/sample rate/sample format, converts
/// to mono `f32` `[-1.0, 1.0]` at 16kHz (this crate's PCM convention, see
/// `docs/model-artifact-contract.md` §2), and pushes it through the session
/// in `CHUNK_MS` chunks, followed by enough trailing silence to flush a
/// final in-progress utterance past `VadConfig::speech_timeout_ms`.
pub fn run_wav(session: &HybridSession, path: &str) {
    println!("Reading {path} ...");
    let mut reader = hound::WavReader::open(path)
        .unwrap_or_else(|e| panic!("failed to open {path}: {e}"));
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .map(|s| s.expect("wav sample read failed"))
            .collect(),
        hound::SampleFormat::Int => {
            let max_value = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.expect("wav sample read failed") as f32 / max_value)
                .collect()
        }
    };
    println!(
        "  {} samples, {} channel(s), {}Hz, {:?}",
        samples.len(),
        spec.channels,
        spec.sample_rate,
        spec.sample_format
    );

    let mono16k = resample_to_mono_16k(&samples, spec.channels, spec.sample_rate);
    println!(
        "Feeding {:.2}s of audio through the pipeline in {CHUNK_MS}ms chunks...",
        mono16k.len() as f32 / SAMPLE_RATE as f32
    );

    for chunk in mono16k.chunks(CHUNK_SAMPLES) {
        session
            .push_audio_chunk(chunk.to_vec(), CHUNK_MS)
            .expect("push_audio_chunk failed");
    }

    // Trailing silence so a still-open utterance's speech_timeout_ms
    // (1300ms by default) has a chance to fire before the harness exits --
    // otherwise a WAV that ends mid-speech never emits its final
    // UtteranceEnded/UtteranceReady event.
    let silence = vec![0.0f32; CHUNK_SAMPLES];
    let trailing_chunks = 2000 / CHUNK_MS as usize + 1;
    for _ in 0..trailing_chunks {
        session
            .push_audio_chunk(silence.clone(), CHUNK_MS)
            .expect("push_audio_chunk failed");
    }

    println!("Done.");
}

/// Live microphone capture via `cpal` -- see this file's module doc for why
/// this path is written but not run in this sandbox (no audio hardware).
pub fn run_mic(session: &Arc<HybridSession>) {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let host = cpal::default_host();
    let device = match host.default_input_device() {
        Some(d) => d,
        None => {
            eprintln!(
                "No input audio device found on this host. `mic` mode needs real audio \
                 hardware; use `wav <file>` mode instead, which needs none and is what this \
                 crate's own testing in this sandbox actually used (see this file's module \
                 doc)."
            );
            exit(1);
        }
    };
    let input_config = match device.default_input_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "Could not get an input config from the default device ({e}). This usually \
                 means the host has no real audio hardware (ALSA on Linux still reports a \
                 \"default\" device object even with nothing behind it) rather than a bug in \
                 this harness. Use `wav <file>` mode instead."
            );
            exit(1);
        }
    };
    println!(
        "Input device: {} ({} ch, {}Hz, {:?})",
        device.name().unwrap_or_else(|_| "<unknown>".to_string()),
        input_config.channels(),
        input_config.sample_rate().0,
        input_config.sample_format()
    );

    let channels = input_config.channels();
    let sample_rate = input_config.sample_rate().0;
    let err_fn = |err| eprintln!("audio stream error: {err}");

    let stream = match input_config.sample_format() {
        cpal::SampleFormat::F32 => {
            let session = session.clone();
            let mut pending: Vec<f32> = Vec::new();
            device.build_input_stream(
                &input_config.into(),
                move |data: &[f32], _| {
                    let mono16k = resample_to_mono_16k(data, channels, sample_rate);
                    feed_pending(&session, &mut pending, &mono16k);
                },
                err_fn,
                None,
            )
        }
        cpal::SampleFormat::I16 => {
            let session = session.clone();
            let mut pending: Vec<f32> = Vec::new();
            device.build_input_stream(
                &input_config.into(),
                move |data: &[i16], _| {
                    let floats: Vec<f32> = data.iter().map(|s| *s as f32 / i16::MAX as f32).collect();
                    let mono16k = resample_to_mono_16k(&floats, channels, sample_rate);
                    feed_pending(&session, &mut pending, &mono16k);
                },
                err_fn,
                None,
            )
        }
        other => {
            eprintln!("Unsupported input sample format: {other:?} (only F32/I16 handled here)");
            exit(1);
        }
    }
    .expect("failed to build input stream");

    stream.play().expect("failed to start audio stream");
    println!("Listening... (Ctrl+C to stop)");
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

fn feed_pending(session: &HybridSession, pending: &mut Vec<f32>, new_samples: &[f32]) {
    pending.extend_from_slice(new_samples);
    while pending.len() >= CHUNK_SAMPLES {
        let chunk: Vec<f32> = pending.drain(..CHUNK_SAMPLES).collect();
        if let Err(e) = session.push_audio_chunk(chunk, CHUNK_MS) {
            eprintln!("push_audio_chunk failed: {e:?}");
        }
    }
}

/// Averages down to mono, then linearly resamples to `SAMPLE_RATE`. This is
/// a naive resampler (no anti-aliasing filter) -- adequate for a demo
/// harness talking to an energy-based VAD, not audio-quality-sensitive
/// production code; a real product build should use a proper resampler
/// (e.g. `rubato`) if the input device's native rate isn't already 16kHz.
pub fn resample_to_mono_16k(samples: &[f32], channels: u16, input_rate: u32) -> Vec<f32> {
    let mono: Vec<f32> = if channels <= 1 {
        samples.to_vec()
    } else {
        samples
            .chunks(channels as usize)
            .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
            .collect()
    };

    if input_rate == SAMPLE_RATE || mono.is_empty() {
        return mono;
    }

    let ratio = input_rate as f64 / SAMPLE_RATE as f64;
    let out_len = ((mono.len() as f64) / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = i as f64 * ratio;
        let idx0 = src_pos.floor() as usize;
        let idx1 = (idx0 + 1).min(mono.len() - 1);
        let frac = (src_pos - idx0 as f64) as f32;
        out.push(mono[idx0] + (mono[idx1] - mono[idx0]) * frac);
    }
    out
}
