#!/usr/bin/env bash
set -euo pipefail
: "${RTAO_TEMP_ROOT:?record the task root before preparing the PR-U4 probe}"
: "${CARGO_TARGET_DIR:?use the task-scoped Cargo target}"
: "${TMPDIR:?use the task-scoped temporary directory}"
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)
[[ "$RTAO_TEMP_ROOT" = /* && "$RTAO_TEMP_ROOT" =~ ^[A-Za-z0-9_./-]+$ ]]
[[ "$CARGO_TARGET_DIR" = "$RTAO_TEMP_ROOT/"* && "$TMPDIR" = "$RTAO_TEMP_ROOT/"* ]]
proof="$RTAO_TEMP_ROOT/pr-u4-proof"
mkdir -p "$proof"
core_commit=$(sed -n '/^\[core\]/,/^\[/s/^commit = "\([^"]*\)"/\1/p' "$repo/upstream.toml")
core_tag=$(sed -n '/^\[core\]/,/^\[/s/^tag = "\([^"]*\)"/\1/p' "$repo/upstream.toml")
[[ "$core_commit" =~ ^[0-9a-f]{40}$ && "$core_tag" =~ ^v[0-9]+$ ]]
source_repo="$proof/source"
worktree="$proof/upstream"
if ! test -d "$source_repo/.git"; then
  git clone --filter=blob:none --no-checkout --depth 1 --branch "$core_tag" \
    https://github.com/RaoFoundation/subtensor.git "$source_repo"
fi
test "$(git -C "$source_repo" rev-parse "refs/tags/$core_tag^{commit}")" = "$core_commit"
if ! test -f "$worktree/.git"; then
  git -C "$source_repo" worktree add --detach "$worktree" "$core_commit"
fi
test "$(git -C "$worktree" rev-parse HEAD)" = "$core_commit"
core_manifest="$worktree/sdk/bittensor-core/Cargo.toml"
if rg -q '^w3f-bls = \{ workspace = true, features = \["std"\] \}$' "$core_manifest"; then
  sed 's/^w3f-bls = { workspace = true, features = \["std"\] }$/w3f-bls = { version = "=0.1.3", features = ["std"] }/' \
    "$core_manifest" > "$proof/core-Cargo.toml"
  mv "$proof/core-Cargo.toml" "$core_manifest"
fi
rg -q '^w3f-bls = \{ version = "=0.1.3", features = \["std"\] \}$' "$core_manifest"
git -C "$worktree" diff -- sdk/bittensor-core/Cargo.toml > "$proof/pr-u4.patch"
test "$(git -C "$worktree" diff --name-only)" = sdk/bittensor-core/Cargo.toml
probe="$proof/probe"
mkdir -p "$probe/src"
cp "$repo/ci/pr-u4-probe/src/main.rs" "$probe/src/main.rs"
cp "$repo/ci/pr-u4-probe/deterministic.rs" "$probe/deterministic.rs"
sed "s|@CORE_PATH@|$worktree/sdk/bittensor-core|g" \
  "$repo/ci/pr-u4-probe/Cargo.toml.in" > "$probe/Cargo.toml"
if test -f "$repo/ci/pr-u4-probe/Cargo.lock"; then
  cp "$repo/ci/pr-u4-probe/Cargo.lock" "$probe/Cargo.lock"
fi
if ! test -f "$probe/Cargo.lock"; then
  echo 'The probe needs its reviewed isolated Cargo.lock; ask the lock owner to generate it.' >&2
  exit 1
fi
cargo metadata --locked --manifest-path "$probe/Cargo.toml" --format-version 1 > "$proof/cargo-metadata.json"
jq -e '[.packages[] | select(.name == "w3f-bls")] | length == 1 and .[0].version == "0.1.3" and (.[0].source | startswith("registry+"))' \
  "$proof/cargo-metadata.json" >/dev/null
cargo build --locked --manifest-path "$probe/Cargo.toml"
jq -n --arg manifest "$probe/Cargo.toml" --arg binary "$CARGO_TARGET_DIR/debug/rtao-pr-u4-probe" \
  --arg core_commit "$core_commit" --arg metadata "$proof/cargo-metadata.json" \
  '{format:1, manifest:$manifest, binary:$binary, core_commit:$core_commit, metadata:$metadata}' \
  > "$proof/prepared.json"
