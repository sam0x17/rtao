#!/usr/bin/env bash
# Compile and test generated client source on a disposable hosted runner.
set -euo pipefail

test "${GITHUB_ACTIONS:-}" = true
test "${RUNNER_OS:-}" = Linux
test "${RUNNER_ARCH:-}" = X64
test "${RUNNER_ENVIRONMENT:-}" = github-hosted
template_options=(check)
case "${1:-}" in
  '') test "$#" -eq 0 ;;
  --localnet) test "$#" -eq 1; template_options+=(--localnet) ;;
  *) echo 'usage: bash ci/template-check.sh [--localnet]' >&2; exit 2 ;;
esac
: "${RTAO_TEMP_ROOT:?hosted task root is required}"
: "${CARGO_TARGET_DIR:?reuse the hosted gate target directory}"
: "${TMPDIR:?use the hosted task scratch directory}"
[[ "$RTAO_TEMP_ROOT" = /* && "/$RTAO_TEMP_ROOT/" != */../* ]]
test "$CARGO_TARGET_DIR" = "$RTAO_TEMP_ROOT/build"
test "$TMPDIR" = "$RTAO_TEMP_ROOT/tmp"
# Use the gate's installed toolchain across both workspaces. MSRV is a separate
# hosted gate; this check must not install another compiler or duplicate caches.
RUSTUP_TOOLCHAIN="$(rustup show active-toolchain | cut -d' ' -f1)"
export RUSTUP_TOOLCHAIN

template_tool_root="$RTAO_TEMP_ROOT/ci/template-tool"
test ! -e "$template_tool_root"
test ! -L "$template_tool_root"
mkdir -p "$template_tool_root" "$TMPDIR"
cleanup_template_tool() {
  # Only these exact downloaded/extracted files are disposable; the generated
  # project's separate ownership/live-dependency audit is handled by xtask.
  local template_tool_status="$?"
  if ! rm -f -- "$template_tool_root/cargo-generate.tar.gz" "$template_tool_root/cargo-generate"; then
    if test "$template_tool_status" -eq 0; then template_tool_status=1; fi
  fi
  if ! rmdir -- "$template_tool_root"; then
    if test "$template_tool_status" -eq 0; then template_tool_status=1; fi
  fi
  trap - EXIT
  exit "$template_tool_status"
}
trap cleanup_template_tool EXIT

curl --fail --location --proto '=https' --tlsv1.2 \
  --connect-timeout 15 --max-time 120 --retry 2 --max-filesize 6797913 \
  'https://github.com/cargo-generate/cargo-generate/releases/download/v0.23.12/cargo-generate-v0.23.12-x86_64-unknown-linux-gnu.tar.gz' \
  -o "$template_tool_root/cargo-generate.tar.gz"
test "$(stat -c %s "$template_tool_root/cargo-generate.tar.gz")" = 6797913
printf '%s  %s\n' \
  20e51f445322e39a9d3aa1a383cb8b02c6cc5993852d4dd43c9d08f52a6147f3 \
  "$template_tool_root/cargo-generate.tar.gz" | sha256sum --check --strict
tar -xOf "$template_tool_root/cargo-generate.tar.gz" cargo-generate > "$template_tool_root/cargo-generate"
chmod 755 "$template_tool_root/cargo-generate"
export RTAO_CARGO_GENERATE="$template_tool_root/cargo-generate"
test "$("$RTAO_CARGO_GENERATE" --version)" = 'cargo generate 0.23.12'

# xtask bounds the complete command sequence and audits generated-root cleanup
# after errors as well as success. All Cargo calls reuse CARGO_TARGET_DIR.
cargo run --locked -p xtask -- template "${template_options[@]}"
