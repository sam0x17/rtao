#!/usr/bin/env bash
set -euo pipefail
cd /Users/sam/workspace/rtao/target/execute-plan.6Ro0PC/tmp/upstream-key-helpers-u10/repo
[[ "$1" == origin ]]
[[ "$2" == git@github.com:sam0x17/subtensor.git ]]
[[ "$(gh api user --jq .login)" == sam0x17 ]]
[[ -z "$(git status --porcelain)" ]]
head=$(git rev-parse HEAD)
while read -r local_ref local_sha remote_ref remote_sha; do
  [[ "$local_sha" == "$head" ]]
  [[ "$local_ref" == refs/heads/sdk-pure-key-helpers ]]
  [[ "$remote_ref" == "$local_ref" ]]
done
base=850f2755bb05a3851f948489f0ab27b0645cebce
git diff --name-only "$base" HEAD | sort > ../changed-paths.txt
cmp ../expected-paths.txt ../changed-paths.txt
git diff --check "$base" HEAD
# User authorization fixes sam0x17 as actor and keeps Python execution in CI.
# Validate the exact clean committed HEAD with scoped Rust checks.
{
  printf 'Committed review HEAD: %s\n' "$head"
  ../rust-gate.sh 2>&1
} | tee ../pre-push.log
