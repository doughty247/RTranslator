# Adaptive Hybrid Mode — Design Doc (Phase 3, draft for developer sign-off)

**Status: DRAFT. Do not begin Phase 4/5 implementation of the sections below marked
"decision needed" until the developer has picked an option.** Per `CLAUDE.md`, this is
the developer's own design contribution — this doc lays out options and tradeoffs, it
does not unilaterally pick an architecture. `adaptive-hybrid-core/src/config.rs` and
`ffi.rs` already implement §1 and part of §3 below as working Phase 2 code, since those
two were low-risk enough to prototype directly; §2 (state machine) and §4 (echo-safety
signal) are genuinely open and this doc proposes rather than implements them.

## 1. Per-direction configuration model

**Decision made (low-risk, already implemented):** config lives in Rust, exposed via
UniFFI getters/setters, per `CLAUDE.md`'s stated lean ("to keep the crate self-sufficient
for the standalone-extraction goal"). See `config.rs`:

- `Direction` — `FirstToSecond` / `SecondToFirst`, i.e. which of the two configured
  languages is the source for this direction.
- `DirectionMode` — `Live` / `PushToTalk`.
- `HybridConfig` — a UniFFI `Object` (interior-mutable via `RwLock`) holding both
  directions' mode and target language independently. `set_mode()` can be called at any
  time without recreating the session; switching one direction never touches the other
  (unit-tested in `config.rs`).

This part doesn't need developer sign-off — it's a direct implementation of what
`CLAUDE.md` §Phase 3.1 already specified. Flagging it here only so the full config
surface is visible in one place before §2 depends on it.

## 2. State machine — DECISION NEEDED

The hard part: a live direction is a continuous loop (VAD trigger → ASR → MT → emit →
keep listening), a push-to-talk direction is a discrete one (button down → capture →
button up → ASR → MT → emit), and both directions share one physical microphone / one
Java-side `AudioRecord` session. `CLAUDE.md` is explicit that this must not be forced
into WalkieTalkie's turn-taking state machine.

### The actual conflict to resolve

Both directions listen for *different source languages*, but only one `AudioRecord`
capture stream exists. Two sub-questions:

**(a) How does incoming audio get attributed to a direction before ASR runs?**
Unlike WalkieTalkie (which always runs *both* languages' forced-decode and picks a winner
after the fact, per the research into `WalkieTalkieService.compareResults()`), Adaptive
Hybrid Mode's two directions may be in different modes simultaneously — e.g. direction A
live (always listening) while direction B is push-to-talk (only listening while the
button is held). This is actually simpler than WalkieTalkie's problem in the mixed-mode
case, because push-to-talk's button press *is* the attribution signal — no language ID
needed for that stream. The only case that still needs disambiguation is **both
directions configured Live simultaneously** (ambient in both languages at once), which
degenerates to WalkieTalkie's original problem (whoever's talking, in which language).

**(b) When push-to-talk is held down, does the Live direction's VAD stay armed?**
If both are armed, a PTT utterance could double-fire through the Live direction's VAD
too. If Live is suspended during PTT capture, that's a coordination signal similar in
shape to the TTS-playback-window signal from §4 (below) — the button-hold state is
another thing Rust-side gating needs to know about, alongside "TTS is playing."

### Option A — Two independent per-direction loops, audio fan-out (recommended)

Each direction gets its own internal loop object in `pipeline.rs` (`LiveLoop`,
`PushToTalkLoop`), each with its own VAD/capture-buffer state. Java pushes every raw PCM
chunk into `HybridSession::push_audio_chunk()` once; Rust internally fans that chunk out
to whichever loop(s) are currently "armed" (a Live direction is always armed unless
suppressed per §4; a PushToTalk direction is armed only between button-down and
button-up events, which also cross the boundary as explicit calls, e.g.
`begin_push_to_talk(direction)` / `end_push_to_talk(direction)`).

- If both directions are Live simultaneously: both loops are armed on every chunk, and
  each independently runs ASR forced to its own expected source language, similar to
  WalkieTalkie's dual-decode-then-compare, reusing the same MLKit-or-not decision from
  §5 to resolve which direction the utterance actually belongs to.
- If one is Live and the other PushToTalk: no ambiguity, each chunk only reaches the
  loop(s) currently armed by construction.
- If both are PushToTalk: only one can physically be "held" at a time given one button
  per direction on a phone UI (or the UI enforces mutual exclusion) — Java's problem, not
  Rust's.

