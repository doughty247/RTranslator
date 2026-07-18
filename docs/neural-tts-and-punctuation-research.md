# Neural TTS, RAM Gating, and Punctuation — Research Notes

Research backing the on-device neural TTS feature and the punctuation investigation
requested after the Solo Conversation mode work. Covers what was found and why it points at
`sherpa-onnx` specifically, plus a real (not speculative) answer to the punctuation
question that emerged from that same research.

## Neural TTS engine: `sherpa-onnx`, not raw Piper

**Piper** is the name most likely to come up first, but development moved to a
**GPL-3.0** fork (`OHF-Voice/piper1-gpl`) after the original `rhasspy/piper` repository was
archived in October 2025. Bundling GPL-licensed code into this project would add a real
new licensing complication on top of NLLB's existing non-commercial restriction — a
different *kind* of constraint (copyleft on the code itself), not something to take on
without deliberate sign-off.

**[`k2-fsa/sherpa-onnx`](https://github.com/k2-fsa/sherpa-onnx)** is the better fit:

- **Apache-2.0** licensed — matches this project and RTranslator's own license
- Wraps **ONNX Runtime** — the same inference runtime already bundled for Whisper/NLLB, no
  second runtime to add to the APK
- Supports multiple TTS model families through one Kotlin API (VITS/Piper voices, Matcha,
  Kokoro, Kitten, and others), each with a natural size/quality gradient — low tier
  (~20-30MB), medium (~60-80MB), high (~100MB+) — that maps directly onto a RAM-gated
  quality setting
- Ships prebuilt Android native libraries (`arm64-v8a` included, matching this app's
  `abiFilters`)
- Confirmed via a shallow clone of the upstream repo (`sherpa-onnx/kotlin-api/Tts.kt`) —
  see below for the exact API shape found

### Integration mechanics (confirmed from source, not guessed)

Unlike a typical Maven dependency, sherpa-onnx's Android integration vendors two things
directly into the app rather than pulling a library artifact:

1. **Native libraries** (`libsherpa-onnx-jni.so` per ABI) placed under
   `app/src/main/jniLibs/<abi>/`, loaded at runtime via `System.loadLibrary("sherpa-onnx-jni")`
   — the same pattern this app already uses for its own vendored native code
   (`app/src/main/cpp/`).
2. **Kotlin API source files** (`Tts.kt`, `OfflinePunctuation.kt`, etc., from
   `sherpa-onnx/kotlin-api/` upstream) copied directly into the app's source tree, since
   these are thin JNI wrapper classes, not a compiled library.

Both need to be fetched from a tagged sherpa-onnx release — this repo doesn't vendor them
sight-unseen; the developer needs to pull a specific release's Android artifacts (the
release asset naming wasn't independently verifiable from this sandbox, since GitHub's
release-download host wasn't reachable — confirm the current release's exact asset names
before wiring the Gradle/jniLibs setup).

### Kotlin API shape (from `sherpa-onnx/kotlin-api/Tts.kt`)

```kotlin
class OfflineTts(assetManager: AssetManager? = null, var config: OfflineTtsConfig) {
    fun generate(text: String, sid: Int = 0, speed: Float = 1.0f): GeneratedAudio
    // GeneratedAudio { val samples: FloatArray; val sampleRate: Int }
}
```

`OfflineTtsConfig` nests a model-family-specific config (`OfflineTtsVitsModelConfig` for
Piper-style voices, `OfflineTtsKokoroModelConfig`, etc.) — each just points at the
`.onnx` file(s) and a `tokens.txt` for that voice, all paths relative to a downloaded model
directory. This is a natural fit for this app's existing download-then-reference pattern
(`Downloader.java`/`DownloadFragment.java` already stage files into `Context.getFilesDir()`
the same way).

## Punctuation: a real answer, not a guess

The original ask was "add punctuation from Whisper." Code inspection (see the main
conversation record / commit history) found no punctuation-stripping logic anywhere in the
existing ASR/MT pipeline — `Recognizer.java`'s decode loop is unfiltered greedy argmax, and
neither `Tokenizer.java` nor `Translator.java` touch punctuation tokens. If punctuation is
genuinely missing in practice, the most likely cause is Whisper-Small's own inconsistent
punctuation prediction (a documented characteristic of smaller Whisper checkpoints,
especially outside English) — not a bug in this app's integration of it.

**sherpa-onnx ships an actual fix for exactly this**, independent of Whisper's own output
quality: `OfflinePunctuation` (`sherpa-onnx/kotlin-api/OfflinePunctuation.kt`), a
CT-Transformer-based punctuation restoration model:

```kotlin
class OfflinePunctuation(assetManager: AssetManager? = null, config: OfflinePunctuationConfig) {
    fun addPunctuation(text: String): String
}
```

This runs as a text-in/text-out post-processing step — feed it Whisper's raw transcript
(with or without punctuation already), get back punctuated text, *before* that text goes
into NLLB translation. Since this is the same runtime/library being added for TTS anyway,
this is close to free to include rather than a separate research/integration effort. This
supersedes the original plan of "wait for debug-log evidence before touching punctuation"
— there's now a concrete, low-risk thing to add regardless of exactly how bad Whisper's
own punctuation is, since it's strictly additive (only relevant to languages the
CT-Transformer model covers — this needs checking against RTranslator's supported language
list before assuming universal coverage).

## RAM gating

`Global.java` already has exactly the infrastructure needed —
`getTotalRamSize()` (total device RAM in MB) and `getAvailableRamSize()` — and an
established convention for using it: `Recognizer.java` gates an ONNX session optimization
at `7000` MB, and `NoticeFragment.java` (part of the app's first-run setup flow,
`AccessActivity`) already warns the user below `5000` MB. The README's own stated baseline
is "6GB+ RAM to run without crash risk."

Proposed threshold: gate medium/high-quality neural TTS tiers behind **8000 MB** total
RAM — a margin above the existing 7000 MB / 6GB baseline, since this is additional load on
top of an already RAM-intensive ASR+MT pipeline, not a replacement for anything. The
low-quality tier (~20-30MB models) can reasonably be offered at the same baseline the app
already requires to run at all, since its incremental RAM cost is small relative to
Whisper/NLLB's own footprint. These numbers are starting points, not measured — nothing in
this sandbox can profile actual peak RAM with Whisper+NLLB+sherpa-onnx all resident at
once; confirm on-device before finalizing.
