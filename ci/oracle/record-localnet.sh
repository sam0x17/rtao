#!/usr/bin/env bash
set -euo pipefail

: "${RTAO_TEMP_ROOT:?record the task root}"
: "${RTAO_ORACLE_RECORD_IMAGE:?use the verified GHCR image digest}"
test "${GITHUB_ACTIONS:-}" = true
test "$(uname -s)" = Linux
test "${RUNNER_OS:-}" = Linux
test "${RUNNER_ENVIRONMENT:-}" = github-hosted
test "${GITHUB_REPOSITORY:-}" = sam0x17/rtao
test "${RTAO_PYTHON_LANE:-}" = record
if test "${RTAO_ORACLE_RECORD_BUILT:-false}" = true; then
  [[ "$RTAO_ORACLE_RECORD_IMAGE" =~ ^sha256:[0-9a-f]{64}$ ]]
  image_flag=--built-image-id
else
  [[ "$RTAO_ORACLE_RECORD_IMAGE" =~ ^ghcr.io/sam0x17/rtao-oracle@sha256:[0-9a-f]{64}$ ]]
  image_flag=--image
  docker pull "$RTAO_ORACLE_RECORD_IMAGE"
fi
image=$(jq -er '.parity.tag as $tag | if .localnet.pinned.tag == $tag then .localnet.image + "@" + .localnet.pinned.digest else error("localnet and parity tags differ") end' compat.json)
spec=$(jq -er '.parity.tag as $tag | [.metadata_pins[] | select(.network == "finney" and .tag == $tag) | .spec_version] | if length == 1 then .[0] else error("expected one finney metadata pin for parity tag") end' compat.json)
curl --fail --silent --show-error --proto '=https' --tlsv1.2 \
  https://api.drand.sh/52db9ba70e0cc0f6eaf7803dd07447a1f5477735fd3f661792ba94600c84e971/public/latest \
  -o "$RTAO_TEMP_ROOT/drand-preflight.json"
docker pull "$image"
node_id=''
cleanup() {
  if test -n "$node_id"; then
    if ! docker inspect "$node_id" >/dev/null 2>&1; then
      node_id=''
      return
    fi
    test "$(docker inspect --format '{{index .Config.Labels "io.rtao.localnet"}}' "$node_id")" = recording
    docker logs --tail 2000 "$node_id" > "$RTAO_TEMP_ROOT/record-localnet.log" 2>&1 || true
    docker rm -f "$node_id"
  fi
}
trap cleanup EXIT
for attempt in 1 2; do
  node_id=$(docker run -d --rm --label io.rtao.localnet=recording "$image" True)
  case "$node_id" in *[!0-9a-f]*|'') exit 2 ;; esac
  test "${#node_id}" = 64
  ready=false
  for _ in $(seq 1 180); do
    if ! health=$(docker inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{end}}' "$node_id"); then
      break
    fi
    if ! docker logs --tail 200 "$node_id" > "$RTAO_TEMP_ROOT/record-readiness.log" 2>&1; then
      break
    fi
    if test "$health" = healthy || rg -q 'Imported #1([[:space:]]|$)' "$RTAO_TEMP_ROOT/record-readiness.log"; then
      ready=true
      break
    fi
    sleep 1
  done
  if test "$ready" = true && docker run --rm --network "container:$node_id" \
    --env GITHUB_ACTIONS=true --env RTAO_PYTHON_LANE=record \
    --entrypoint /upstream/sdk/python/.venv/bin/python "$RTAO_ORACLE_RECORD_IMAGE" /oracle/wait_localnet.py; then
    cargo run --locked -p xtask -- record "$image_flag" "$RTAO_ORACLE_RECORD_IMAGE" --localnet "$node_id"
    cargo run --locked -p xtask -- record --check --out "$RTAO_TEMP_ROOT/recorded-$spec"
    exit 0
  fi
  cleanup
  node_id=''
  if test "$attempt" = 2; then
    echo '::error::recording localnet failed readiness/drand checks after one fresh-container retry'
    exit 1
  fi
done
