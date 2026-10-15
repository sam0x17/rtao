#!/usr/bin/env bash
# User authorization fixes sam0x17, these two branches and Rust-only local gates.
set -euo pipefail
unicode_evidence=/Users/sam/workspace/rtao/docs/upstream/validation/quality-unicode-publication
[[ "$1" == origin ]]
[[ "$2" == git@github.com:sam0x17/subtensor.git ]]
[[ "$(gh api user --jq .login)" == sam0x17 ]]
[[ -z "$(git status --porcelain)" ]]
unicode_head=$(git rev-parse HEAD)
unicode_seen=false
while read -r unicode_local_ref unicode_local_sha unicode_remote_ref unicode_remote_sha; do
    [[ "$unicode_seen" == false ]]
    [[ "$unicode_local_ref" == HEAD ]]
    [[ "$unicode_local_sha" == "$unicode_head" ]]
    case "$unicode_remote_ref:$unicode_remote_sha" in
        refs/heads/sdk-portable-protocol:9e926d1f2f4943f11a23ef13ff517e7857e9cc27) unicode_pr=3246 ;;
        refs/heads/sdk-receipt-nested-failures:aadeb06bb517b24428f4d7f64a351ccc8765e502) unicode_pr=3247 ;;
        *) exit 1 ;;
    esac
    [[ "$unicode_head" == "$(cat "$unicode_evidence/pr-$unicode_pr-head.txt")" ]]
    unicode_seen=true
done
[[ "$unicode_seen" == true ]]
unicode_ssh_status=0
ssh -T git@github.com > "$unicode_evidence/pr-$unicode_pr-ssh.txt" 2>&1 || unicode_ssh_status=$?
[[ "$unicode_ssh_status" == 1 ]]
rg -q '^Hi sam0x17! You.ve successfully authenticated, but GitHub does not provide shell access\.$' "$unicode_evidence/pr-$unicode_pr-ssh.txt"
shasum -a 256 /Users/sam/workspace/rtao/docs/upstream/validation/quality-unicode-outgoing-gate.sh "$unicode_evidence/source-guard.rb" "$unicode_evidence/result-guard.rb" > "$unicode_evidence/pr-$unicode_pr-gate-sha256.txt"
bash /Users/sam/workspace/rtao/docs/upstream/validation/quality-unicode-outgoing-gate.sh "$unicode_pr" 2>&1 | tee "$unicode_evidence/pr-$unicode_pr-pre-push.log"
