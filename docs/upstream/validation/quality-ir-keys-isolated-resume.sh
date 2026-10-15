#!/usr/bin/env bash
set -euo pipefail
root=/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC
export PYO3_NO_PYTHON=1
export CARGO_TARGET_DIR="$root/build"
export TMPDIR="$root/tmp"
export CLIPPY_CONF_DIR="$root/upstream-quality/ir-keys-proof/clippy-empty"
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0

cd "$root/upstream-quality/pr-3249"
export RUSTC_WORKSPACE_WRAPPER="$root/upstream-quality/ir-keys-proof/rustc-u9.sh"
[[ "$(git rev-parse HEAD)" == 0c4ed7585ae9d232577e53d45813f383e0ec2eec ]]
cargo +1.89 check --locked --offline -p bittensor-core --no-default-features --target wasm32-unknown-unknown
cargo +stable check --locked --offline -p bittensor-core --no-default-features --target wasm32-unknown-unknown
cargo +1.89.0 fmt -p bittensor-core -p bittensor-core-py --check
git diff --check
git diff --exit-code -- sdk/bittensor-core/src sdk/bittensor-core-py/src Cargo.lock

cd "$root/upstream-quality/pr-3250"
export RUSTC_WORKSPACE_WRAPPER="$root/upstream-quality/ir-keys-proof/rustc-u10.sh"
[[ "$(git rev-parse HEAD)" == c0208a82ab2a17a91c87654f3d6ebc0dc887cd20 ]]
git apply --check --reverse ../ir-keys-proof/u10-error-path.patch
git apply --reverse ../ir-keys-proof/u10-error-path.patch
restore_u10() { git apply ../ir-keys-proof/u10-error-path.patch; }
trap restore_u10 EXIT
git diff --exit-code -- sdk/bittensor-core/src sdk/bittensor-core-py/src sdk/bittensor-core/tests/password_environment.rs Cargo.lock
shasum -a 256 Cargo.lock
touch sdk/bittensor-core/src/lib.rs sdk/bittensor-core-py/src/lib.rs
cargo +1.89.0 test --locked --offline -p bittensor-core --no-default-features --test verify_strict --test password_environment --test review_keys_independent
cargo +stable test --locked --offline -p bittensor-core --no-default-features --test verify_strict --test password_environment --test review_keys_independent
restore_u10
trap - EXIT
touch sdk/bittensor-core/src/lib.rs sdk/bittensor-core-py/src/lib.rs
cargo +1.89.0 test --locked --offline -p bittensor-core --no-default-features --test verify_strict --test password_environment --test review_keys_independent
cargo +stable test --locked --offline -p bittensor-core --no-default-features --test verify_strict --test password_environment --test review_keys_independent
cargo +1.89.0 test --locked --offline -p bittensor-core --all-features --lib
cargo +stable test --locked --offline -p bittensor-core --all-features --lib
cargo +1.89.0 clippy --locked --offline -p bittensor-core -p bittensor-core-py --all-features --all-targets -- -D warnings
# Current strict Clippy's two unchanged base-file findings are recorded separately.
cargo +1.89 check --locked --offline -p bittensor-core --no-default-features --target wasm32-unknown-unknown
cargo +stable check --locked --offline -p bittensor-core --no-default-features --target wasm32-unknown-unknown
cargo +1.89.0 fmt -p bittensor-core -p bittensor-core-py --check
git diff --check
git diff --exit-code -- Cargo.lock

cd /Users/sam/workspace/rtao
unset CLIPPY_CONF_DIR RUSTC_WORKSPACE_WRAPPER
cargo +1.89.0 test --locked --offline -p rtao-core-ext --test password_environment
cargo +stable test --locked --offline -p rtao-core-ext --test password_environment
cargo +1.89.0 clippy --locked --offline -p rtao-core-ext --all-features --all-targets -- -D warnings
cargo +stable clippy --locked --offline -p rtao-core-ext --all-features --all-targets -- -D warnings
cargo +1.89.0 fmt -p rtao-core-ext --check
