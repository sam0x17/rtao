#!/usr/bin/env bash
# User authorization selects sam0x17 and confines Python runtime checks to CI.
set -euo pipefail
[[ "$(gh api user --jq .login)" == sam0x17 ]]
case "${2-}" in
    git@github.com:sam0x17/subtensor.git|https://github.com/sam0x17/subtensor.git) ;;
    *) exit 1 ;;
esac
rtao_pushed_sha=$(git rev-parse HEAD)
[[ -z "$(git status --porcelain)" ]]
rtao_seen_ref=false
while read -r rtao_local_ref rtao_local_sha rtao_remote_ref rtao_remote_sha; do
    [[ "$rtao_local_sha" == "$rtao_pushed_sha" ]]
    [[ "$rtao_remote_ref" == refs/heads/sdk-pure-key-helpers ]]
    rtao_seen_ref=true
done
[[ "$rtao_seen_ref" == true ]]
proof=/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC/upstream-quality/u10-outgoing-"$rtao_pushed_sha"
mkdir -p "$proof"
bash /Users/sam/workspace/rtao/docs/upstream/validation/quality-u10-outgoing-gate.sh 2>&1 | tee "$proof/pre-push.log"
