#!/usr/bin/env bash
set -euo pipefail
: "${RTAO_TEMP_ROOT:?record the canary task root}"
test "${GITHUB_ACTIONS:-}" = true
test "${RUNNER_OS:-}" = Linux
test "${RUNNER_ENVIRONMENT:-}" = github-hosted
test "${RTAO_CORE_CANARY_DISPOSABLE:-}" = 1
test "$(uname -s)" = Linux
mkdir -p "$RTAO_TEMP_ROOT/canary"
report="$RTAO_TEMP_ROOT/canary/node.json"
cleanup() {
  result=$?
  if test -f "$report"; then
    id=$(jq -er .container_id "$report") || exit 1
    [[ "$id" =~ ^[0-9a-f]{64}$ ]] || exit 1
    if docker inspect "$id" >/dev/null 2>&1; then
      test "$(docker inspect --format '{{index .Config.Labels "io.rtao.localnet"}}' "$id")" = e2e || exit 1
      docker logs --tail 2000 "$id" > "$RTAO_TEMP_ROOT/canary/node.log" 2>&1 || true
      docker rm -f "$id" || result=1
    fi
  fi
  exit "$result"
}
trap cleanup EXIT
digest=$(sed -n 's/^main = { tag = "main", digest = "\([^"]*\)" }.*/\1/p' upstream.toml)
[[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]]
image="ghcr.io/raofoundation/subtensor-localnet@$digest"
docker pull "$image"
commit=$(sed -n '/^\[core\.canary\]/,/^\[/s/^commit = "\([^"]*\)"/\1/p' upstream.toml)
[[ "$commit" =~ ^[0-9a-f]{40}$ ]]
docker image inspect --format '{{json .Config.Labels}}' "$image" > "$RTAO_TEMP_ROOT/canary/image-labels.json"
jq -e --arg commit "$commit" \
  '.["org.opencontainers.image.revision"] == $commit and .["org.opencontainers.image.source"] == "https://github.com/RaoFoundation/subtensor"' \
  "$RTAO_TEMP_ROOT/canary/image-labels.json" >/dev/null
curl --fail --silent --show-error --proto '=https' --tlsv1.2 \
  https://api.drand.sh/52db9ba70e0cc0f6eaf7803dd07447a1f5477735fd3f661792ba94600c84e971/public/latest \
  -o "$RTAO_TEMP_ROOT/canary/drand-preflight.json"
cargo run --locked -p xtask -- localnet up --lane main --profile fast --out "$report"
export PATH="$RTAO_TEMP_ROOT/tools/bin:$PATH"
E2E_ENDPOINT=$(jq -er .endpoint "$report") \
  cargo nextest run --locked -p rtao-testkit --features core-oracle --test core_oracle --profile core-oracle --no-tests fail
