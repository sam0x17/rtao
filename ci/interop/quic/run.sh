#!/usr/bin/env bash
# Interop between rtao-transport-quic and btlightning's Python bindings.
#
# Requires a Python environment with btlightning-py installed from
# inference-labs-inc/btlightning @ 53c090b436238f78a4a0dd30cbbfcba1cbebe2e0:
#
#   cd <lightning>/crates/btlightning-py && maturin develop --release
#
# Run maturin from a directory outside the rtao checkout (or pass
# --manifest-path from outside it): rtao's .cargo/config.toml sets
# PYO3_NO_PYTHON=1, which otherwise makes pyo3 0.29 fail with
# "Neither abi3 or abi3t features are enabled".
#
# Environment:
#   PYTHON       interpreter with btlightning installed (default: python3)
#   CAPTURE_DIR  directory for captured handshake fixtures (optional)
#   RESULTS      path for a JSON results file (optional)
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
PYTHON=${PYTHON:-python3}

cargo build --locked --manifest-path "$ROOT/Cargo.toml" -p rtao-transport-quic --example quic_peer
TARGET_DIR=$(cargo metadata --manifest-path "$ROOT/Cargo.toml" --format-version 1 --no-deps \
  | "$PYTHON" -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
PEER="$TARGET_DIR/debug/examples/quic_peer"

"$PYTHON" -c 'import btlightning, sys; print("btlightning", btlightning.__file__, "python", sys.version.split()[0])'
rustc --version

ARGS=(--peer "$PEER")
if [[ -n "${CAPTURE_DIR:-}" ]]; then
  mkdir -p "$CAPTURE_DIR"
  ARGS+=(--capture-dir "$CAPTURE_DIR")
fi
if [[ -n "${RESULTS:-}" ]]; then
  ARGS+=(--results "$RESULTS")
fi
exec "$PYTHON" -I "$ROOT/ci/interop/quic/interop.py" "${ARGS[@]}"
