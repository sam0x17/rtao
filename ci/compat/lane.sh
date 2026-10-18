#!/usr/bin/env bash
# Run the required e2e stage and the core oracle against one digest-pinned
# localnet lane, as the pin pipeline does. With RTAO_COMPAT_REPORTS set, a
# hosted lane records a passed e2e or interop report after each stage passes.
set -euo pipefail
lane=${1:?localnet lane}
digest=${2:?expected localnet image digest}
: "${RTAO_TEMP_ROOT:?task-scoped scratch}"
case "$lane" in
  pinned | mainnet | testnet | devnet | main) ;;
  *)
    echo "unknown localnet lane $lane" >&2
    exit 2
    ;;
esac
[[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]]
xtask() { cargo run --locked -p xtask -- "$@"; }
cleanup() {
  result=$?
  for node in target upgrade-target upgrade-source; do
    node_report="$RTAO_TEMP_ROOT/node-$node.json"
    test -f "$node_report" || continue
    node_id=$(jq -er .container_id "$node_report") || { result=1; continue; }
    [[ "$node_id" =~ ^[0-9a-f]{64}$ ]] || { result=1; continue; }
    if docker inspect "$node_id" >/dev/null 2>&1; then
      if test "$(docker inspect --format '{{index .Config.Labels "io.rtao.localnet"}}' "$node_id")" = e2e; then
        docker logs --tail 2000 "$node_id" > "$RTAO_TEMP_ROOT/node-$node.log" 2>&1 || true
        docker rm -f "$node_id" >/dev/null || result=1
      else
        result=1
      fi
    fi
  done
  # Each localnet runs on its own labelled network; drop the unused ones.
  docker network prune -f --filter label=io.rtao.localnet=e2e >/dev/null || true
  exit "$result"
}
trap cleanup EXIT
report() {
  test -n "${RTAO_COMPAT_REPORTS:-}" || return 0
  mkdir -p "$RTAO_COMPAT_REPORTS"
  xtask compat report --kind "$1" --image-digest "$digest" \
    --out "$RTAO_COMPAT_REPORTS/$1-${digest:7:12}.json"
}

xtask localnet contracts --check
# Scenario 16 needs the isolated registry-BLS probe; scenario 8 needs the
# digest-pinned main fast runtime and a node serving it.
# Its binary must live under the task root; a local run whose shared Cargo
# target lies elsewhere names a probe-only target with RTAO_PROBE_TARGET_DIR.
CARGO_TARGET_DIR="${RTAO_PROBE_TARGET_DIR:-${CARGO_TARGET_DIR:?Cargo target}}" bash ci/pr-u4-probe/prepare.sh
xtask localnet up --lane main --profile fast --out "$RTAO_TEMP_ROOT/node-upgrade-source.json"
upgrade_id=$(jq -er .container_id "$RTAO_TEMP_ROOT/node-upgrade-source.json")
[[ "$upgrade_id" =~ ^[0-9a-f]{64}$ ]]
test "$(docker inspect --format '{{index .Config.Labels "io.rtao.localnet"}}' "$upgrade_id")" = e2e
docker cp "$upgrade_id:/target/fast-runtime/release/node_subtensor_runtime.compact.compressed.wasm" "$RTAO_TEMP_ROOT/main-fast-runtime.wasm"
upgrade_endpoint=$(jq -er .endpoint "$RTAO_TEMP_ROOT/node-upgrade-source.json")

start() {
  xtask localnet up --lane "$lane" --profile fast --out "$RTAO_TEMP_ROOT/node-$1.json"
  image=$(jq -er .image "$RTAO_TEMP_ROOT/node-$1.json")
  if test "${image##*@}" != "$digest"; then
    echo "lane $lane started $image, expected digest $digest" >&2
    exit 1
  fi
}
# Scenario 8 replaces the chain's runtime with the main fast runtime when that
# is newer. It gets its own node, so every other scenario and the core oracle
# exercise this lane's own runtime.
start target
start upgrade-target
endpoint=$(jq -er .endpoint "$RTAO_TEMP_ROOT/node-target.json")

E2E_ENDPOINT="$endpoint" E2E_LANE="$lane" E2E_BLOCKS=fast \
  cargo nextest run --locked -p rtao-testkit --features docker --test e2e --profile e2e-fast \
  --test-threads 1 --no-tests fail -E 'not test(/^scenario_08_/)'
E2E_ENDPOINT=$(jq -er .endpoint "$RTAO_TEMP_ROOT/node-upgrade-target.json") E2E_LANE="$lane" E2E_BLOCKS=fast \
  E2E_UPGRADE_ENDPOINT="$upgrade_endpoint" E2E_UPGRADE_WASM="$RTAO_TEMP_ROOT/main-fast-runtime.wasm" \
  cargo nextest run --locked -p rtao-testkit --features docker --test e2e --profile e2e-fast \
  --test-threads 1 --no-tests fail -E 'test(/^scenario_08_/)'
report e2e

E2E_ENDPOINT="$endpoint" \
  cargo nextest run --locked -p rtao-testkit --features core-oracle --test core_oracle --profile core-oracle --no-tests fail
report interop
