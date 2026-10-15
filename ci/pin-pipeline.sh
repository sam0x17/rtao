#!/usr/bin/env bash
# Called by xtask only after its hosted-lane and candidate-scope checks.
set -euo pipefail
candidate=${1:?candidate workspace}
baseline=${2:?unchanged original workspace}
requested=${3:?release tag or auto}
force=${4:?replay flag}
report=${5:?porting report path}
retained=$(dirname "$report")
: "${RTAO_TEMP_ROOT:?task-scoped scratch}"
: "${RTAO_PIN_STAGE_FILE:?retained pipeline status}"
test "${GITHUB_ACTIONS:-}" = true
test "${RUNNER_OS:-}" = Linux
test "${RUNNER_ENVIRONMENT:-}" = github-hosted
test "${GITHUB_REPOSITORY:-}" = sam0x17/rtao
test "${RTAO_PIN_CANDIDATE:-}" = 1
test "$(uname -s)" = Linux
cd "$candidate"
stage() {
  jq -n --arg stage "$1" --arg tag "$requested" '{schema:1,status:"running",stage:$stage,tag:$tag}' > "$RTAO_PIN_STAGE_FILE"
}
xtask() { cargo run --locked -p xtask -- "$@"; }
cleanup() {
  result=$?
  for profile in fast slow; do
    node_report="$RTAO_TEMP_ROOT/node-$profile.json"
    test -f "$node_report" || continue
    node_id=$(jq -er .container_id "$node_report") || { result=1; continue; }
    [[ "$node_id" =~ ^[0-9a-f]{64}$ ]] || { result=1; continue; }
    if docker inspect "$node_id" >/dev/null 2>&1; then
      if test "$(docker inspect --format '{{index .Config.Labels "io.rtao.localnet"}}' "$node_id")" = e2e; then
        docker logs --tail 2000 "$node_id" > "$RTAO_TEMP_ROOT/node-$profile.log" 2>&1 || true
        docker rm -f "$node_id" || result=1
      else
        result=1
      fi
    fi
  done
  if test "$result" != 0; then
    jq '.status="held"' "$RTAO_PIN_STAGE_FILE" > "$RTAO_TEMP_ROOT/held.json"
    mv "$RTAO_TEMP_ROOT/held.json" "$RTAO_PIN_STAGE_FILE"
  fi
  exit "$result"
}
trap cleanup EXIT
stage provenance
if test "$force" != true; then
  xtask pin --parity "$requested" --prepare-candidate
fi
stage import
if test "$force" = true; then
  xtask import-upstream --cache "$RTAO_UPSTREAM_CACHE"
else
  # Pin preparation removed prior copied artifacts. Use its existing native
  # binary to restore imports before recompiling any consumers of those files.
  "$CARGO_TARGET_DIR/debug/xtask" import-upstream --cache "$RTAO_UPSTREAM_CACHE"
fi
stage core-bump
if ! cmp -s "$baseline/Cargo.toml" Cargo.toml; then
  cargo update -p bittensor-core -p w3f-bls -p subtensor-macros -p subtensor-linting
fi
stage extract
xtask extract
stage regen
xtask regen
xtask compat
stage oracle-stale
export RTAO_PYTHON_LANE=oracle
if test "$force" = true; then
  xtask oracle --check
  xtask record --check
fi
xtask oracle inputs > "$RTAO_TEMP_ROOT/inputs.json"
xtask oracle image
image_tag=$(jq -er .image_tag "$RTAO_TEMP_ROOT/inputs.json")
image_id=$(docker image inspect --format '{{.Id}}' "$image_tag")
[[ "$image_id" =~ ^sha256:[0-9a-f]{64}$ ]]
xtask oracle dump --built-image-id "$image_id" --scope M2
xtask oracle --check --out "$RTAO_TEMP_ROOT/oracle-output"
mkdir -p "$retained"
tar -czf "$retained/oracle-evidence.tar.gz" -C "$RTAO_TEMP_ROOT" oracle-output
if test "$force" = true; then
  # Execution-image IDs are build evidence. Every behavior payload and input
  # lock must still match, and both image/source attestations were checked.
  jq -S '.files | del(.["image.json"])' vendor/oracle/manifest.json > "$RTAO_TEMP_ROOT/oracle-before.json"
  jq -S '.files | del(.["image.json"])' "$RTAO_TEMP_ROOT/oracle-output/manifest.json" > "$RTAO_TEMP_ROOT/oracle-after.json"
  cmp "$RTAO_TEMP_ROOT/oracle-before.json" "$RTAO_TEMP_ROOT/oracle-after.json"
else
  mv vendor/oracle "$RTAO_TEMP_ROOT/previous-oracle"
  mkdir vendor/oracle
  cp -R "$RTAO_TEMP_ROOT/oracle-output/." vendor/oracle/
fi
xtask oracle --check
stage recordings-stale
export RTAO_PYTHON_LANE=record
export RTAO_ORACLE_RECORD_IMAGE="$image_id"
export RTAO_ORACLE_RECORD_BUILT=true
bash ci/oracle/record-localnet.sh
spec=$(jq -er '.metadata_pins[] | select(.network=="finney") | .spec_version' compat.json)
if test "$force" != true; then
  if test -d "vendor/recorded/$spec"; then
    mv "vendor/recorded/$spec" "$RTAO_TEMP_ROOT/previous-recorded"
  fi
  mkdir -p "vendor/recorded/$spec"
  cp -R "$RTAO_TEMP_ROOT/recorded-$spec/." "vendor/recorded/$spec/"
fi
xtask record --check
stage recorded-rust-replay
RTAO_RECORDED_ROOT="$RTAO_TEMP_ROOT/recorded-$spec" \
  cargo test --locked -p rtao-testkit --test goldens --test typed_calls
RTAO_RECORDED_ROOT="$RTAO_TEMP_ROOT/recorded-$spec" \
  cargo test --locked -p rtao-testkit --test corpus corpus_replay -- --exact
if test "$force" = true; then
  RTAO_RECORDED_ROOT="$candidate/vendor/recorded/$spec" \
    cargo test --locked -p rtao-testkit --test goldens --test typed_calls
  RTAO_RECORDED_ROOT="$candidate/vendor/recorded/$spec" \
    cargo test --locked -p rtao-testkit --test corpus corpus_replay -- --exact
fi
tar -czf "$retained/recorded-evidence.tar.gz" -C "$RTAO_TEMP_ROOT" "recorded-$spec"
stage core-surface
xtask core-surface
if ! cmp -s "$baseline/Cargo.toml" Cargo.toml && test -f ci/ledger/pins.toml; then
  stage ledger-stale
  xtask ledger-emu record --if-stale
fi
stage parity-report
xtask parity report --base "$baseline" --out "$report" --autobless
stage pr-gates
xtask gates
stage e2e-required
xtask localnet contracts --check
for profile in fast slow; do
  xtask localnet up --lane pinned --profile "$profile" --out "$RTAO_TEMP_ROOT/node-$profile.json"
  endpoint=$(jq -er .endpoint "$RTAO_TEMP_ROOT/node-$profile.json")
  E2E_ENDPOINT="$endpoint" E2E_LANE=pinned E2E_BLOCKS="$profile" \
    cargo nextest run --locked -p rtao-testkit --features docker --test e2e --profile "e2e-$profile" --test-threads 1 --no-tests fail
done
stage core-oracle
E2E_ENDPOINT=$(jq -er .endpoint "$RTAO_TEMP_ROOT/node-fast.json") \
  cargo nextest run --locked -p rtao-testkit --features core-oracle --test core_oracle --profile core-oracle --no-tests fail
stage validated
