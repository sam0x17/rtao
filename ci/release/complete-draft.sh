#!/usr/bin/env bash
set -euo pipefail

# The caller first downloads Linux/docs artifacts from the successful tag run
# into the same dist directory as the signed local Mac build.
tag=${1:?usage: complete-draft.sh tag}
: "${RTAO_TEMP_ROOT:?record the task root}"
dist="$RTAO_TEMP_ROOT/dist"
cargo run --locked -p xtask -- release verify --tag "$tag" --dir "$dist" --write-checksums
if gh release view "$tag" --repo sam0x17/rtao >/dev/null 2>&1; then
  echo 'release already exists; refusing to replace any assets' >&2
  exit 1
fi
# Nothing is uploaded until verification of every planned target and its
# provenance has passed. This creates a complete draft, never publishes it.
gh release create "$tag" "$dist"/* --repo sam0x17/rtao --verify-tag \
  --title "$tag" --notes-file "$dist/notes.md" --draft
echo 'Complete draft created. Dispatch release.yml in dry-run mode to verify it, then publish mode.'
