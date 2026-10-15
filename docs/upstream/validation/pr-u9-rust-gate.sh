#!/usr/bin/env bash
set -euo pipefail
cd /Users/sam/workspace/rtao/target/execute-plan.6Ro0PC/tmp/upstream-metadata-u9/repo
export PYO3_NO_PYTHON=1
export CARGO_TARGET_DIR=/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC/build
export TMPDIR=/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC/tmp/upstream-metadata-u9/tmp
export CLIPPY_CONF_DIR=/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC/tmp/upstream-metadata-u9/clippy-empty
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
touch sdk/bittensor-core/src/lib.rs sdk/bittensor-core-py/src/lib.rs
cargo +1.89.0 fmt -p bittensor-core -p bittensor-core-py --check
cargo +1.89.0 test --locked -p bittensor-core --no-default-features --test metadata_ir
cargo +1.89.0 test --locked -p bittensor-core --all-features --lib
cargo +1.89.0 clippy --locked -p bittensor-core -p bittensor-core-py --all-features --all-targets -- -D warnings
cargo +1.89 check --locked -p bittensor-core --no-default-features --target wasm32-unknown-unknown
git diff --check
