#!/usr/bin/env bash
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
[[ "$1" == origin ]]
[[ "$2" == git@github.com:sam0x17/subtensor.git ]]
[[ "$(gh api user --jq .login)" == sam0x17 ]]
[[ -z "$(git status --porcelain)" ]]
head=$(git rev-parse HEAD)
while read -r local_ref local_sha remote_ref remote_sha; do
  [[ "$local_sha" == "$head" ]]
  [[ "$local_ref" == refs/heads/sdk-portable-protocol || "$local_ref" == refs/heads/sdk-receipt-nested-failures || "$local_ref" == refs/heads/sdk-timelock-epoch-safety ]]
  [[ "$remote_ref" == "$local_ref" ]]
done
# User authorization fixes actor sam0x17 and keeps Python in CI. These Rust
# checks validate the exact clean committed HEAD; no hook bypass is supported.
/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC/upstream-filings/rust-gate.sh
