#!/usr/bin/env bash
set -euo pipefail

tag=${1:?usage: macos-build.sh tag}
: "${RTAO_TEMP_ROOT:?record the task root}"
: "${CARGO_TARGET_DIR:?build under the task root}"
: "${TMPDIR:?scratch under the task root}"
source ci/release/license.sh
test "$(uname -s)" = Darwin
test "$(uname -m)" = arm64
test "$(git rev-parse HEAD)" = "$(git rev-parse "refs/tags/$tag^{commit}")"
test -z "$(git status --porcelain --untracked-files=normal)"
# Prerequisites are inspected, never installed or changed on the user's host.
rustup target list --installed | rg -qx aarch64-apple-darwin
if test -n "${RTAO_MAC_INTEL_SYSROOT:-}"; then
  RTAO_MAC_INTEL_SYSROOT=$(cd "$RTAO_MAC_INTEL_SYSROOT" && pwd -P)
  task_root=$(cd "$RTAO_TEMP_ROOT" && pwd -P)
  case "$RTAO_MAC_INTEL_SYSROOT" in "$task_root/"*) ;; *)
    echo 'Intel sysroot must be under the recorded task root' >&2; exit 1 ;;
  esac
  test -d "$RTAO_MAC_INTEL_SYSROOT/lib/rustlib/x86_64-apple-darwin/lib"
  export CARGO_TARGET_X86_64_APPLE_DARWIN_RUSTFLAGS="--sysroot $RTAO_MAC_INTEL_SYSROOT"
else
  rustup target list --installed | rg -qx x86_64-apple-darwin
fi
arch -x86_64 /usr/bin/true
dist="$RTAO_TEMP_ROOT/dist"
mkdir -p "$dist" "$CARGO_TARGET_DIR" "$TMPDIR"
rtao_copy_release_licenses "$dist"
test ! -e "$dist/macos-build.json"
# Use the repository gate's already-provisioned runner. Never install a tool
# or alter PATH outside this process on the user's Mac Studio.
export PATH="$RTAO_TEMP_ROOT/ci/tools/bin:$PATH"
nextest_version=$(sed -n 's/^cargo-nextest = "\([^"]*\)"/\1/p' ci/tool-versions.toml)
test -n "$nextest_version"
case "$(cargo nextest --version)" in "cargo-nextest $nextest_version"|"cargo-nextest $nextest_version "*) ;; *)
  echo 'the repository-pinned nextest runner must already be available' >&2; exit 1 ;;
esac
check() {
  local name=$1
  shift
  local log="$dist/macos-$name.log"
  test ! -e "$log"
  {
    printf 'Command:'
    printf ' %q' "$@"
    printf '\n'
  } > "$log"
  "$@" >> "$log" 2>&1
  printf '\nRTAO_CHECK_EXIT=0\n' >> "$log"
}
check build-arm cargo build --locked --release -p rtao-cli --target aarch64-apple-darwin
check build-intel cargo build --locked --release -p rtao-cli --target x86_64-apple-darwin
check facade-arm cargo check --locked -p rtao --all-features --target aarch64-apple-darwin
check facade-intel cargo check --locked -p rtao --all-features --target x86_64-apple-darwin
check test-cli-arm cargo test --locked -p rtao-cli --target aarch64-apple-darwin
check test-cli-intel cargo test --locked -p rtao-cli --target x86_64-apple-darwin
check test-suite-arm cargo nextest run --locked --workspace --all-features --profile ci --target aarch64-apple-darwin
check test-suite-intel cargo nextest run --locked --workspace --all-features --profile ci --target x86_64-apple-darwin
smoke_arm() {
  "$CARGO_TARGET_DIR/aarch64-apple-darwin/release/rtao" --version
  "$CARGO_TARGET_DIR/aarch64-apple-darwin/release/rtao" doctor --manifest-path Cargo.toml
}
smoke_intel() {
  arch -x86_64 "$CARGO_TARGET_DIR/x86_64-apple-darwin/release/rtao" --version
  arch -x86_64 "$CARGO_TARGET_DIR/x86_64-apple-darwin/release/rtao" doctor --manifest-path Cargo.toml
}
check smoke-arm smoke_arm
check smoke-intel smoke_intel
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  tar -czf "$dist/rtao-$tag-$target.tar.gz" -C "$CARGO_TARGET_DIR/$target/release" rtao \
    -C "$dist" "${rtao_release_license_names[@]}"
done
cargo run --locked -p xtask -- release local-record --tag "$tag" --dir "$dist"
echo 'Mac assets and unsigned build record are ready. Sign only with the approved release key.'
