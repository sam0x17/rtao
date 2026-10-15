#!/usr/bin/env bash
set -euo pipefail
root=/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC
export PYO3_NO_PYTHON=1
export CARGO_TARGET_DIR="$root/build"
export TMPDIR="$root/tmp"
export CLIPPY_CONF_DIR="$root/upstream-quality/ir-keys-proof/clippy-empty"
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
cd "$root/upstream-quality/pr-3249"
[[ "$(git rev-parse HEAD)" == 0c4ed7585ae9d232577e53d45813f383e0ec2eec ]]
git diff --exit-code -- sdk/bittensor-core/src sdk/bittensor-core-py/src Cargo.lock
shasum -a 256 Cargo.lock
touch sdk/bittensor-core/src/lib.rs sdk/bittensor-core-py/src/lib.rs
cargo +1.89.0 test --locked --offline -p bittensor-core --no-default-features --test metadata_ir --test review_ir_independent
cargo +stable test --locked --offline -p bittensor-core --no-default-features --test metadata_ir --test review_ir_independent
cd "$root/upstream-quality/pr-3250"
[[ "$(git rev-parse HEAD)" == c0208a82ab2a17a91c87654f3d6ebc0dc887cd20 ]]
git diff --exit-code -- sdk/bittensor-core/src sdk/bittensor-core-py/src Cargo.lock
shasum -a 256 Cargo.lock
touch sdk/bittensor-core/src/lib.rs sdk/bittensor-core-py/src/lib.rs
cargo +1.89.0 test --locked --offline -p bittensor-core --no-default-features --test verify_strict --test password_environment --test review_keys_independent
cargo +stable test --locked --offline -p bittensor-core --no-default-features --test verify_strict --test password_environment --test review_keys_independent
