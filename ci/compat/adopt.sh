#!/usr/bin/env bash
# Adopt the attested compatibility reports of one pr-gates validate_compat run:
# download them, verify each report's GitHub artifact attestation names this
# repository's pr-gates workflow, that run and its commit, then record them as
# the evidence ledger and regenerate compat.json and COMPAT.md.
set -euo pipefail
run_id=${1:?usage: ci/compat/adopt.sh <run-id>}
[[ "$run_id" =~ ^[0-9]+$ ]] || { echo "run id must be numeric" >&2; exit 2; }
: "${RTAO_TEMP_ROOT:?set RTAO_TEMP_ROOT to the recorded task-scoped temporary root}"
repo=sam0x17/rtao
workflow="$repo/.github/workflows/pr.yml"
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)

run=$(gh run view "$run_id" --repo "$repo" --json headSha,workflowName,event,status)
jq -e '.workflowName == "pr-gates" and .event == "workflow_dispatch" and .status == "completed"' \
  <<< "$run" > /dev/null || { echo "run $run_id is not a completed pr-gates dispatch" >&2; exit 1; }
head=$(jq -er .headSha <<< "$run")
[[ "$head" =~ ^[0-9a-f]{40}$ ]]

work="$RTAO_TEMP_ROOT/compat-adopt-$run_id"
if test -e "$work"; then
  echo "$work already exists; remove it or choose another task root" >&2
  exit 1
fi
reports="$work/reports"
verified="$work/attestations"
mkdir -p "$reports" "$verified"
gh run download "$run_id" --repo "$repo" --name "compat-validation-$head" --dir "$reports"

shopt -s nullglob
files=("$reports"/*.json)
test "${#files[@]}" -gt 0 || { echo "run $run_id uploaded no reports" >&2; exit 1; }
url="https://github.com/$repo/actions/runs/$run_id"
for file in "${files[@]}"; do
  name=$(basename "$file")
  jq -e --arg url "$url" --arg head "$head" '.run.url == $url and .run.head_commit == $head' \
    "$file" > /dev/null || { echo "$name was not produced by run $run_id at $head" >&2; exit 1; }
  gh attestation verify "$file" --repo "$repo" --signer-workflow "$workflow" \
    --source-digest "$head" --deny-self-hosted-runners --format json > "$verified/$name"
  # Every attestation for these bytes must come from this run.
  jq -e --arg run "$url/" \
    'length > 0 and all(.[]; .verificationResult.signature.certificate.runInvocationURI | startswith($run))' \
    "$verified/$name" > /dev/null || { echo "$name attestation names another run" >&2; exit 1; }
  echo "verified attestation for $name"
done

cd "$root"
cargo run --locked -p xtask -- compat adopt --dir "$reports"
cargo run --locked -p xtask -- compat
echo "Review docs/compat/validation.json, docs/compat/reports, compat.json and COMPAT.md, then commit them."
