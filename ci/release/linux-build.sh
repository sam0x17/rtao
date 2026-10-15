#!/usr/bin/env bash
set -euo pipefail

tag=${1:?usage: linux-build.sh tag target}
target=${2:?usage: linux-build.sh tag target}
case "$tag" in rtao-v*) ;; *) exit 2 ;; esac
case "$target" in
  x86_64-unknown-linux-gnu) mode=native ;;
  aarch64-unknown-linux-gnu) mode=cross-qemu ;;
  *) echo 'unsupported Linux release target' >&2; exit 2 ;;
esac
: "${RTAO_TEMP_ROOT:?record the task root}"
: "${CARGO_TARGET_DIR:?build under the task root}"
: "${TMPDIR:?scratch under the task root}"
source ci/release/pins.env
source ci/release/license.sh
dist="$RTAO_TEMP_ROOT/dist"
mkdir -p "$dist" "$CARGO_TARGET_DIR" "$TMPDIR"
rtao_copy_release_licenses "$dist"
test "$(getconf GNU_LIBC_VERSION)" = 'glibc 2.35'
test "$(git rev-parse HEAD)" = "$(git rev-parse "refs/tags/$tag^{commit}")"
test -z "$(git status --porcelain --untracked-files=normal)"
# A prepare dispatch must select --ref "$tag" as well as --field tag="$tag";
# GitHub provenance binds the triggering ref, not merely the checkout input.
test "${GITHUB_REF:?must run in the tag workflow}" = "refs/tags/$tag"
test "${GITHUB_SHA:?must run in the tag workflow}" = "$(git rev-parse HEAD)"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER='qemu-aarch64 -L /usr/aarch64-linux-gnu'
export CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
export CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++
export PKG_CONFIG_ALLOW_CROSS=1
export PKG_CONFIG_LIBDIR_aarch64_unknown_linux_gnu=/usr/lib/aarch64-linux-gnu/pkgconfig:/usr/share/pkgconfig
started=$(date +%s)
cargo build --locked --release -p rtao-cli --target "$target"
cargo test --locked -p rtao-cli --target "$target"
cargo check --locked -p rtao --all-features --target "$target"
cargo nextest run --locked --workspace --all-features --profile ci --target "$target"
binary="$CARGO_TARGET_DIR/$target/release/rtao"
if test "$mode" = native; then
  "$binary" --version
  "$binary" doctor --manifest-path Cargo.toml
else
  qemu-aarch64 -L /usr/aarch64-linux-gnu "$binary" --version
  qemu-aarch64 -L /usr/aarch64-linux-gnu "$binary" doctor --manifest-path Cargo.toml
fi
# All versioned GLIBC imports must stay at or below 2.35, including when the
# runner's own host libc is newer. GNU readelf works on both target ELF formats.
glibc_versions=$(readelf --version-info "$binary" | sed -n 's/.*Name: GLIBC_\([0-9][0-9.]*\).*/\1/p' | sort -Vu)
test -n "$glibc_versions"
maximum=$(printf '%s\n' "$glibc_versions" | tail -1)
test "$(printf '%s\n' "$maximum" 2.35 | sort -V | tail -1)" = 2.35
name="rtao-$tag-$target.tar.gz"
tar -czf "$dist/$name" -C "$(dirname "$binary")" rtao \
  -C "$dist" "${rtao_release_license_names[@]}"
jq -n \
  --arg tag "$tag" --arg commit "$(git rev-parse HEAD)" --arg tree "$(git rev-parse 'HEAD^{tree}')" \
  --arg rustc "$(rustc --version --verbose)" --arg cargo "$(cargo --version)" \
  --arg container "$RTAO_RELEASE_CONTAINER" --arg glibc_import_max "$maximum" \
  --arg target "$target" --arg mode "$mode" --arg name "$name" \
  --arg sha256 "$(sha256sum "$dist/$name" | cut -d' ' -f1)" \
  --arg packages "$(dpkg-query -W -f='${Package} ${Version}\n')" \
  --argjson elapsed_seconds "$(($(date +%s) - started))" --argjson target_bytes "$(du -sb "$CARGO_TARGET_DIR" | cut -f1)" \
  '{schema:"rtao-build-v1",tag:$tag,commit:$commit,tree:$tree,rustc:$rustc,cargo:$cargo,builder:"github-ubuntu-latest",container:$container,glibc_ceiling:"2.35",glibc_import_max:$glibc_import_max,checks_passed:true,packages:$packages,elapsed_seconds:$elapsed_seconds,target_bytes:$target_bytes,artifact:{name:$name,target:$target,mode:$mode,sha256:$sha256}}' \
  > "$dist/linux-$target-build.json"
