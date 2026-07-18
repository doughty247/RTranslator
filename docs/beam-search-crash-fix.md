# NLLB Beam Search Crash — Investigation and Fix

Upstream's own code comment on `Translator.java`'s `executeCacheDecoderBeam()` reads:

> for now beam search is not included (and not updated, so it won't work with the final
> models) because with this implementation we have random crashes

This document explains what was actually found reading that code, the fix applied, and —
importantly — what's still unverified.

## Root cause: an unvalidated hardcoded head count, silently corrupting memory

`Translator.java` constructs a `CacheContainerNative` (the native, zero-copy KV-cache
reordering helper used only by the beam-search path) with the attention head count
**hardcoded to `16`**, regardless of which model/mode is active:

```java
cacheContainer = new CacheContainerNative(onnxEnv, result, nLayers, beamSize, 16, j, hiddenSize);
```

`nLayers`/`hiddenSize` already vary by mode (`NLLB_CACHE`: 12 layers, head_dim 64;
`MADLAD_CACHE`: 32 layers, head_dim 128) — but the head count passed alongside them never
does. On the native side (`CacheContainerNative.cpp`), `insertData()` *does* check the
incoming tensor's real size against what that hardcoded head count predicts:

```cpp
int length = dim2*dim3*dim4*dim5;
if(length*4 != size){
    __android_log_print(ANDROID_LOG_ERROR, "BUFFER ERROR", "%s", "size of buffer different from expected");
}
cacheContainer[index] = bufferArr;   // stored regardless of whether the check just failed
```

That mismatch is **only logged** — the code stores the pointer and keeps going either way.
Every later `reorder()` call then indexes into that buffer using the wrong (hardcoded)
head count, computing offsets that don't match the tensor's real layout — reading and
writing outside the actual allocation. That's undefined behavior with exactly the
signature described: it can appear to work when the overrun lands in memory that happens
to be unused or gets overwritten harmlessly, and crash or corrupt unrelated state when it
doesn't — "random," not reliably reproducible from run to run.

For NLLB-600M-distilled specifically, 16 heads × 64 head_dim = 1024, which matches that
model's published `d_model` — so this hardcoded value is plausibly *correct* for
`NLLB_CACHE` mode. `MADLAD_CACHE` mode (a much larger, differently-shaped 3B-parameter
model, per `Translator.java`'s own `nLayers=32, hiddenSize=128`) was not verifiable against
a real model config in this environment, but a mismatch there is a very plausible
candidate for at least some of the reported crashes, precisely because nothing in the code
would have caught it if it were wrong.

## The fix

`CacheContainerNative.cpp`: stop trusting the caller's head count as ground truth. On the
first tensor inserted, derive the *actual* head count from that tensor's real buffer size
instead, and use the derived value for everything from then on (logged as a warning if it
disagreed with what Java passed in, so a real mismatch is visible rather than silent).
Every subsequent tensor is then checked against that *derived* shape — and if one of those
genuinely doesn't match, that's a real problem (not just an unverified guess), so it's now
a **thrown `IllegalStateException`** surfaced back to Java, rather than a log line and
silent continuation into corrupted native memory.

`Translator.java`: a second, smaller issue found while reading the same function —
`oldResult.close()` (releasing the previous decoder step's ONNX Runtime result) ran
*before* `cacheContainer` — which holds zero-copy native pointers directly into that same
result's tensor memory — got rotated out and closed later in the same loop iteration. That
left a window, every iteration, where the cache container referenced memory that had
already been freed. Nothing in this session's reading of the code found a concrete path
where that dangling window is actually *dereferenced* (the container's own `close()` never
touches the pointers, and `reorder()` only ever runs against the *current*, still-valid
result) — so this is defense-in-depth rather than a confirmed second crash cause. Fixed by
deferring `oldResult.close()` until after the cache container that depends on it has
already been closed, removing the fragile ordering regardless.

## What's still unverified

Everything here was found and fixed by reading `Translator.java` and
`CacheContainerNative.cpp`/`.java` directly — this environment has neither the actual NLLB
weight files nor an Android device, so **none of this has actually been run**. Specifically
unverified:

- Whether the real MADLAD-400 3B head count actually differs from 16 (the reasoning above
  is architectural inference, not a confirmed model config)
- Whether the head-count bug is the *only* cause of the reported crashes, or one of several
- Whether the fix's derived-head-count logic produces correct *translation output*, not
  just memory-safe execution — the crash and the correctness of beam search are separate
  questions; this fix addresses the crash, not the (separately unverified) quality of
  beam-search decoding itself

Before re-enabling beam search as a supported option (the app currently forces greedy
decoding, `TRANSLATOR_BEAM_SIZE = 1`, regardless of this fix), run it with a real beam size
> 1 on-device against the real models, ideally with `dim3Confirmed`'s warning log watched
for on the first run — if it fires, that confirms the original hardcoded "16" was in fact
wrong for whatever mode/model you tested.
