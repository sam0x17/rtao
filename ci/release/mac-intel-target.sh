#!/usr/bin/env bash
set -euo pipefail

# Install target stdlib under the recorded task root, never under the user's
# rustup/toolchain home. This does not select a default toolchain or install Rosetta.
: "${RTAO_TEMP_ROOT:?record the task root}"
source ci/release/pins.env
case "$(rustc --version)" in "rustc $RTAO_MAC_INTEL_RUST_VERSION ("*) ;; *)
  echo 'Intel target archive pin differs from the current Rust compiler' >&2; exit 1 ;;
esac
scratch="$RTAO_TEMP_ROOT/mac-intel-target"
test ! -e "$scratch"
mkdir -p "$scratch"
name="rust-std-$RTAO_MAC_INTEL_RUST_VERSION-x86_64-apple-darwin"
curl --fail --location --proto '=https' --tlsv1.2 \
  "https://static.rust-lang.org/dist/$name.tar.xz" -o "$scratch/rust-std.tar.xz"
printf '%s  %s\n' "$RTAO_MAC_INTEL_RUST_STD_SHA256" "$scratch/rust-std.tar.xz" | shasum -a 256 --check
tar -C "$scratch" -xf "$scratch/rust-std.tar.xz"
"$scratch/$name/install.sh" --prefix="$scratch/sysroot" --disable-ldconfig
printf 'export RTAO_MAC_INTEL_SYSROOT=%q\n' "$scratch/sysroot"