**Tradeoff:** clean separation, easy to reason about per-direction, but the
both-Live-simultaneously case still needs the compare-and-disambiguate logic — this
doesn't eliminate that problem, it just scopes it to one case instead of being the
default for every case (which is the actual improvement over WalkieTalkie).

### Option B — Single shared loop with a mode-per-direction dispatch table

One loop, one VAD instance; on each VAD-triggered utterance boundary, check both
directions' configured mode and dispatch accordingly (run ASR for whichever
direction(s) are "listening" at that instant, same disambiguation as Option A's
both-Live case, always). Push-to-talk's button-down still needs to bypass VAD gating
entirely (immediate capture start, not waiting for energy threshold).

**Tradeoff:** less code (one state object instead of two loop types), but blurs the
"independent, not fighting over shared state" goal `CLAUDE.md` explicitly asks for —
Option A's separation more directly satisfies that requirement. Recommend **Option A**
unless the developer has a reason to prefer the simpler single-loop shape.

### What this doc is NOT deciding

Exact VAD algorithm (energy-threshold like `Recorder.java`'s
`DEFAULT_AMPLITUDE_THRESHOLD`/`DEFAULT_SPEECH_TIMEOUT_MILLIS`, vs. something more robust
like WebRTC VAD) is left to `vad.rs`'s Phase 4 implementation — not architecturally
significant enough to block sign-off on this doc, but worth the developer's explicit
call before Phase 4 starts since it affects both loop types.

## 3. UniFFI interface surface

Implemented in `ffi.rs` as a first pass; kept intentionally minimal per `CLAUDE.md`
("a small, stable UniFFI interface is what makes this portable later"):

| Crossing the boundary | Direction | Notes |
|---|---|---|
| `HybridConfig::new(...)` | Java → Rust | language codes + initial per-direction modes |
| `HybridConfig::mode/set_mode` | both | runtime mode switch, §1 |
| `HybridSession::new(config)` | Java → Rust | one session per active pair of languages |
| `HybridSession::set_listener` | Java → Rust | registers the callback below |
| `HybridSession::start/stop` | Java → Rust | session lifecycle |
| `HybridSession::push_audio_chunk(direction, pcm)` | Java → Rust | raw PCM in; **§2 decision changes this signature** — if Option A ships, this likely stays direction-agnostic (Rust does the fan-out internally) rather than Java pre-labeling which direction a chunk belongs to, since Java can't know that for the both-Live case any better than Rust can |
| `TranslationListener::on_translated_text(direction, text)` | Rust → Java | the callback; Java routes to headphone or speaker TTS based on that direction's mode |

**Open question this doc flags rather than resolves:** should `begin_push_to_talk` /
`end_push_to_talk` (needed by §2 Option A) take a `Direction`, or should Java pass audio
for a push-to-talk direction through a *separate* method entirely rather than sharing
`push_audio_chunk`? Leaning toward sharing the method (one audio ingestion path is
simpler for Java to call correctly) with separate `begin_push_to_talk`/`end_push_to_talk`
signals bracketing it, but this should be confirmed once §2 is settled, not assumed now.

**Also open:** what crosses for §4's echo-safety signal — see below, that's this doc's
main remaining undecided piece along with §2.

## 4. Echo-safety coordination signal — DECISION NEEDED

Full analysis in `../docs/echo-safety-analysis.md`; summary of what that analysis
concluded Rust needs from Java: a **playback-window signal**, because
`AcousticEchoCanceler` is Java-only and reduces but doesn't eliminate echo (especially
over Bluetooth's added latency), so Rust-side VAD gating still needs to cooperate.

### Option A — Hard mute window (recommended as a starting point)

