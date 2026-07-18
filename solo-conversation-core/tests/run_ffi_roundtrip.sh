#!/usr/bin/env bash
# Builds the cdylib, generates UniFFI's Python bindings against it, and runs
# tests/ffi_roundtrip.py. See that file's docstring for why Python stands in
# for a Java/Kotlin round trip in this environment.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --lib
LIB=target/debug/libsolo_conversation_core.so
if [ ! -f "$LIB" ]; then
  LIB=target/debug/libsolo_conversation_core.dylib
fi

mkdir -p target/bindings/python
cargo run --bin uniffi-bindgen -- generate --library "$LIB" --language python --out-dir target/bindings/python

# UniFFI's Python target needs the compiled dylib discoverable at import time.
cp "$LIB" target/bindings/python/

PYTHONPATH="target/bindings/python:${PYTHONPATH:-}" python3 tests/ffi_roundtrip.py
