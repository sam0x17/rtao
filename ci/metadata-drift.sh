#!/usr/bin/env bash
# Run semantic drift checks, then prepare upkeep changes without publishing in dry-run.
set -euo pipefail
network=${1:?network required}
phase=${2:-scan}
case "$network" in finney|testnet|devnet|localnet-main) ;; *) echo "Unknown network" >&2; exit 2 ;; esac
case "$phase" in scan|apply|publish) ;; *) echo "Unknown phase" >&2; exit 2 ;; esac
: "${RTAO_TEMP_ROOT:=$PWD/target/metadata-drift}"
: "${RTAO_DRIFT_DRY_RUN:=true}"
case "$RTAO_DRIFT_DRY_RUN" in true|false) ;; *) echo "Invalid dry-run boolean" >&2; exit 2 ;; esac
out="$RTAO_TEMP_ROOT/drift/$network"
mkdir -p "$out"
args=(--network "$network" --out "$out" --check-open-prs)
if [[ "$network" == localnet-main ]]; then
  : "${RTAO_DRIFT_CONTAINER:?localnet-main container required}"
  args+=(--localnet-container "$RTAO_DRIFT_CONTAINER")
fi
if [[ "$phase" == scan ]]; then
  cargo run --locked -p xtask -- drift "${args[@]}"
  exit 0
fi
[[ -f "$out/changed" ]] || exit 0
if [[ "$phase" == apply ]]; then
  cargo run --locked -p xtask -- drift "${args[@]}" --apply
  cargo run --locked -p xtask -- regen --check
  cargo run --locked -p xtask -- extract --check
  cargo test --locked -p rtao-runtime -p rtao-reads -p rtao-metagraph --lib
  git diff --stat
  exit 0
fi
[[ "$RTAO_DRIFT_DRY_RUN" == false ]] || { git status --short; exit 0; }
# GitHub App token is present only in the publish step, after read-only checks.
: "${GH_TOKEN:?GitHub App token required for upkeep publication}"
spec=$(cat "$out/spec")
[[ "$spec" =~ ^[0-9]+$ ]] || exit 2
branch="bot/drift-$network-$spec"
git switch -c "$branch"
git add vendor/metadata crates/rtao-runtime/src/generated crates/rtao-catalog/src/generated crates/rtao-reads/src/generated
git -c user.name=rtao-bot -c user.email=rtao-bot@users.noreply.github.com commit -m "$(cat "$out/title")"
git push origin "HEAD:refs/heads/$branch"
url=$(gh pr create --title "$(cat "$out/title")" --body-file "$out/report.md" --head "$branch" --base main)
if [[ "$(cat "$out/class")" == surface-unchanged ]]; then
  gh pr merge "$url" --auto --squash
fi
