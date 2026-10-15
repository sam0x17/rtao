#!/usr/bin/env bash
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
export PYO3_NO_PYTHON=1
export CARGO_TARGET_DIR=/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC/build
export TMPDIR=/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC/tmp
export CLIPPY_CONF_DIR=/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC/upstream-filings/clippy-empty
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
# Sibling isolated clones share this bounded target cache. Force the local
# packages to rebuild so Cargo cannot reuse another clone timestamp result.
touch sdk/bittensor-core/src/lib.rs sdk/bittensor-core-py/src/lib.rs
cargo +1.89.0 fmt -p bittensor-core --check
cargo +1.89.0 test --locked -p bittensor-core --all-features --lib
if [[ -d sdk/bittensor-core/src/protocol ]]; then
  cargo +1.89.0 test --locked -p bittensor-core --no-default-features --test protocol_ops --test portable_fee_dispatch --test value_navigation
  if [[ -f sdk/bittensor-core/tests/receipt_parity.rs ]]; then
    cargo +1.89.0 test --locked -p bittensor-core --no-default-features --test receipt_parity
  fi
else
  cargo +1.89.0 test --locked -p bittensor-core --no-default-features --test epoch_safety --test explicit_commit_clock
fi
cargo +1.89.0 clippy --locked -p bittensor-core -p bittensor-core-py --all-features --all-targets -- -D warnings
cargo +1.89 check --locked -p bittensor-core --no-default-features --target wasm32-unknown-unknown
if git diff --name-only 850f2755bb05a3851f948489f0ab27b0645cebce HEAD | rg -v '^(Cargo.lock|sdk/bittensor-core/)'; then
  echo 'unexpected committed upstream scope' >&2
  exit 1
fi
git diff --check
