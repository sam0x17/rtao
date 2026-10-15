#!/usr/bin/env bash
# Validate the clean outgoing correction; local Python execution remains forbidden.
set -euo pipefail
rtao_review_checkout=$(git rev-parse --show-toplevel)
rtao_outgoing_sha=$(git rev-parse HEAD)
rtao_submitted_sha=c0208a82ab2a17a91c87654f3d6ebc0dc887cd20
[[ -z "$(git status --porcelain)" ]]
git merge-base --is-ancestor "$rtao_submitted_sha" "$rtao_outgoing_sha"
[[ "$rtao_outgoing_sha" != "$rtao_submitted_sha" ]]

export RTAO_TEMP_ROOT=/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC
rtao_hook_proof="$RTAO_TEMP_ROOT/upstream-quality/u10-outgoing-$rtao_outgoing_sha"
mkdir -p "$rtao_hook_proof/clippy"
cat > "$rtao_hook_proof/expected-paths.txt" <<'EOF'
sdk/bittensor-core/src/keyfile_env.rs
sdk/bittensor-core/src/keyfiles/mod.rs
sdk/bittensor-core/tests/password_environment.rs
EOF
git diff --name-only "$rtao_submitted_sha" "$rtao_outgoing_sha" | LC_ALL=C sort > "$rtao_hook_proof/actual-paths.txt"
cmp "$rtao_hook_proof/expected-paths.txt" "$rtao_hook_proof/actual-paths.txt"
git diff --check "$rtao_submitted_sha" "$rtao_outgoing_sha"
git diff --exit-code "$rtao_submitted_sha" "$rtao_outgoing_sha" -- Cargo.lock

# Cargo uses the wrapper path in workspace artifact hashes. This isolates the
# committed source from other review worktrees sharing the dependency cache.
cat > "$rtao_hook_proof/rustc.sh" <<'EOF'
#!/bin/sh
exec "$@"
EOF
chmod +x "$rtao_hook_proof/rustc.sh"
: > "$rtao_hook_proof/clippy/clippy.toml"
export RUSTC_WORKSPACE_WRAPPER="$rtao_hook_proof/rustc.sh"
export CLIPPY_CONF_DIR="$rtao_hook_proof/clippy"
export CARGO_TARGET_DIR="$RTAO_TEMP_ROOT/build"
export TMPDIR="$RTAO_TEMP_ROOT/tmp"
export PYO3_NO_PYTHON=1
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
cd "$rtao_review_checkout"
touch sdk/bittensor-core/src/lib.rs sdk/bittensor-core-py/src/lib.rs
git rev-parse HEAD
shasum -a 256 Cargo.lock
cargo +1.89.0 fmt -p bittensor-core -p bittensor-core-py --check
for rtao_toolchain in 1.89.0 stable; do
    cargo "+$rtao_toolchain" test --locked --offline -p bittensor-core --no-default-features --test verify_strict --test password_environment
    cargo "+$rtao_toolchain" test --locked --offline -p bittensor-core --all-features --lib
done
# Use upstream's pinned CI toolchain for strict Clippy. The separate stable
# diagnostic has two unchanged base-file lints documented in quality-ir-keys.md.
rtao_u10_lint_target="$RTAO_TEMP_ROOT/upstream-quality/lint/ir-keys-u10"
mkdir -p "$rtao_u10_lint_target"
# Clippy supplies its own workspace wrapper, so a separate target is required
# for source isolation even though functional test artifacts use our wrapper.
CARGO_TARGET_DIR="$rtao_u10_lint_target" cargo +1.89.0 clippy --locked --offline -p bittensor-core -p bittensor-core-py --all-features --all-targets -- -D warnings
cargo +1.89 check --locked --offline -p bittensor-core --no-default-features --target wasm32-unknown-unknown
cargo +stable check --locked --offline -p bittensor-core --no-default-features --target wasm32-unknown-unknown
git diff --check
[[ -z "$(git status --porcelain)" ]]
[[ "$(git rev-parse HEAD)" == "$rtao_outgoing_sha" ]]
