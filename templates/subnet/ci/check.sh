#!/usr/bin/env bash
set -euo pipefail
export PYO3_NO_PYTHON=1
: "${RTAO_TEMP_ROOT:?Record one absolute task-scoped root before running checks}"
[[ "$RTAO_TEMP_ROOT" = /* ]]
mkdir -p "$RTAO_TEMP_ROOT/tmp"
export TMPDIR="$RTAO_TEMP_ROOT/tmp"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$RTAO_TEMP_ROOT/build}"
rustc --version
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo build --locked -p subnet-validator -p subnet-miner
cargo test --locked -p subnet-e2e --test localnet --no-run
for package in subnet-validator subnet-miner; do
  "$CARGO_TARGET_DIR/debug/$package" --help > "$RTAO_TEMP_ROOT/$package-help.txt"
  cargo tree --locked -p "$package" --edges normal,build --prefix none --format '{p}' > "$RTAO_TEMP_ROOT/$package-tree.txt"
  if rg '^(sodiumoxide|libsodium-sys|openssl|openssl-sys|openssl-src|native-tls|fernet|ansible-vault|secp256k1-sys|aws-lc-rs|aws-lc-sys|cmake|hidapi|libloading|rayon) ' "$RTAO_TEMP_ROOT/$package-tree.txt"; then
    exit 1
  fi
done
"${RTAO_DOCTOR:-rtao}" doctor --manifest-path "$PWD/Cargo.toml"
if test "${1:-}" = native; then
  exit 0
fi
# Each value is obtained from Cargo's actual SDK source, never a floating image tag.
while IFS='=' read -r key value; do
  export "$key=$value"
done < <(bash ci/source.sh)
export COMPOSE_PROJECT_NAME="rtao-template-${GITHUB_RUN_ID:-$$}"
printf '%s\n' "$COMPOSE_PROJECT_NAME" > "$RTAO_TEMP_ROOT/compose-project.txt"
export NEURON_UID="$(id -u)" NEURON_GID="$(id -g)"
bash ci/build-images.sh
cleanup() {
  local template_exit_status="$?"
  docker compose logs --no-color --tail 1000 > "$RTAO_TEMP_ROOT/compose.log" 2>&1 || true
  if ! docker compose --profile neurons down --volumes --remove-orphans; then
    if test "$template_exit_status" -eq 0; then
      template_exit_status=1
    fi
  fi
  trap - EXIT
  exit "$template_exit_status"
}
trap cleanup EXIT
docker compose up -d localnet
cargo test --locked -p subnet-e2e --test localnet
