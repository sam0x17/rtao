#!/usr/bin/env bash
set -euo pipefail
root=/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC
export PYO3_NO_PYTHON=1
export CARGO_TARGET_DIR="$root/build"
export TMPDIR="$root/tmp"
export CLIPPY_CONF_DIR="$root/upstream-quality/ir-keys-proof/clippy-empty"
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
cd "$root/upstream-quality/pr-3249"
cargo +1.89.0 test --locked --offline -p bittensor-core --all-features --lib
cargo +1.89.0 clippy --locked --offline -p bittensor-core -p bittensor-core-py --all-features --all-targets -- -D warnings
cargo +stable clippy --locked --offline -p bittensor-core -p bittensor-core-py --all-features --all-targets -- -D warnings
cargo +1.89 check --locked --offline -p bittensor-core --no-default-features --target wasm32-unknown-unknown
cargo +stable check --locked --offline -p bittensor-core --no-default-features --target wasm32-unknown-unknown
cargo +1.89.0 fmt -p bittensor-core -p bittensor-core-py --check
git diff --check
git diff --exit-code -- sdk/bittensor-core/src sdk/bittensor-core-py/src Cargo.lock
cd "$root/upstream-quality/pr-3250"
cargo +1.89.0 test --locked --offline -p bittensor-core --no-default-features --test verify_strict --test password_environment --test review_keys_independent
cargo +stable test --locked --offline -p bittensor-core --no-default-features --test verify_strict --test password_environment --test review_keys_independent
cargo +1.89.0 test --locked --offline -p bittensor-core --all-features --lib
cargo +stable test --locked --offline -p bittensor-core --all-features --lib
cargo +1.89.0 clippy --locked --offline -p bittensor-core -p bittensor-core-py --all-features --all-targets -- -D warnings
cargo +stable clippy --locked --offline -p bittensor-core -p bittensor-core-py --all-features --all-targets -- -D warnings
cargo +1.89 check --locked --offline -p bittensor-core --no-default-features --target wasm32-unknown-unknown
cargo +stable check --locked --offline -p bittensor-core --no-default-features --target wasm32-unknown-unknown
cargo +1.89.0 fmt -p bittensor-core -p bittensor-core-py --check
git diff --check
git diff --exit-code -- Cargo.lock
cd /Users/sam/workspace/rtao
unset CLIPPY_CONF_DIR
cargo +1.89.0 test --locked --offline -p rtao-core-ext --test password_environment
cargo +stable test --locked --offline -p rtao-core-ext --test password_environment
cargo +1.89.0 clippy --locked --offline -p rtao-core-ext --all-features --all-targets -- -D warnings
cargo +stable clippy --locked --offline -p rtao-core-ext --all-features --all-targets -- -D warnings
cargo +1.89.0 fmt -p rtao-core-ext --check
