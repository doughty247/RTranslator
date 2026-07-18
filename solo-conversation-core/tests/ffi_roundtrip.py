#!/usr/bin/env python3
"""Exercises the compiled cdylib through UniFFI's generated Python bindings.

This is the closest thing to a real Java<->Rust round trip test achievable in
a sandbox without Android SDK/NDK: it runs against the *same* generated
extern "C" scaffolding that Kotlin/Java bindings would call through (UniFFI
generates one Rust-side FFI layer shared by every language target), just
driven from Python instead. It is not a substitute for an actual on-device
Kotlin/Java smoke test — see solo-conversation-core/README.md.

Covers the post-sign-off surface from docs/solo-conversation-mode-design.md:
undirected push_audio_chunk with Rust-side fan-out (§2/§3), the
notify_playback_window echo-safety signal (§4), push-to-talk bracketing, and
the optional debug callback (§7) — on top of what Phase 2 already verified
(session lifecycle, config get/set, Off mode).

Run via solo-conversation-core/tests/run_ffi_roundtrip.sh, which generates the
bindings this script imports before invoking it.
"""
import sys

sys.path.insert(0, "target/bindings/python")

import solo_conversation_core as core  # noqa: E402


class RecordingListener(core.TranslationListener):
    def __init__(self):
        self.calls = []

    def on_translated_text(self, direction, text):
        self.calls.append((direction, text))


class RecordingDebugListener(core.DebugListener):
    def __init__(self):
        self.events = []

    def on_debug_event(self, direction, event, elapsed_ms):
        self.events.append((direction, event, elapsed_ms))


LOUD = [0.5] * 10
SILENT = [0.0] * 10


def test_config_and_off_mode():
    config = core.HybridConfig(
        first_language_code="en",
        second_language_code="es",
        first_to_second_mode=core.DirectionMode.LIVE,
        second_to_first_mode=core.DirectionMode.PUSH_TO_TALK,
    )
    assert config.mode(core.Direction.FIRST_TO_SECOND) == core.DirectionMode.LIVE
    assert config.mode(core.Direction.SECOND_TO_FIRST) == core.DirectionMode.PUSH_TO_TALK

    config.set_mode(core.Direction.SECOND_TO_FIRST, core.DirectionMode.OFF)
    assert config.mode(core.Direction.SECOND_TO_FIRST) == core.DirectionMode.OFF
    assert config.mode(core.Direction.FIRST_TO_SECOND) == core.DirectionMode.LIVE, (
        "switching one direction must not affect the other"
    )


def test_session_lifecycle_errors():
    config = core.HybridConfig(
        first_language_code="en",
        second_language_code="es",
        first_to_second_mode=core.DirectionMode.LIVE,
        second_to_first_mode=core.DirectionMode.OFF,
    )
    session = core.HybridSession(config)

    try:
        session.push_audio_chunk(LOUD, 100)
        raise AssertionError("push_audio_chunk should fail before start()")
    except core.HybridError.NotRunning:
        pass

    assert not session.is_running()
    session.start()
    assert session.is_running()

    try:
        session.start()
        raise AssertionError("start() twice should raise AlreadyRunning")
    except core.HybridError.AlreadyRunning:
        pass

    session.stop()
    try:
        session.push_audio_chunk(LOUD, 100)
        raise AssertionError("push_audio_chunk should fail after stop()")
    except core.HybridError.NotRunning:
        pass


def test_live_direction_fires_on_utterance_boundary_not_every_chunk():
    config = core.HybridConfig(
        first_language_code="en",
        second_language_code="es",
        first_to_second_mode=core.DirectionMode.LIVE,
        second_to_first_mode=core.DirectionMode.OFF,
    )
    session = core.HybridSession(config)
    listener = RecordingListener()
    debug_listener = RecordingDebugListener()
    session.set_listener(listener)
    session.set_debug_listener(debug_listener)
    session.start()

    # Below min_voice_ms (250ms default): no translation yet, chunks are
    # still being gated by VAD, not fired per-chunk like the Phase 2 stub did.
    session.push_audio_chunk(LOUD, 100)
    session.push_audio_chunk(LOUD, 100)
    assert len(listener.calls) == 0

    # Crosses 250ms of contiguous voiced audio -> UtteranceStarted.
    session.push_audio_chunk(LOUD, 100)
    assert len(listener.calls) == 0, "utterance started, but not ended -- no translation yet"
    assert any(e == core.GateEvent.UTTERANCE_STARTED for _, e, _ in debug_listener.events)

    # Silence past speech_timeout_ms (1300ms default) ends the utterance.
    for _ in range(14):
        session.push_audio_chunk(SILENT, 100)

    assert len(listener.calls) == 1, f"expected exactly one translation, got {listener.calls}"
    direction, text = listener.calls[0]
    assert direction == core.Direction.FIRST_TO_SECOND
    assert "es" in text, "should route to first_to_second's configured target language"
    assert any(e == core.GateEvent.UTTERANCE_ENDED for _, e, _ in debug_listener.events)

    # Debug events carry increasing timestamps, not all zero.
    timestamps = [ms for _, _, ms in debug_listener.events]
    assert timestamps == sorted(timestamps)
    assert timestamps[-1] > 0

    session.stop()


