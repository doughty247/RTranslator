# Echo-Safety Analysis

## What RTranslator does today: nothing, by design

A full-repo search for `AcousticEchoCanceler`, `NoiseSuppressor`, and `AudioFocus` returns
**zero hits** anywhere in `app/src`. `AudioManager` is used in exactly two files, and
neither use is related to echo cancellation or audio focus:

- `tools/BluetoothHeadsetUtils.java` — checks `isBluetoothScoAvailableOffCall()` /
  `isBluetoothScoOn()`, i.e. Bluetooth SCO connection state only.
- `voice_translation/neural_networks/voice/Recorder.java` — uses `AudioManager` only inside
  a `useBluetoothHeadset` branch to enumerate input devices and start/stop Bluetooth SCO
  (`startBluetoothSco()`/`stopBluetoothSco()`). WalkieTalkie mode constructs its `Recorder`
  with `useBluetoothHeadset = false` (`WalkieTalkieService.initializeVoiceRecorder()`), so
  it never even exercises this branch — it opens a plain `AudioSource.MIC` `AudioRecord`
  with no device routing and no focus request.

This confirms the `CLAUDE.md` brief's suspicion exactly: RTranslator has never needed echo
cancellation because it has never had a mode that plays TTS through the same output the mic
is also listening on. Instead, it sidesteps the problem entirely:

**`VoiceTranslationService.shouldDeactivateMicDuringTTS()` always returns `true`** in the
base class both WalkieTalkie and Conversation mode share. The mic is explicitly stopped
before TTS playback starts and restarted only after `TextToSpeech`'s `onDone()` callback
fires (with an extra ~500ms buffer, `VoiceTranslationService.java:131`). This is a
**half-duplex** design: listen, then speak, never both. It works because WalkieTalkie
always outputs to the phone's own speaker, at a volume and physical position where "mic off
during playback" is sufficient — there's no scenario where the user needs to hear something
else *while* the phone is talking.

## Why Adaptive Hybrid Mode can't reuse this

The live/ambient direction's entire premise breaks the assumption above: TTS output goes to
**Bluetooth headphones the mic may also be near or share signal path with** (depending on
headphone model — many consumer earbuds route sidetone or have imperfect isolation), and the
mode's whole value proposition is *not* pausing to listen — a live direction should be able
to pick up the next utterance shortly after speaking the last translation, not necessarily
strictly after playback completes plus a fixed buffer. Naively porting
`shouldDeactivateMicDuringTTS()`'s "always true, fixed 500ms tail" approach would work as a
crude first cut (see Phase 3 design doc's fallback option) but doesn't solve — it just
avoids — the actual echo problem, and reintroduces WalkieTalkie's push-to-talk-shaped
latency in a mode explicitly meant to feel ambient.

## What's available to actually solve it

- **`AcousticEchoCanceler`** (`android.media.audiofx.AcousticEchoCanceler`) — Java-only,
  no Rust/NDK equivalent. Must be enabled on the Java-side `AudioRecord` capture session
  (`AcousticEchoCanceler.create(audioSessionId)`), and its `isAvailable()` result is
  hardware/OEM-dependent — **this is explicitly listed as needing the physical Pixel 9 Pro
  XL to confirm** (Phase 4 device-in-the-loop item; not something this session can verify).
- Because AEC is Java-only, the Rust crate's VAD gating (`vad.rs`) cannot itself perform
  echo cancellation — the actual cancellation happens in the Java audio pipeline, upstream
  of whatever PCM buffers reach Rust. What Rust-side VAD *can* do is cooperate: avoid
  triggering a new listen-decode cycle on the residual echo tail AEC doesn't fully remove
  (AEC reduces but rarely eliminates echo, especially through Bluetooth's extra latency).
- That means a **coordination signal must cross the UniFFI boundary**: Java knows exactly
  when it started/stopped TTS playback (it owns the `TextToSpeech` calls); Rust's VAD needs
  that "playback window" to suppress false-positive VAD triggers during and briefly after
  TTS output, even with AEC active. This is a strict superset of the Phase 3 design doc's
  job to specify precisely (signal shape, timing, whether it's a hard mute vs. a VAD
  sensitivity adjustment) — flagged here as the concrete requirement this analysis produces,
  not resolved here.
- Bluetooth adds latency AEC's usual assumptions (tight loopback timing) may not hold for.
  This is the single highest-risk unknown the whole project has (per `CLAUDE.md`) and can
  only be resolved by testing with the developer's actual headphones on the actual device —
  not something to over-design for in Phase 3 without that data.

## Bottom line for Phase 3

Echo-safety in Adaptive Hybrid Mode is **not** a solved problem being ported from
WalkieTalkie — it's new work that must combine (a) Java-side `AcousticEchoCanceler` on the
capture session, (b) a Java→Rust playback-window signal so VAD gating cooperates with it,
and (c) empirical tuning against real Bluetooth hardware that can only happen on-device.
The Phase 3 design doc proposes the coordination signal's shape; this document's job was
narrower — confirm there is no existing code to lean on, and pin down exactly why not.
