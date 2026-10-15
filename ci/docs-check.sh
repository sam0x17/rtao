#!/usr/bin/env bash
# Development docs checks only; no release tag, packaging or publishing.
set -euo pipefail

test "${GITHUB_ACTIONS:-}" = true
test "${RUNNER_OS:-}" = Linux
test "${RUNNER_ARCH:-}" = X64
: "${RTAO_TEMP_ROOT:?hosted task root is required}"
: "${CARGO_TARGET_DIR:?reuse the hosted gate target directory}"
: "${TMPDIR:?use the hosted task scratch directory}"
[[ "$RTAO_TEMP_ROOT" = /* ]]
test "$CARGO_TARGET_DIR" = "$RTAO_TEMP_ROOT/build"
test "$TMPDIR" = "$RTAO_TEMP_ROOT/tmp"

source ci/release/pins.env
docs_root="$RTAO_TEMP_ROOT/ci/docs"
test ! -e "$docs_root"
mkdir -p "$docs_root/bin" "$docs_root/generated"

# Reuse the reviewed mdBook release and checksum instead of compiling a tool.
curl --fail --location --proto '=https' --tlsv1.2 \
  --connect-timeout 15 --max-time 120 --retry 2 --max-filesize 33554432 \
  "https://github.com/rust-lang/mdBook/releases/download/v$RTAO_MDBOOK_VERSION/mdbook-v$RTAO_MDBOOK_VERSION-x86_64-unknown-linux-gnu.tar.gz" \
  -o "$docs_root/mdbook.tar.gz"
printf '%s  %s\n' "$RTAO_MDBOOK_LINUX_SHA256" "$docs_root/mdbook.tar.gz" | sha256sum --check --strict
tar -xOf "$docs_root/mdbook.tar.gz" mdbook > "$docs_root/bin/mdbook"
chmod 755 "$docs_root/bin/mdbook"
test "$("$docs_root/bin/mdbook" --version)" = "mdbook v$RTAO_MDBOOK_VERSION"

RUSTDOCFLAGS='-D warnings' cargo doc --locked --workspace --no-deps --all-features
cargo run --locked -p xtask -- docs --llms --out "$docs_root/generated"
cargo run --locked -p xtask -- docs --llms --out "$docs_root/generated" --check

# Copy only the small book source and generated references. The generated
# SUMMARY includes every catalog page so mdBook renders and links the full set.
cp -R book "$docs_root/book-source"
cp -R "$docs_root/generated/." "$docs_root/book-source/src/"
"$docs_root/bin/mdbook" build "$docs_root/book-source" --dest-dir "$docs_root/book-html"

# Downloadable development docs retain the same required attribution as source.
source ci/release/license.sh
rtao_copy_release_licenses "$CARGO_TARGET_DIR/doc"
rtao_copy_release_licenses "$docs_root/book-html"
rtao_copy_release_licenses "$docs_root/generated"
# Keep the book's API links usable without copying another rendered doc tree.
mv "$CARGO_TARGET_DIR/doc" "$docs_root/book-html/api"