def test_playback_window_suppresses_vad_and_emits_debug_events():
    config = core.HybridConfig(
        first_language_code="en",
        second_language_code="es",
        first_to_second_mode=core.DirectionMode.LIVE,
        second_to_first_mode=core.DirectionMode.OFF,
    )
    session = core.HybridSession(config)
    listener = RecordingListener()
    debug_listener = RecordingDebugListener()
    session.set_listener(listener)
    session.set_debug_listener(debug_listener)
    session.start()

    session.notify_playback_window(core.Direction.FIRST_TO_SECOND, True)
    assert any(
        e == core.GateEvent.PLAYBACK_WINDOW_STARTED for _, e, _ in debug_listener.events
    )

    # Loud audio arriving while playback is active never starts an utterance.
    session.push_audio_chunk(LOUD, 100)
    session.push_audio_chunk(LOUD, 100)
    session.push_audio_chunk(LOUD, 100)
    assert len(listener.calls) == 0
    assert any(e == core.GateEvent.SUPPRESSED_BY_PLAYBACK for _, e, _ in debug_listener.events)

    session.notify_playback_window(core.Direction.FIRST_TO_SECOND, False)
    assert any(e == core.GateEvent.PLAYBACK_WINDOW_ENDED for _, e, _ in debug_listener.events)

    session.stop()


def test_push_to_talk_bracketing():
    config = core.HybridConfig(
        first_language_code="en",
        second_language_code="es",
        first_to_second_mode=core.DirectionMode.OFF,
        second_to_first_mode=core.DirectionMode.PUSH_TO_TALK,
    )
    session = core.HybridSession(config)
    listener = RecordingListener()
    session.set_listener(listener)
    session.start()

    # Audio pushed before begin_push_to_talk is discarded (Off direction, and
    # PushToTalk isn't armed yet).
    session.push_audio_chunk(LOUD, 100)
    assert len(listener.calls) == 0

    session.begin_push_to_talk(core.Direction.SECOND_TO_FIRST)
    session.push_audio_chunk(LOUD, 100)
    session.push_audio_chunk(LOUD, 100)
    assert len(listener.calls) == 0, "no translation until end_push_to_talk"

    session.end_push_to_talk(core.Direction.SECOND_TO_FIRST)
    assert len(listener.calls) == 1
    direction, text = listener.calls[0]
    assert direction == core.Direction.SECOND_TO_FIRST
    assert "en" in text, "should route to second_to_first's configured target language"

    # Releasing with nothing captured emits nothing.
    session.begin_push_to_talk(core.Direction.SECOND_TO_FIRST)
    session.end_push_to_talk(core.Direction.SECOND_TO_FIRST)
    assert len(listener.calls) == 1, "no new call when no audio was captured while armed"

    session.stop()


def test_both_live_produces_no_premature_guess():
    # Both directions Live is the ambiguous case (§5/§6) -- without real ASR
    # text to disambiguate with, ffi.rs must not guess a direction and emit a
    # translation anyway.
    config = core.HybridConfig(
        first_language_code="en",
        second_language_code="es",
        first_to_second_mode=core.DirectionMode.LIVE,
        second_to_first_mode=core.DirectionMode.LIVE,
    )
    session = core.HybridSession(config)
    listener = RecordingListener()
    session.set_listener(listener)
    session.start()

    for _ in range(3):
        session.push_audio_chunk(LOUD, 100)
    for _ in range(14):
        session.push_audio_chunk(SILENT, 100)

    assert len(listener.calls) == 0, (
        "both-Live ambiguous utterances must not produce a guessed translation "
        "without real ASR text to disambiguate with"
    )

    session.stop()


def main():
    test_config_and_off_mode()
    test_session_lifecycle_errors()
    test_live_direction_fires_on_utterance_boundary_not_every_chunk()
    test_playback_window_suppresses_vad_and_emits_debug_events()
    test_push_to_talk_bracketing()
    test_both_live_produces_no_premature_guess()
    print("ffi_roundtrip.py: all assertions passed")


if __name__ == "__main__":
    main()
