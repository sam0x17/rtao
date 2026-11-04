#!/usr/bin/env bash
# Offline checks of ci/runtime-adopt.sh's decision: recorded and simulated
# observations against the committed pins. No network, no build.
#
#   ci/runtime-adopt-test.sh
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
script=$root/ci/runtime-adopt.sh
work=$(mktemp -d "${RTAO_TEMP_ROOT:-${TMPDIR:-/tmp}}/runtime-adopt-test.XXXXXX")
trap 'rm -rf "$work"' EXIT

toml_get() {
  awk -v section="[$2]" -v key="$3" '
    /^\[/ { inside = ($0 == section); next }
    inside && $1 == key && $2 == "=" { value = $0; sub(/^[^=]*=[ \t]*/, "", value); gsub(/^"|"$/, "", value); print value; exit }
  ' "$1"
}

spec=$(toml_get "$root/upstream.toml" parity spec_version)
code=$(toml_get "$root/upstream.toml" networks.finney.metadata code_hash)
testnet_code=$(awk '/^\[\[runtimes\]\]/{s=""} $1=="spec"{s=$3} $1=="code_hash"{c=$3; gsub(/"/,"",c); last=s" "c} END{print last}' \
  "$root/vendor/metadata/testnet/index.toml")
testnet_spec=${testnet_code%% *}
testnet_code=${testnet_code##* }
next=$((spec + 1))
other=0x$(printf '%064d' 7)
merge=$(printf '%040d' 3)

# observation <finney spec> <finney code> <testnet spec> <testnet code> [sources] [branches]
observation() {
  jq -n --argjson fs "$1" --arg fc "$2" --argjson ts "$3" --arg tc "$4" \
    --argjson sources "${5:-{\}}" --argjson branches "${6:-{\}}" '
    def network($spec; $code; $n): if $spec == null then {error: "unreachable"}
      else {endpoint: "https://example.invalid", finalized: ("0x" + ("ab" * 32)), number: $n,
            spec: $spec, spec_name: "node-subtensor", transaction_version: 1, code: $code} end;
    {schema: 1, observed_at: "2026-11-04T00:00:00Z",
     finney: network($fs; $fc; 9400000), testnet: network($ts; $tc; 6000000),
     upstream: {branches: $branches, sources: $sources}}'
}

failures=0
# expect <name> <observation json> <jq predicate on the decision>
expect() {
  local name=$1 observed=$work/$1.observed.json decision=$work/$1.decision.json
  printf '%s\n' "$2" >"$observed"
  bash "$script" decide "$observed" "$decision" >/dev/null
  if jq -e "$3" "$decision" >/dev/null; then
    echo "ok   $name: $(jq -c '{action, source, id, deferred: (.deferred | map(.action)), upcoming: (.upcoming | map(.spec))}' "$decision")"
  else
    echo "FAIL $name: $(jq -c . "$decision")"
    failures=$((failures + 1))
  fi
}

release="{\"$next\": {\"release\": {\"tag\": \"v$next\", \"prerelease\": false, \"draft\": false}, \"main_merge\": \"$merge\"}}"
prerelease="{\"$next\": {\"release\": {\"tag\": \"v$next\", \"prerelease\": true, \"draft\": false}, \"main_merge\": \"$merge\"}}"
merge_only="{\"$next\": {\"release\": null, \"main_merge\": \"$merge\"}}"
nothing="{\"$next\": {\"release\": null, \"main_merge\": null}}"
ahead="{\"main\": {\"head\": \"$merge\", \"spec\": $next}, \"testnet\": {\"head\": \"$merge\", \"spec\": $testnet_spec}}"

expect current "$(observation "$spec" "$code" "$testnet_spec" "$testnet_code")" \
  '.action == "none" and .deferred == [] and .upcoming == [] and .unobserved == []'
expect finney-release "$(observation "$next" "$other" "$testnet_spec" "$testnet_code" "$release")" \
  ".action == \"adopt-finney\" and .source == \"v$next\" and .spec == $next and .id == \"finney-$next-${other:0:10}\""
expect finney-prerelease-uses-main-merge "$(observation "$next" "$other" "$testnet_spec" "$testnet_code" "$prerelease")" \
  ".action == \"adopt-finney\" and .source == \"branch:main@$merge\""
expect finney-main-merge "$(observation "$next" "$other" "$testnet_spec" "$testnet_code" "$merge_only")" \
  ".action == \"adopt-finney\" and .source == \"branch:main@$merge\""
expect finney-without-source "$(observation "$next" "$other" "$testnet_spec" "$testnet_code" "$nothing")" \
  '.action == "blocked" and .network == "finney" and (.reason | test("neither"))'
expect finney-code-changed "$(observation "$spec" "$other" "$testnet_spec" "$testnet_code")" \
  '.action == "blocked" and (.reason | test(":code changed"))'
expect finney-behind-pin "$(observation "$((spec - 1))" "$other" "$testnet_spec" "$testnet_code")" \
  '.action == "blocked" and (.reason | test("ahead of Finney"))'
expect testnet-new "$(observation "$spec" "$code" "$next" "$other")" \
  ".action == \"record-testnet\" and .spec == $next and .network == \"testnet\""
expect testnet-code-changed "$(observation "$spec" "$code" "$testnet_spec" "$other")" \
  '.action == "blocked" and .network == "testnet"'
expect both-new "$(observation "$next" "$other" "$next" "$other" "$release")" \
  '.action == "adopt-finney" and (.deferred | map(.action)) == ["record-testnet"]'
expect finney-blocked-testnet-new "$(observation "$spec" "$other" "$next" "$other")" \
  '.action == "record-testnet" and (.deferred | map(.action)) == ["blocked"]'
expect finney-unreachable "$(observation null "" "$testnet_spec" "$testnet_code")" \
  '.action == "none" and (.unobserved | length) == 1'
expect upstream-ahead "$(observation "$spec" "$code" "$testnet_spec" "$testnet_code" '{}' "$ahead")" \
  ".action == \"none\" and (.upcoming | map(.branch)) == [\"main\"] and .upcoming[0].spec == $next"
expect wrong-spec-name "$(observation "$spec" "$code" "$testnet_spec" "$testnet_code" | jq -c '.finney.spec_name = "other"')" \
  '.action == "blocked" and (.reason | test("spec_name"))'

if ((failures)); then
  echo "$failures decision checks failed" >&2
  exit 1
fi
echo "all decision checks passed"
