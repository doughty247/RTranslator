# Model Artifact Contract

This document is the format contract the `adaptive-hybrid-core` Rust crate must match to
call the same Whisper (ASR) and NLLB (MT) ONNX models that RTranslator's existing Java
pipeline (`Recognizer.java`, `Translator.java`) already uses. It was reverse-engineered
from that Java pipeline plus `Sideloading.md` and `access/DownloadFragment.java`, since
there is no separate spec upstream. Line numbers refer to the RTranslator v2.x codebase
at the time of writing and may drift as upstream changes.

Reuse target (per `CLAUDE.md`): the **model files and this format contract**, not
RTranslator's Java call path. `adaptive-hybrid-core` loads these same `.onnx` files via
`ort` directly.

## 1. File inventory, source, and on-device location

Models are **not bundled in the APK**. They are downloaded on first launch (or sideloaded
per `Sideloading.md`) from the `2.0.0` GitHub release into
`Android/data/nie.translator.rtranslator/files/` (external app storage), then copied by
`DownloadFragment` into `Context.getFilesDir()` (internal app storage) — that internal
path is what `Recognizer`/`Translator` actually open at runtime.

| File | Approx. size | Role |
|---|---|---|
| `Whisper_initializer.onnx` | 69 KB | PCM → mel/`input_features` feature extraction |
| `Whisper_encoder.onnx` | 88 MB | Audio encoder → `encoder_hidden_states` |
| `Whisper_cache_initializer.onnx` | 14 MB | Cross-attention KV-cache init, batch=1 |
| `Whisper_cache_initializer_batch.onnx` | 14 MB | Same, batch=2 (WalkieTalkie dual-language mode only — not needed for Adaptive Hybrid Mode, which processes one direction's audio at a time) |
| `Whisper_decoder.onnx` | 173 MB | Autoregressive token decoder |
| `Whisper_detokenizer.onnx` | 461 KB | Token IDs → text (string-output graph; Whisper has its own baked-in vocab, no external tokenizer file) |
| `NLLB_encoder.onnx` | 254 MB | Source text encoder |
| `NLLB_cache_initializer.onnx` | 24 MB | Cross-attention KV-cache init |
| `NLLB_decoder.onnx` | 171 MB | Autoregressive token decoder |
| `NLLB_embed_and_lm_head.onnx` | 500 MB | Split-out embedding lookup + LM head (selected via a `use_lm_head` bool at call time) |

All ONNX files above are **pre-quantized int8** artifacts (per `README.md`); no runtime
quantization step exists in the app. `adaptive-hybrid-core` must treat them as opaque
int8 graphs and must NOT attempt to re-quantize or re-export them.

Tokenizer file (bundled in the APK, not downloaded):

| File | Location | Role |
|---|---|---|
| `sentencepiece_bpe.model` | `app/src/main/assets/sentencepiece_bpe.model` | Shared SentencePiece BPE/unigram vocab for NLLB, `DICTIONARY_LENGTH = 256000` |

For the Phase 2 desktop test harness, `adaptive-hybrid-core` reads `sentencepiece_bpe.model`
directly from this repo path (see `adaptive-hybrid-core/tests/`). On-device, the crate
should be pointed at the same copy the Java side already stages into `Context.getFilesDir()`
(no need to duplicate the copy-from-assets step — reuse the file Java already staged).

**Not available in this development environment:** the `.onnx` weight files themselves.
They are multi-hundred-MB downloads from `dl.google.com`-adjacent-scale hosts and this
sandbox's network policy does not allow fetching them (`dl.google.com` is blocked by the
outbound proxy; GitHub release *asset* downloads were not attempted but should be assumed
similarly constrained until verified). Phase 2's "confirm inference works standalone" was
only completable for the tokenizer path (see `adaptive-hybrid-core/README.md`); ASR/MT
tensor I/O below is documented from Java source reading, not empirically re-verified
against the actual weights from Rust. Verify this on the developer's machine or the
Pixel 9 Pro XL before relying on it in Phase 4.

