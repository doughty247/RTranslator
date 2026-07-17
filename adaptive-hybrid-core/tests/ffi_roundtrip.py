#!/usr/bin/env python3
"""Exercises the compiled cdylib through UniFFI's generated Python bindings.

This is the closest thing to a real Java<->Rust round trip test achievable in
a sandbox without Android SDK/NDK: it runs against the *same* generated
extern "C" scaffolding that Kotlin/Java bindings would call through (UniFFI
generates one Rust-side FFI layer shared by every language target), just
driven from Python instead. It is not a substitute for an actual on-device
Kotlin/Java smoke test — see adaptive-hybrid-core/README.md.

Run via adaptive-hybrid-core/tests/run_ffi_roundtrip.sh, which generates the
bindings this script imports before invoking it.
"""
import sys

sys.path.insert(0, "target/bindings/python")

import adaptive_hybrid_core as core  # noqa: E402


class RecordingListener(core.TranslationListener):
    def __init__(self):
        self.calls = []

    def on_translated_text(self, direction, text):
        self.calls.append((direction, text))


def main():
    config = core.HybridConfig(
        first_language_code="en",
        second_language_code="es",
        first_to_second_mode=core.DirectionMode.LIVE,
        second_to_first_mode=core.DirectionMode.PUSH_TO_TALK,
    )
    assert config.mode(core.Direction.FIRST_TO_SECOND) == core.DirectionMode.LIVE
    assert config.mode(core.Direction.SECOND_TO_FIRST) == core.DirectionMode.PUSH_TO_TALK

    config.set_mode(core.Direction.SECOND_TO_FIRST, core.DirectionMode.LIVE)
    assert config.mode(core.Direction.SECOND_TO_FIRST) == core.DirectionMode.LIVE
    assert config.mode(core.Direction.FIRST_TO_SECOND) == core.DirectionMode.LIVE, (
        "switching one direction must not affect the other"
    )

    session = core.HybridSession(config)
    listener = RecordingListener()
    session.set_listener(listener)

    try:
        session.push_audio_chunk(core.Direction.FIRST_TO_SECOND, [0.0, 0.1, 0.2])
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

    pcm = [0.0] * 1600  # 100ms of silence at 16kHz, per the model artifact contract
    session.push_audio_chunk(core.Direction.FIRST_TO_SECOND, pcm)
    assert session.chunks_received() == 1
    assert len(listener.calls) == 1
    direction, text = listener.calls[0]
    assert direction == core.Direction.FIRST_TO_SECOND
    assert "1600 samples" in text
    assert "es" in text, "should route to first_to_second's configured target language"

    session.stop()
    assert not session.is_running()

    try:
        session.push_audio_chunk(core.Direction.FIRST_TO_SECOND, pcm)
        raise AssertionError("push_audio_chunk should fail after stop()")
    except core.HybridError.NotRunning:
        pass

    print("ffi_roundtrip.py: all assertions passed")


if __name__ == "__main__":
    main()
