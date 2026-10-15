#!/usr/bin/env bash
set -euo pipefail

tag=${1:?usage: docs-build.sh tag}
: "${RTAO_TEMP_ROOT:?record the task root}"
: "${CARGO_TARGET_DIR:?build under the task root}"
: "${TMPDIR:?scratch under the task root}"
source ci/release/pins.env
source ci/release/license.sh
dist="$RTAO_TEMP_ROOT/dist"
scratch="$RTAO_TEMP_ROOT/docs"
mkdir -p "$dist" "$scratch" "$TMPDIR"
rtao_copy_release_licenses "$dist"
test "$(git rev-parse HEAD)" = "$(git rev-parse "refs/tags/$tag^{commit}")"
test -z "$(git status --porcelain --untracked-files=normal)"
test "${GITHUB_REF:?must run in the tag workflow}" = "refs/tags/$tag"
test "${GITHUB_SHA:?must run in the tag workflow}" = "$(git rev-parse HEAD)"
cargo run --locked -p xtask -- compat --check
cargo run --locked -p xtask -- release-notes --tag "$tag" > "$dist/notes.md"
cp COMPAT.md compat.json "$dist/"
RUSTDOCFLAGS='-D warnings' cargo doc --locked --workspace --no-deps --all-features
printf '%s\n' '<!doctype html><html lang="en"><meta charset="utf-8"><meta http-equiv="refresh" content="0;url=rtao/index.html"><title>rtao API</title><a href="rtao/index.html">Open the rtao Rust API documentation</a></html>' > "$CARGO_TARGET_DIR/doc/index.html"
cargo run --locked -p xtask -- docs --llms --out "$scratch"
cargo run --locked -p xtask -- docs --llms --out "$scratch" --check
cp -R book "$scratch/book-source"
cp "$scratch/api-index.md" "$scratch/book-source/src/api-index.md"
cp "$scratch/tx-reference.md" "$scratch/book-source/src/tx-reference.md"
mdbook build "$scratch/book-source" --dest-dir "$scratch/book-html"
cp -R "$CARGO_TARGET_DIR/doc" "$scratch/book-html/api"
cp "$scratch/llms.txt" "$dist/llms.txt"
rtao_copy_release_licenses "$CARGO_TARGET_DIR/doc"
rtao_copy_release_licenses "$scratch/book-html"
tar -C "$CARGO_TARGET_DIR/doc" -czf "$dist/rustdoc-$tag.tar.gz" .
tar -C "$scratch/book-html" -czf "$dist/book-$tag.tar.gz" .
artifacts=$(for name in "rustdoc-$tag.tar.gz" "book-$tag.tar.gz" COMPAT.md compat.json llms.txt notes.md "${rtao_release_license_names[@]}"; do
  jq -n --arg name "$name" --arg hash "$(sha256sum "$dist/$name" | cut -d' ' -f1)" '{key:$name,value:$hash}'
done | jq -s 'from_entries')
jq -n --arg tag "$tag" --arg commit "$(git rev-parse HEAD)" --arg tree "$(git rev-parse 'HEAD^{tree}')" \
  --arg rustc "$(rustc --version --verbose)" --arg cargo "$(cargo --version)" \
  --arg mdbook "$(mdbook --version)" --argjson artifacts "$artifacts" \
  '{schema:"rtao-build-v1",tag:$tag,commit:$commit,tree:$tree,rustc:$rustc,cargo:$cargo,builder:"github-ubuntu-latest",mdbook:$mdbook,artifacts:$artifacts}' \
  > "$dist/docs-build.json"
