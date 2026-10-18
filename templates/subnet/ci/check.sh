#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
  ''|native|--native-only) template_localnet=false ;;
  --localnet) template_localnet=true ;;
  *) echo 'usage: bash ci/check.sh [--localnet]' >&2; exit 2 ;;
esac
test "$#" -le 1
export PYO3_NO_PYTHON=1
: "${RTAO_TEMP_ROOT:?Record one absolute task-scoped root before running checks}"
: "${CARGO_TARGET_DIR:?Reuse the hosted job target directory}"
[[ "$RTAO_TEMP_ROOT" = /* && "$CARGO_TARGET_DIR" = /* ]]
[[ "/$RTAO_TEMP_ROOT/" != */../* && "/$CARGO_TARGET_DIR/" != */../* ]]
mkdir -p "$RTAO_TEMP_ROOT/tmp"
export TMPDIR="$RTAO_TEMP_ROOT/tmp"
export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0
rustc --version
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
# Each selection gets its own feature resolution. Selecting the coldkey owner
# beside a neuron would enable core/host in that neuron's actual build graph.
for package in subnet-validator subnet-miner subnet-owner-tool; do
  cargo build --locked -p "$package"
  cargo test --locked -p "$package"
done
# Compile the live harness for downstream API compatibility without starting it.
cargo test --locked -p subnet-e2e --test localnet --no-run
"$CARGO_TARGET_DIR/debug/subnet-owner-tool" --help > "$RTAO_TEMP_ROOT/subnet-owner-tool-help.txt"
for package in subnet-validator subnet-miner; do
  "$CARGO_TARGET_DIR/debug/$package" --help > "$RTAO_TEMP_ROOT/$package-help.txt"
  cargo tree --locked -p "$package" --edges normal,build --prefix none --format '{p}' > "$RTAO_TEMP_ROOT/$package-tree.txt"
  if rg '^(sodiumoxide|libsodium-sys|openssl|openssl-sys|openssl-src|native-tls|fernet|ansible-vault|secp256k1-sys|aws-lc-rs|aws-lc-sys|cmake|hidapi|libloading|rayon) ' "$RTAO_TEMP_ROOT/$package-tree.txt"; then
    exit 1
  fi
done
"${RTAO_DOCTOR:-rtao}" doctor --manifest-path "$PWD/Cargo.toml"
if test "$template_localnet" = false; then
  exit 0
fi
# Each value is obtained from Cargo's actual SDK source, never a floating image tag.
while IFS='=' read -r key value; do
  export "$key=$value"
done < <(bash ci/source.sh)
export COMPOSE_PROJECT_NAME="rtao-template-${GITHUB_RUN_ID:-$$}"
printf '%s\n' "$COMPOSE_PROJECT_NAME" > "$RTAO_TEMP_ROOT/compose-project.txt"
export NEURON_UID="$(id -u)" NEURON_GID="$(id -g)"
cleanup() {
  local template_exit_status="$?"
  # The neurons live in a compose profile; name each service so its logs are
  # kept even though the profile is inactive in this shell.
  local service
  for service in localnet validator miner; do
    docker compose --profile neurons logs --no-color --timestamps --tail 1000 "$service" 2>&1 \
      | tail -c 1048576 > "$RTAO_TEMP_ROOT/compose-$service.log" || true
  done
  if ! docker compose --profile neurons down --volumes --remove-orphans; then
    if test "$template_exit_status" -eq 0; then
      template_exit_status=1
    fi
  fi
  trap - EXIT
  exit "$template_exit_status"
}
trap cleanup EXIT
bash ci/build-images.sh
docker compose up -d localnet
cargo test --locked -p subnet-e2e --test localnet
