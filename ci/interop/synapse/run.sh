#!/usr/bin/env bash
# M10-01 interop: rtao-compat-synapse against bittensor==10.5.0 on loopback.
#
# Runs, in order:
#   1. Rust dendrite  -> Python axon   (interop_dendrite <port> python)
#   2. Python dendrite -> Rust axon    (py_dendrite.py <port> rust)
#   3. Python dendrite -> Python axon  (baseline: validates the expectations)
#   4. Rust dendrite  -> Rust axon     (rtao <-> rtao)
#   5. S3/I18: Python bittensor-wallet verifies a Rust signature
#
# Environment:
#   PYTHON   interpreter with exactly requirements.txt installed (default
#            .local/m10/synapse-py/bin/python), created with:
#              uv venv --seed --python 3.12 .local/m10/synapse-py
#              .local/m10/synapse-py/bin/python -m pip install --require-hashes \
#                -r ci/interop/synapse/requirements.txt
#   OUT_DIR  where JSON-lines results are written (default .local/m10/synapse-interop)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
HARNESS="$ROOT/ci/interop/synapse"
PYTHON="${PYTHON:-$ROOT/.local/m10/synapse-py/bin/python}"
OUT_DIR="${OUT_DIR:-$ROOT/.local/m10/synapse-interop}"
PY_PORT="${PY_PORT:-18291}"
RS_PORT="${RS_PORT:-18292}"
mkdir -p "$OUT_DIR"
umask 022

cargo build -p rtao-compat-synapse --features axon,dendrite,tensor \
  --example interop_axon --example interop_dendrite
EXAMPLES="$(cargo metadata --format-version 1 --no-deps | "$PYTHON" -I -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')/debug/examples"

pids=()
cleanup() {
  for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done
  wait 2>/dev/null || true
}
trap cleanup EXIT

wait_ready() {
  local log="$1"
  for _ in $(seq 1 200); do
    grep -q '^READY' "$log" 2>/dev/null && return 0
    sleep 0.1
  done
  echo "server did not start; log follows:" >&2
  cat "$log" >&2
  return 1
}

"$PYTHON" -I "$HARNESS/py_axon.py" "$PY_PORT" "$HARNESS" >"$OUT_DIR/py_axon.log" 2>&1 &
pids+=($!)
"$EXAMPLES/interop_axon" "$RS_PORT" >"$OUT_DIR/rust_axon.log" 2>&1 &
pids+=($!)
wait_ready "$OUT_DIR/py_axon.log"
wait_ready "$OUT_DIR/rust_axon.log"

status=0
run() {
  local name="$1"
  shift
  if "$@" >"$OUT_DIR/$name.jsonl" 2>"$OUT_DIR/$name.stderr"; then
    echo "PASS $name ($(grep -c '"pass": true\|"pass":true' "$OUT_DIR/$name.jsonl") checks)"
  else
    echo "FAIL $name"
    grep -v '"pass": true\|"pass":true' "$OUT_DIR/$name.jsonl" || true
    status=1
  fi
}

run rust-dendrite-python-axon "$EXAMPLES/interop_dendrite" "$PY_PORT" python
run python-dendrite-rust-axon "$PYTHON" -I "$HARNESS/py_dendrite.py" "$RS_PORT" rust "$HARNESS"
run python-dendrite-python-axon "$PYTHON" -I "$HARNESS/py_dendrite.py" "$PY_PORT" python "$HARNESS"
run rust-dendrite-rust-axon "$EXAMPLES/interop_dendrite" "$RS_PORT" rust
S1="$("$EXAMPLES/interop_dendrite" --sign-s1)"
run python-verifies-rust-signature "$PYTHON" -I "$HARNESS/py_dendrite.py" --verify-s3 "$S1" "$HARNESS"

"$PYTHON" -I -c 'import importlib.metadata as m; print({p: m.version(p) for p in ["bittensor","bittensor-wallet","aiohttp","fastapi","starlette","pydantic","uvicorn","msgpack-numpy-opentensor","numpy"]})' \
  >"$OUT_DIR/versions.txt"
cat "$OUT_DIR/versions.txt"
exit "$status"
