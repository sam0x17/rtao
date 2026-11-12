#!/usr/bin/env bash
set -euo pipefail
# Resolve the SDK Cargo actually selected: the crates.io release, or a checkout
# that ci/patch-source.sh put in its place.
testkit=$(cargo metadata --locked --format-version 1 | jq -ec '.packages[] | select(.name == "rtao-testkit")')
crate=$(dirname "$(jq -er .manifest_path <<<"$testkit")")
version=$(jq -er .version <<<"$testkit")
# rtao-testkit carries the pins it runs (its own copy of upstream.toml).
test -f "$crate/upstream.toml"
image=$(sed -n 's/^image = "\([^"]*\)"$/\1/p' "$crate/upstream.toml")
digest=$(sed -n 's/^pinned = .*digest = "\(sha256:[0-9a-f]*\)".*$/\1/p' "$crate/upstream.toml")
[[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]]
[[ "$image" =~ ^ghcr.io/[a-zA-Z0-9_/-]+$ ]]
# The checkout root, when the SDK comes from one; empty for the release.
sdk=$(cd "$crate/../.." && pwd -P)
if ! test -f "$sdk/upstream.toml" || ! test -f "$sdk/crates/rtao-cli/Cargo.toml"; then
  sdk=
fi
printf 'RTAO_SOURCE=%s\nRTAO_VERSION=%s\nLOCALNET_IMAGE=%s@%s\n' "$sdk" "$version" "$image" "$digest"
