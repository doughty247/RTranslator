//! Loads the real `sentencepiece_bpe.model` bundled in the Android app's
//! assets (not a copy — the same file `../app/src/main/assets/` ships) and
//! round-trips a string through it via the `sentencepiece` Rust crate. This
//! is the one piece of the Phase 2 "confirm inference works standalone"
//! goal that's actually verifiable in this sandbox: the `.onnx` weight files
//! aren't available here (see `docs/model-artifact-contract.md` §1), but the
//! tokenizer is bundled in-repo and needs no network access.

use sentencepiece::SentencePieceProcessor;

fn model_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../app/src/main/assets/sentencepiece_bpe.model")
}

#[test]
fn loads_the_bundled_nllb_vocab_and_round_trips_text() {
    let path = model_path();
    assert!(
        path.exists(),
        "expected the app's bundled tokenizer at {path:?} — did the asset move?"
    );

    let processor = SentencePieceProcessor::open(&path)
        .expect("sentencepiece crate should load the same .model file the Java JNI binding uses");

    assert_eq!(
        processor.len(),
        256_000,
        "DICTIONARY_LENGTH in Tokenizer.java is 256000 — vocab size mismatch means this isn't the model NLLB expects"
    );

    let pieces = processor
        .encode("Hello, how are you?")
        .expect("encoding should succeed on ordinary ASCII input");
    assert!(!pieces.is_empty());

    let ids: Vec<u32> = pieces.iter().map(|p| p.id).collect();
    let decoded = processor
        .decode_piece_ids(&ids)
        .expect("decoding the ids we just encoded should succeed");
    assert!(
        decoded.to_lowercase().contains("hello"),
        "round trip lost the original content: got {decoded:?}"
    );
}
