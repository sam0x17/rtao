#!/usr/bin/env bash
# PLAN M10-04 exit check: RemoteSigner signs a localnet transfer through the
# reference server, which refuses a replayed or tampered request.
#
# For a GitHub ubuntu-latest job with Docker (never a developer machine):
#   - starts and bootstraps one digest-pinned localnet with `xtask localnet up`,
#   - runs crates/rtao-remote-signer/tests/localnet.rs against it (ignored test),
#   - captures bounded node logs and removes only the container it started.
#
# Environment:
#   RTAO_TEMP_ROOT   absolute task root (default: <repo>/target/remote-signer-localnet)
#   LOCALNET_LANE    upstream.toml lane (default: pinned)
#   E2E_BLOCKS       fast | slow (default: fast)
#   SKIP_PULL        set to reuse an already pulled image
#
# Suggested job (integrator adds it under .github/workflows):
#   runs-on: ubuntu-latest
#   timeout-minutes: 45
#   steps: checkout, setup-rust-toolchain, rust-cache,
#     - run: bash ci/interop/remote-signer/localnet.sh
#     - if: always(): upload-artifact path target/remote-signer-localnet/*.log
set -euo pipefail
umask 022

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo=$(cd "$here/../../.." && pwd)
export RTAO_TEMP_ROOT=${RTAO_TEMP_ROOT:-$repo/target/remote-signer-localnet}
case "$RTAO_TEMP_ROOT" in
  /*) ;;
  *) echo "RTAO_TEMP_ROOT must be absolute" >&2; exit 2 ;;
esac
lane=${LOCALNET_LANE:-pinned}
export E2E_BLOCKS=${E2E_BLOCKS:-fast}
report="$RTAO_TEMP_ROOT/node-remote-signer.json"

command -v docker >/dev/null || { echo "docker is required" >&2; exit 2; }
command -v jq >/dev/null || { echo "jq is required" >&2; exit 2; }
mkdir -p "$RTAO_TEMP_ROOT"
rm -f "$report"

cleanup() {
  status=$?
  if [ -f "$report" ]; then
    if id=$(jq -er .container_id "$report") && [[ "$id" =~ ^[0-9a-f]{64}$ ]]; then
      label=$(docker inspect --format '{{index .Config.Labels "io.rtao.localnet"}}' "$id" 2>/dev/null || true)
      if [ "$label" = e2e ]; then
        docker logs --tail 1000 "$id" > "$RTAO_TEMP_ROOT/node-remote-signer.log" 2>&1 || true
        docker rm -f "$id" >/dev/null || status=1
      else
        echo "refusing to remove container $id without the e2e localnet label" >&2
        status=1
      fi
    else
      echo "localnet report has no valid container id" >&2
      status=1
    fi
  fi
  exit "$status"
}
trap cleanup EXIT

cd "$repo"
# Build first so container lifetime covers only the scenario.
cargo test --locked -p rtao-remote-signer --features client,server-online --test localnet --no-run
cargo run --locked -p xtask -- localnet up --lane "$lane" --profile "$E2E_BLOCKS" --out "$report"
endpoint=$(jq -er .endpoint "$report")
case "$endpoint" in
  ws://127.0.0.1:*) ;;
  *) echo "unexpected localnet endpoint $endpoint" >&2; exit 1 ;;
esac
E2E_ENDPOINT="$endpoint" \
  cargo test --locked -p rtao-remote-signer --features client,server-online --test localnet \
  -- --ignored --test-threads 1 --nocapture
echo "remote-signer localnet exit check: ok"
