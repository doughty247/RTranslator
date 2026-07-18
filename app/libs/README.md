# app/libs/

Local AAR/JAR dependencies that aren't published to a Maven repository, picked up by
`app/build.gradle`'s `fileTree(include: ['*.jar', '*.aar'], dir: 'libs')` dependency.

## sherpa-onnx.aar

Needed for the neural TTS feature (`docs/neural-tts-and-punctuation-research.md`). sherpa-onnx
publishes prebuilt Android artifacts as a release asset rather than a Maven coordinate.

1. Go to https://github.com/k2-fsa/sherpa-onnx/releases and find the current release's
   Android AAR asset (this repo's development environment couldn't reach GitHub's release
   download host to confirm the exact current filename — check the release notes for
   something like `sherpa-onnx-*-android.aar` or a `.tar.bz2` containing per-ABI
   `libsherpa-onnx-jni.so` files under `jniLibs/`).
2. Place the `.aar` here as `app/libs/sherpa-onnx.aar` (or, if it's a `.tar.bz2` of raw
   `.so` files instead of a packaged AAR, extract the `arm64-v8a/libsherpa-onnx-jni.so`
   into `app/src/main/jniLibs/arm64-v8a/` directly instead — this app only targets
   `arm64-v8a`, per `app/build.gradle`'s `abiFilters`).
3. Copy the Kotlin API wrapper source files this app's `tools/tts/` package depends on
   from `sherpa-onnx/kotlin-api/` in the sherpa-onnx repo (`Tts.kt`,
   `OfflinePunctuation.kt`) into `app/src/main/java/com/k2fsa/sherpa/onnx/` (the package
   name the upstream files declare — don't relocate them into RTranslator's own package,
   that just makes re-syncing a future sherpa-onnx update harder).

Neither the `.aar` nor the Kotlin API source files are committed to this repo — same
reasoning as why the Whisper/NLLB `.onnx` model files aren't (large, versioned upstream,
fetched at setup time rather than vendored).