Java calls e.g. `notify_tts_playback(direction, started: bool)` at TTS start/end (plus a
trailing buffer, similar in spirit to `VoiceTranslationService`'s existing ~500ms tail).
While active, the Live direction's VAD is fully suppressed — no utterance boundaries are
even considered. This is the closest analog to what WalkieTalkie already does
(`shouldDeactivateMicDuringTTS() == true`), just re-implemented Rust-side instead of by
stopping `AudioRecord` entirely, so audio can keep flowing to Rust for possible future use
(e.g. barge-in detection) even while suppressed.

**Tradeoff:** simplest to implement and reason about; reintroduces some of the
listen-then-speak latency the project is trying to get away from, exactly as
`echo-safety-analysis.md` warns. Reasonable as a first cut to get *something* working in
Phase 4, with Option B as a later refinement once real Bluetooth latency/echo data exists
from on-device testing (which, per `CLAUDE.md`, only the developer can gather).

### Option B — Sensitivity adjustment, not a hard gate

Instead of fully suppressing VAD, raise its trigger threshold during the playback window
(residual echo is typically quieter than the original TTS output, so a higher energy
threshold may pass genuine speech while rejecting echo tail). Allows barge-in / faster
resumption.

**Tradeoff:** meaningfully more complex, and its correctness entirely depends on
empirical echo characteristics of the developer's specific Bluetooth headphones —
exactly the "highest-risk unknown" `CLAUDE.md` flags as needing the physical device.
Not implementable responsibly without that data; listed here so the interface (§3) can
leave room for it (e.g. a threshold-adjustment parameter, not just a boolean) without
committing to building it in Phase 4.

### Signal shape (applies to either option)

Proposed UniFFI addition to `ffi.rs` (not yet implemented, pending this section's
sign-off):

```rust
fn notify_playback_window(&self, direction: Direction, active: bool);
```

Java calls this around every `TextToSpeech` start/done callback for a Live direction
(push-to-talk directions don't need it — they're never listening during their own TTS
output by construction, same as WalkieTalkie today). Kept as a single boolean rather than
a duration/timestamp so Rust doesn't need to trust Java's clock or guess a trailing-buffer
length — Java, which owns the actual `TextToSpeech.onDone()` callback, decides exactly
when to flip it back off (including whatever trailing buffer it wants), rather than Rust
guessing a fixed offset the way `VoiceTranslationService`'s hardcoded 500ms does today.

## 5. MLKit vs. open-source language ID — DECISION NEEDED

Needed by §2's both-Live-simultaneously disambiguation case (the only case that still
needs it, per §2's analysis — narrower than WalkieTalkie's requirement, which needed it
for its default automatic mode).

### Option A — Reuse MLKit via the Java boundary (recommended for Phase 4, revisit later)

Java already has the MLKit call (`Translator.java`'s `detectLanguage`, confirmed
research: single-file, well-isolated, confidence-threshold-configurable). Cheapest path:
keep using it from Java, pass the *result* (detected language + confidence) across the
UniFFI boundary into whichever Rust-side disambiguation logic §2 needs, rather than
re-implementing language ID in Rust. This keeps the crate's Rust-only claim slightly
weaker (a language-ID decision made outside it) but ships faster and reuses a
well-tested, already-integrated dependency.

**Tradeoff:** `CLAUDE.md` notes upstream RTranslator 3.0 plans to remove MLKit entirely,
and it's closed-source, which cuts against the open-source-extraction goal for a
standalone crate — anyone depending on `adaptive-hybrid-core` outside an MLKit-having
Android app would need to supply their own language ID. Acceptable as a Phase 4 shortcut
specifically because §2 narrowed the requirement to one case (both-Live), not the
default path every utterance takes.

### Option B — Rust-native language ID (e.g. a small fastText/lid.176 style model via `ort`)

Fully self-contained in the crate, no Java dependency, portable to the standalone-repo
goal without caveats. Adds a new model dependency (separate from Whisper/NLLB) and
implementation work not currently scoped anywhere in the phase plan.

**Recommendation:** ship Option A for Phase 4 to keep scope bounded (this project already
has three genuinely novel pieces — the state machine, the echo-safety signal, and the
per-direction pipeline; language ID doesn't need to be a fourth), revisit Option B only
if/when the crate is actually being extracted standalone and MLKit's Java dependency
becomes a real blocker rather than a hypothetical one.

## Summary: what needs the developer's answer before Phase 4 starts

1. §2 — Option A (two independent loops) vs. Option B (single shared loop)? (recommend A)
2. §4 — Option A (hard mute window) vs. Option B (sensitivity adjustment)? (recommend A
   as a first cut, given B needs on-device data this session can't gather)
3. §5 — Option A (reuse MLKit via Java) vs. Option B (Rust-native)? (recommend A)
4. §3's open question — should push-to-talk audio share `push_audio_chunk` with Live
   audio, or use a separate ingestion method? (leaning toward sharing, not yet confirmed)

Everything else in this doc (§1, and the parts of §3 already implemented) is either
already built in Phase 2 code or narrow enough not to need a separate sign-off round.
