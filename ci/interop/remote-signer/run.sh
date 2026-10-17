#!/usr/bin/env bash
# Two-way interop for rtao-remote-signer/1 against bittensor 10.5.0 key code.
#
# Scope: the upstream code exercised on the Python side is bittensor_wallet's
# Keypair signing and verification. bittensor 10.5.0 ships no http_auth
# module, so both Python peers authenticate with btauth.py, an rtao port of
# the vendored upstream http_auth.py (same payload, headers, check order and
# messages). py_server.py performs no runtime or policy checks. There is no
# upstream rtao-remote-signer/1 implementation (D-031).
#
# 1. Python peer (py_client.py, bittensor-wallet keys) -> Rust rtao-remote-signerd
#    (offline golden runtime): btauth sr25519 and ed25519 clients, signature
#    verification in Python, replay, in-transit tamper, inconsistent body,
#    unknown client, wrong receiver, immortal era and spend policy.
# 2. Rust RemoteSigner client -> Python peer (py_server.py): signing, fresh
#    nonces per attempt, hashed long payloads and upstream refusal messages.
#
# Environment:
#   RTAO_INTEROP_PYTHON  Python with bittensor==10.5.0 (default: .local/m10/py/bin/python)
#   RTAO_TEMP_ROOT       task root for disposable files (default: .local/test-root)
# No node, Docker or network access is needed.
set -euo pipefail
umask 022
# Never leave __pycache__ beside the harness sources.
export PYTHONDONTWRITEBYTECODE=1

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(cd "$here/../../.." && pwd)
python=${RTAO_INTEROP_PYTHON:-$repo/.local/m10/py/bin/python}
export RTAO_TEMP_ROOT=${RTAO_TEMP_ROOT:-$repo/.local/test-root}
work="$RTAO_TEMP_ROOT/remote-signer-interop"
golden="$repo/vendor/upstream/golden.json"

"$python" -c 'import bittensor, bittensor_wallet; assert bittensor.__version__ == "10.5.0", bittensor.__version__'

rm -rf "$work"
mkdir -p "$work"
pids=()
cleanup() {
  for pid in "${pids[@]:-}"; do
    [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
  done
  rm -rf "$work"
}
trap cleanup EXIT

wait_listening() {  # file -> address
  for _ in $(seq 1 200); do
    if line=$(grep -m1 '^listening on ' "$1" 2>/dev/null); then
      echo "${line#listening on }"
      return 0
    fi
    sleep 0.1
  done
  echo "server did not start; log:" >&2
  cat "$1" >&2 || true
  return 1
}

cd "$repo"
cargo build --locked -p rtao-remote-signer --features server-bin --bin rtao-remote-signerd
bin="${CARGO_TARGET_DIR:-$repo/target}/debug/rtao-remote-signerd"

ferdie_ed=$("$python" -c 'from bittensor_wallet import Keypair; print(Keypair.create_from_uri("//Ferdie", crypto_type=0).ss58_address)')
"$python" - "$golden" "$work/metadata.hex" <<'PY'
import json, sys
golden = json.load(open(sys.argv[1]))
open(sys.argv[2], "w").write(golden["metadata"]["v15_hex"])
PY
alice=5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY
charlie=5FLSigC9HGRKVhB9FiEo4Y3koPsNmBmLJbpXg2mp1hXcS59Y
cat > "$work/daemon.toml" <<TOML
[server]
listen = "127.0.0.1:0"

[chain]
metadata = "$work/metadata.hex"
spec_version = 461
transaction_version = 1
ss58_format = 42
spec_name = "node-subtensor"
genesis_hash = "0x1dc00e76a13fc48b9c932e076622c5920295afaad4fd805c284a5cf3222ce452"

[[accounts]]
secret_uri_env = "RTAO_INTEROP_ACCOUNT_URI"
crypto = "sr25519"

[[clients]]
address = "$charlie"
accounts = ["$alice"]

[[clients]]
address = "$ferdie_ed"
accounts = ["$alice"]

[policy]
max_spend_tao = "0.000012345"
allowed_calls = ["Balances.transfer_keep_alive"]
TOML

echo "== Python client -> Rust rtao-remote-signerd"
RTAO_INTEROP_ACCOUNT_URI=//Alice "$bin" --config "$work/daemon.toml" > "$work/daemon.log" 2>&1 &
pids+=($!)
address=$(wait_listening "$work/daemon.log")
(cd "$here" && "$python" "$here/py_client.py" "http://$address/" "$golden") || {
  cat "$work/daemon.log" >&2
  exit 1
}

echo "== Rust RemoteSigner -> Python peer"
(cd "$here" && exec "$python" "$here/py_server.py" 0x1dc00e76a13fc48b9c932e076622c5920295afaad4fd805c284a5cf3222ce452) > "$work/py_server.log" 2>&1 &
pids+=($!)
address=$(wait_listening "$work/py_server.log")
RTAO_REMOTE_SIGNER_PY_URL="http://$address/" \
  cargo test --locked -p rtao-remote-signer --features client --test interop -- --ignored --test-threads 1
echo "remote-signer interop: ok"