## 2. ASR (Whisper) tensor contract — `Recognizer.java`

**Audio input:** mono PCM, 16 kHz, `float[]` in `[-1.0, 1.0]` (matches
`Recorder.SAMPLE_RATE_CANDIDATES = {16000}`, `ENCODING_PCM_FLOAT`). Wrapped as ONNX tensor
shape `[1, N]` under input name `audio_pcm`, fed to `Whisper_initializer.onnx`.

**Pipeline (single-utterance, batch=1 — the only batch size Adaptive Hybrid Mode needs;
ignore the batch=2 dual-language path, that's WalkieTalkie-specific):**

1. `Whisper_initializer.onnx`: `audio_pcm [1, N]` → `input_features` (mel spectrogram)
2. `Whisper_encoder.onnx`: `input_features` → `encoder_hidden_states`
3. `Whisper_cache_initializer.onnx`: `encoder_hidden_states` → `present.{i}.encoder.key` /
   `present.{i}.encoder.value` for `i` in `0..12` (12 decoder layers, head_dim 64, i.e.
   cache tensor shape family `[batch, 12, seq, 64]` per the initial zero-length
   self-attention cache built at `Recognizer.java:399`: `{batchSize, 12, 0, 64}`)
4. Iterative greedy decode loop, `Whisper_decoder.onnx`, called once per output token:
   - Inputs each step: current input token id, the **cross-attention** cache from step 3
     (`present.{i}.encoder.key/value` — computed once, reused unchanged every step) and the
     **self-attention** cache (`present.{i}.decoder.key/value` — grows by one position each
     step, fed from the *previous* step's decoder output)
   - Special token IDs: `START_TOKEN_ID = 50258`, language token =
     `START_TOKEN_ID + (index of language in the LANGUAGES array) + 1`,
     `TRANSCRIBE_TOKEN_ID = 50359`, `NO_TIMESTAMPS_TOKEN_ID = 50363`, `eos = 50257`
   - Decoding is greedy (argmax), not beam search, for the paths Adaptive Hybrid Mode cares
     about
   - Loop bounds: `MAX_TOKENS = 445`, and a runaway guard of `MAX_TOKENS_PER_SECOND = 30`
     relative to input audio duration
5. `Whisper_detokenizer.onnx`: final token ID sequence → text string directly (no
   SentencePiece involved for ASR output)

**This is the "KV-cache separation" the project brief refers to:** cross-attention cache
(from the encoder, constant per utterance) and self-attention cache (grows per decode step)
are **separate named tensor pairs** in the graph's input dict, not one combined blob. The
expensive cross-attention/encoder-context computation happens exactly once per utterance
regardless of how many tokens are decoded — `adaptive-hybrid-core`'s `asr.rs` must preserve
this shape (call the cache-initializer session once, then only mutate the self-attention
cache tensors across the decode loop) or it will silently re-pay that cost every token.

## 3. MT (NLLB) tensor contract — `Translator.java`

**Text input:** UTF-8 string, pre-tokenized via SentencePiece (see §4) with NLLB-specific ID
remapping (`Tokenizer.java`): SentencePiece ID → NLLB ID is `+1`, with a special-case remap
for raw SentencePiece IDs 1–3 (`Tokenizer.java:48-74` — read that method directly when
implementing `mt.rs`'s tokenizer glue, the remap table is small but exact). Input text is
pre-split into sentences via `BreakIterator` and re-batched up to a ~200-token budget before
being sent through the model (`Translator.java:454-480`) — `adaptive-hybrid-core` should
replicate this chunking or document why it diverges, since token-budget overflow behavior
of the model past ~200 tokens is unverified either way.

**Pipeline (greedy path only — beam search exists in `Translator.java` but is explicitly
noted upstream as broken: *"beam search is not included... because with this implementation
we have random crashes"*, `Translator.java:815`. Do not port the beam-search path.):**

1. `NLLB_encoder.onnx`: tokenized source ids → encoder hidden states
2. `NLLB_cache_initializer.onnx`: encoder hidden states → cross-attention cache
   (`present.{i}.encoder.key/value`), computed once per input text, same separation pattern
   as Whisper
3. `NLLB_embed_and_lm_head.onnx`, called twice per role via a `use_lm_head` boolean:
   - `use_lm_head=false`: embedding lookup for the next input token
   - `use_lm_head=true`: hidden state → vocab logits for the current step
4. Iterative greedy decode loop, `NLLB_decoder.onnx`: 12 layers, head_dim 64 (`nLayers=12,
   hiddenSize=64`, `Translator.java:646` — this is the NLLB config; MADLAD mode, which
   Adaptive Hybrid Mode does not need, uses 32 layers / hiddenSize 128 and must not be
   assumed interchangeable)
5. Detokenize via the same SentencePiece model used for input, reversing the ID remap from
   step 1

## 4. SentencePiece tokenizer contract

- Model file: `sentencepiece_bpe.model` (§1), vocab size 256000
- Java today calls into a JNI binding around vendored Google `sentencepiece` C++
  (`app/src/main/cpp/src/`). `adaptive-hybrid-core` instead uses the `sentencepiece` Rust
  crate (a binding to the same upstream C++ library) against the identical `.model` file —
  same vocab, same IDs, different binding, per `CLAUDE.md`'s reuse decision.
  `adaptive-hybrid-core/tests/tokenizer_roundtrip.rs` exercises this against the actual
  bundled asset file and is the one piece of this contract empirically verified in this
  environment (network-restricted, no ONNX weights available — see §1).
- NLLB ID remap (SentencePiece raw ID → model input ID) is a Java-side detail in
  `Tokenizer.java`, not part of the `.model` file itself — must be reimplemented in Rust
  from that source, not assumed to be "just add 1" without checking the 1–3 special case.
- Whisper does not use this tokenizer at all — its detokenization is baked into
  `Whisper_detokenizer.onnx` as a string-output graph (§2).

## 5. Confirmed footgun: `ort`'s `load-dynamic` hangs instead of erroring

Discovered while building the Phase 2 crate skeleton, not something to rediscover the hard
way in Phase 4: `adaptive-hybrid-core` uses `ort`'s `load-dynamic` feature (loads
`libonnxruntime.so` at runtime via `dlopen` rather than linking against it at build time —
necessary since this sandbox has no ONNX Runtime installed, and the natural fit for
reusing the app's already-bundled native library on Android). With `ort` 2.0.0-rc.12,
calling `ort::session::Session::builder()` before that shared library can be resolved does
**not** return an `Err` — it hangs the calling thread forever on a futex wait, confirmed via
`strace` even when `ORT_DYLIB_PATH` is explicitly set to a path that plainly doesn't exist.
`asr.rs`/`mt.rs`'s `load()` functions now call `runtime_guard::ensure_available()` first,
which checks the target path exists **before** touching `ort` at all, specifically to avoid
ever reaching that hang. Re-verify this is still necessary against whatever `ort` version
Phase 4 ships with (it's a release candidate; may be fixed upstream by then) — but don't
remove the guard without re-testing, a silent hang is a much worse failure mode on-device
than a clean error.

## 6. Open questions for Phase 4 (flag, do not guess)

- Exact ONNX Runtime execution provider RTranslator's Java side selects on-device (CPU-only
  vs NNAPI — `app/src/main/cpp/src/NNAPITest.cpp` exists in the vendored native tree and is
  worth checking before assuming CPU-only in `ort`'s Android build).
- Whether `ort`'s int8-quantized-op coverage matches what `ai.onnxruntime`'s Java API
  supports out of the box for these specific graphs — not verified here since the weight
  files aren't available in this sandbox.
- Real tensor shapes/names above are transcribed from Java source, not dumped from the
  `.onnx` files directly (e.g. via `onnx.checker` / Netron). Confirm with a direct model
  dump before Phase 4 implementation locks these in.
