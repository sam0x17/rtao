#!/usr/bin/env bash
set -euo pipefail
# Resolve the source actually selected by Cargo, including rtao's checkout overlay.
manifest=$(cargo metadata --locked --format-version 1 | jq -er '.packages[] | select(.name == "rtao-testkit") | .manifest_path')
sdk=$(cd "$(dirname "$manifest")/../.." && pwd -P)
test -f "$sdk/upstream.toml"
image=$(sed -n 's/^image = "\([^"]*\)"$/\1/p' "$sdk/upstream.toml")
digest=$(sed -n 's/^pinned = .*digest = "\(sha256:[0-9a-f]*\)".*$/\1/p' "$sdk/upstream.toml")
[[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]]
[[ "$image" =~ ^ghcr.io/[a-zA-Z0-9_/-]+$ ]]
printf 'RTAO_SOURCE=%s\nLOCALNET_IMAGE=%s@%s\n' "$sdk" "$image" "$digest"
