#!/usr/bin/env bash
set -euo pipefail
unicode_pr="$1"
unicode_root=/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC
unicode_evidence=/Users/sam/workspace/rtao/docs/upstream/validation/quality-unicode-publication
unicode_checkout="$unicode_root/upstream-quality/pr-$unicode_pr"
unicode_scratch="$unicode_root/upstream-quality/unicode-publication/pr-$unicode_pr"
cd "$unicode_checkout"
unicode_head=$(git rev-parse HEAD)
[[ -z "$(git status --porcelain)" ]]
ruby "$unicode_evidence/source-guard.rb" "$unicode_pr"
mkdir -p "$unicode_scratch/clippy" "$unicode_scratch/lint"
export TMPDIR="$unicode_root/tmp" PYO3_NO_PYTHON=1
export CARGO_TARGET_DIR="$unicode_scratch/functional"
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
export CLIPPY_CONF_DIR="$unicode_scratch/clippy"
export RUSTC_WORKSPACE_WRAPPER="$unicode_scratch/rustc.sh"
[[ -x "$RUSTC_WORKSPACE_WRAPPER" ]]
[[ "$(shasum -a 256 "$RUSTC_WORKSPACE_WRAPPER" | cut -d ' ' -f 1)" == 7f812349eb1017d2122230d2daf3592fc5952efee881ffc42585d876687a97ee ]]
: > "$CLIPPY_CONF_DIR/clippy.toml"
unicode_run() {
    local stage="$1"
    shift
    printf 'STAGE %s\nCOMMAND' "$stage"
    printf ' %q' "$@"
    printf '\n'
    "$@" 2>&1 | tee "$unicode_evidence/pr-$unicode_pr-$stage.log"
}
git rev-parse HEAD
git var GIT_AUTHOR_IDENT
shasum -a 256 Cargo.lock sdk/bittensor-core/src/protocol/dispatch_error.rs sdk/bittensor-core/src/protocol/python_printable.rs sdk/bittensor-core/tests/portable_fee_dispatch.rs sdk/bittensor-core/Cargo.toml
rustc +1.89.0 --version
rustc +stable --version
unicode_run fmt cargo +1.89.0 fmt -p bittensor-core -p bittensor-core-py --check
for unicode_toolchain in 1.89.0 stable; do
    unicode_tests=(--test protocol_ops --test portable_fee_dispatch --test value_navigation)
    if [[ "$unicode_pr" == 3247 ]]; then
        unicode_tests+=(--test receipt_parity)
    fi
    unicode_run "$unicode_toolchain-portable" cargo "+$unicode_toolchain" test --locked --offline -p bittensor-core --no-default-features "${unicode_tests[@]}"
    unicode_run "$unicode_toolchain-lib" cargo "+$unicode_toolchain" test --locked --offline -p bittensor-core --all-features --lib
    # The binding defines no Rust unit tests. Compile every binding target with
    # abi3 and without discovering or executing a local Python interpreter.
    unicode_run "$unicode_toolchain-binding" cargo "+$unicode_toolchain" check --locked --offline -p bittensor-core-py --all-features --all-targets
done
# Clippy supplies its own workspace wrapper; this real per-checkout target is
# required to isolate lint artifacts from other committed source trees.
unicode_run clippy-msrv env CARGO_TARGET_DIR="$unicode_scratch/lint" cargo +1.89.0 clippy --locked --offline -p bittensor-core -p bittensor-core-py --all-features --all-targets -- -D warnings
unicode_run wasm-msrv cargo +1.89 check --locked --offline -p bittensor-core --no-default-features --target wasm32-unknown-unknown
unicode_run wasm-current cargo +stable check --locked --offline -p bittensor-core --no-default-features --target wasm32-unknown-unknown
ruby "$unicode_evidence/source-guard.rb" "$unicode_pr"
git diff --check
[[ -z "$(git status --porcelain)" ]]
[[ "$(git rev-parse HEAD)" == "$unicode_head" ]]
ruby "$unicode_evidence/result-guard.rb" "$unicode_pr"
printf 'Clean committed HEAD gate passed: PR %s %s\n' "$unicode_pr" "$unicode_head"
