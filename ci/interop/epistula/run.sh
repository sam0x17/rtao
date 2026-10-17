#!/usr/bin/env bash
# Epistula v2 interop: rtao-compat-epistula against the Python references.
#
#   1. Python (BitMind, Apex) signs fixtures; every Python verifier records a
#      verdict per fixture and mutation; the Rust parity tests replay them.
#   2. rtao signs headers; BitMind, Apex and the spec verifier check them,
#      including the secret signatures and Apex canonical JSON.
#   3. rtao client -> BitMind FastAPI get_verifier over HTTP.
#   4. BitMind httpx header hook -> rtao EpistulaLayer over HTTP.
#
# Go (gobt) is not run: no Go toolchain is installed on the development
# host. The gobt dialect is covered by its byte-exact message vectors.
#
# Oracle coverage: BitMind and Apex verdicts come from the pinned upstream
# Python code. The spec verifier and the secret-signature checker are
# transcriptions inside epistula_interop.py. The rtao and gobt parity
# verdicts are BitMind's plus the deviations in tests/vectors.rs.
#
# Usage: ci/interop/epistula/run.sh [--update-fixtures]
# Environment: EPISTULA_PY (Python 3.12 with ci/interop/epistula/requirements.txt),
# EPISTULA_UPSTREAM (M10 upstream sources), RTAO_TEMP_ROOT (scratch root).
set -euo pipefail

root=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
py=${EPISTULA_PY:-$root/.local/m10/py/bin/python}
upstream=${EPISTULA_UPSTREAM:-$root/.local/m10/upstream}
script=$root/ci/interop/epistula/epistula_interop.py
fixtures=$root/crates/rtao-compat-epistula/tests/vectors/python.json
temp_root=${RTAO_TEMP_ROOT:-$root/.local/test-root}
mkdir -p "$temp_root"
work=$(mktemp -d "$temp_root/epistula-interop.XXXXXX")
pids=()
cleanup() {
  for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done
  rm -rf "$work"
}
trap cleanup EXIT

python() { "$py" -I "$script" --upstream "$upstream" "$@"; }

echo "== build"
cargo build -p rtao-compat-epistula --example epistula_interop --features server,client
bin=$(cargo metadata --format-version 1 --no-deps | "$py" -I -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')/debug/examples/epistula_interop

echo "== 1. Python-signed fixtures -> rtao"
python gen-vectors --out "$work/python.json"
EPISTULA_PY_VECTORS=$work/python.json cargo test -p rtao-compat-epistula --test vectors
if [[ ${1:-} == --update-fixtures ]]; then
  cp "$work/python.json" "$fixtures"
  echo "updated $fixtures"
fi

echo "== 2. rtao-signed headers -> Python verifiers"
"$bin" emit-vectors >"$work/rust.json"
python verify-rust --in "$work/rust.json"

echo "== 3. rtao client -> BitMind FastAPI verifier"
port=$("$py" -I -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
python bm-server --port "$port" >"$work/bm-server.log" 2>&1 &
pids+=($!)
for _ in $(seq 1 100); do
  if "$py" -I -c "import socket,sys; s=socket.socket(); sys.exit(s.connect_ex(('127.0.0.1',$port)))"; then break; fi
  sleep 0.2
done
"$bin" client "http://127.0.0.1:$port"

echo "== 4. BitMind httpx client -> rtao EpistulaLayer"
mkfifo "$work/serve.stdin"
"$bin" serve <"$work/serve.stdin" >"$work/serve.out" 2>&1 &
pids+=($!)
exec 9>"$work/serve.stdin"
for _ in $(seq 1 100); do
  grep -q '^LISTENING ' "$work/serve.out" 2>/dev/null && break
  sleep 0.1
done
rtao_port=$(sed -n 's/^LISTENING //p' "$work/serve.out")
python bm-client --base "http://127.0.0.1:$rtao_port"
exec 9>&-
echo "== epistula interop: all passed"
