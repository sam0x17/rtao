#!/usr/bin/env bash
# Offline checks of ci/runtime-adopt.sh's decision (recorded and simulated
# observations against the committed pins, witness corroboration, backoff),
# its live-evidence verdict and its backoff bookkeeping. No network, no build.
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
# Upcoming means above every running runtime, and the last testnet row may be
# ahead of the parity pin (D-162), so the simulated branch declares one more.
upcoming=$(( (testnet_spec > spec ? testnet_spec : spec) + 1 ))
ahead="{\"main\": {\"head\": \"$merge\", \"spec\": $upcoming}, \"testnet\": {\"head\": \"$merge\", \"spec\": $testnet_spec}}"

expect current "$(observation "$spec" "$code" "$testnet_spec" "$testnet_code")" \
  '.action == "none" and .deferred == [] and .upcoming == [] and .unobserved == []'
expect finney-release "$(observation "$next" "$other" "$testnet_spec" "$testnet_code" "$release")" \
  ".action == \"adopt-finney\" and .source == \"v$next\" and .spec == $next and .id == \"finney-$next-${other:0:10}\""
expect finney-prerelease-blocked "$(observation "$next" "$other" "$testnet_spec" "$testnet_code" "$prerelease")" \
  '.action == "blocked" and .network == "finney" and (.reason | test("prerelease"))'
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
  ".action == \"none\" and (.upcoming | map(.branch)) == [\"main\"] and .upcoming[0].spec == $upcoming"
expect wrong-spec-name "$(observation "$spec" "$code" "$testnet_spec" "$testnet_code" | jq -c '.finney.spec_name = "other"')" \
  '.action == "blocked" and (.reason | test("spec_name"))'

# ── witnesses and backoff ─────────────────────────────────────────────
genesis=$(toml_get "$root/upstream.toml" networks.finney genesis)
# witness <endpoint> <spec> <code> [genesis] [canonical]: one witness row;
# "error" for none; canonical false when it finalized another block there.
witness() {
  if [[ "$2" == error ]]; then
    jq -cn --arg e "$1" '{endpoint: $e, error: "no complete runtime at the block"}'
  else
    jq -cn --arg e "$1" --argjson s "$2" --arg c "$3" --arg g "${4:-$genesis}" --argjson canonical "${5:-true}" \
      '{endpoint: $e, genesis: $g, spec: $s, spec_name: "node-subtensor", code: $c, canonical: $canonical}'
  fi
}
# witnessed <observation> <witness rows...>: Finney seen by these witnesses.
witnessed() {
  local observed=$1
  shift
  jq -c --argjson rows "$(printf '%s\n' "$@" | jq -s .)" --arg g "$genesis" \
    '.finney.genesis = $g | .finney.witnesses = $rows' <<<"$observed"
}
fresh=$(observation "$next" "$other" "$testnet_spec" "$testnet_code" "$release")
id=finney-$next-${other:0:10}
agree=$(witness https://witness.invalid "$next" "$other")
expect no-new-spec-witnessed "$(witnessed "$(observation "$spec" "$code" "$testnet_spec" "$testnet_code")" "$(witness https://witness.invalid "$spec" "$code")")" \
  '.action == "none" and .unobserved == []'
expect new-spec-corroborated "$(witnessed "$fresh" "$agree" "$(witness https://down.invalid error)")" \
  ".action == \"adopt-finney\" and .witnesses == [\"https://witness.invalid\"]"
expect new-spec-witness-disagrees "$(witnessed "$fresh" "$agree" "$(witness https://liar.invalid "$next" "$code")")" \
  '.action == "blocked" and .evidence == "inconsistent" and (.reason | test("disagree.*liar"))'
expect new-spec-other-genesis "$(witnessed "$fresh" "$(witness https://fork.invalid "$next" "$other" "0x$(printf '%064d' 9)")")" \
  '.action == "blocked" and (.reason | test("another genesis"))'
expect new-spec-witness-finalized-another-block "$(witnessed "$fresh" "$agree" "$(witness https://fork.invalid "$next" "$other" "$genesis" false)")" \
  '.action == "blocked" and .evidence == "inconsistent" and (.reason | test("finalized another block"))'
expect new-spec-uncorroborated "$(witnessed "$fresh" "$(witness https://down.invalid error)")" \
  '.action == "none" and (.unobserved | map(test("corroborated")) | any)'
# state <failures> <hours ago> [inputs] [id] [stage]: one recorded state.
state() {
  jq -cn --argjson n "$1" --argjson ago "$2" --arg inputs "${3:-abc}" --arg id "${4:-$id}" --arg stage "${5:-build}" \
    '{id: $id, stage: $stage, failures: $n, inputs: $inputs,
      attempted_at: (("2026-11-04T00:00:00Z" | fromdateiso8601) - $ago * 3600 | todate)}'
}
# previous <state args>: the open issue's states, keyed by id.
previous() { state "$@" | jq -c '{(.id): .}'; }
retry() { jq -c --argjson p "$1" --arg inputs "${2:-abc}" --argjson force "${3:-false}" \
  '.previous = $p | .inputs = $inputs | .force = $force' <<<"$(witnessed "$fresh" "$agree")"; }
expect backoff-after-first-failure "$(retry "$(previous 1 0.5)")" \
  '.action == "backoff" and .wanted == "adopt-finney" and .retry_at == "2026-11-04T00:25:00Z"'
expect retry-after-first-delay "$(retry "$(previous 1 1)")" '.action == "adopt-finney"'
expect blocked-report-does-not-delay "$(retry "$(previous 0 0 abc "$id" decide | jq -c '.[].attempted_at = null')")" '.action == "adopt-finney"'
expect backoff-doubles "$(retry "$(previous 3 3)")" '.action == "backoff" and .retry_at == "2026-11-04T00:55:00Z"'
expect backoff-caps-at-a-day "$(retry "$(previous 9 23)")" '.action == "backoff" and .retry_at == "2026-11-04T00:55:00Z"'
expect retry-after-a-day "$(retry "$(previous 9 24)")" '.action == "adopt-finney"'
expect retry-when-inputs-change "$(retry "$(previous 3 1)" def)" '.action == "adopt-finney"'
expect retry-when-forced "$(retry "$(previous 3 1)" abc true)" '.action == "adopt-finney"'
expect other-runtime-not-held "$(retry "$(previous 3 1 abc finney-999-0x00000000)")" '.action == "adopt-finney"'
expect backoff-does-not-hold-testnet "$(retry "$(previous 3 1)" | jq -c --argjson s "$next" --arg c "$other" '.testnet.spec = $s | .testnet.code = $c')" \
  '.action == "record-testnet" and (.deferred | map(.action)) == ["backoff"]'
# The issue also records a blocked testnet; Finney's state still applies.
shared=$(jq -cn --argjson f "$(state 3 1)" --argjson t "$(state 1 5 abc "testnet-$spec-${other:0:10}" decide)" '{($f.id): $f, ($t.id): $t}')
expect backoff-with-another-network-recorded "$(retry "$shared" | jq -c --arg c "$other" '.testnet.code = $c')" \
  '.action == "blocked" and .network == "testnet" and (.deferred | map(.action)) == ["backoff"]'

# ── core bump (D-136) ─────────────────────────────────────────────────
# Two pins roots: the core held behind the parity pin, and not held.
held_root=$work/core-held
free_root=$work/core-free
mkdir -p "$held_root" "$free_root"
for pins_root in "$held_root" "$free_root"; do
  ln -s "$root/vendor" "$pins_root/vendor"
done
held_fp=b3:$(printf '%064d' 5)
awk -v fp="$held_fp" '
  /^\[/ { inside = ($0 == "[parity]") }
  inside && $1 == "core_fingerprint" { next }
  { print }
  inside && $1 == "spec_version" { print "core_fingerprint = \"" fp "\"" }
' "$root/upstream.toml" >"$held_root/upstream.toml"
grep -v '^core_fingerprint = ' "$root/upstream.toml" >"$free_root/upstream.toml"
parity_tag=$(toml_get "$root/upstream.toml" parity tag)
core_tag=$(toml_get "$root/upstream.toml" core tag)
core_id=core-$parity_tag-${held_fp:3:10}
# expect_pins <name> <pins root> <observation json> <jq predicate>
expect_pins() {
  local name=$1 observed=$work/$1.observed.json decision=$work/$1.decision.json
  printf '%s\n' "$3" >"$observed"
  bash "$script" decide "$observed" "$decision" "$2" >/dev/null
  if jq -e "$4" "$decision" >/dev/null; then
    echo "ok   $name: $(jq -c '{action, source, id, deferred: (.deferred | map(.action)), core_bump}' "$decision")"
  else
    echo "FAIL $name: $(jq -c . "$decision")"
    failures=$((failures + 1))
  fi
}
bump() { jq -c '.core_bump = true' <<<"$1"; }
current=$(observation "$spec" "$code" "$testnet_spec" "$testnet_code")
expect_pins core-bump-not-requested "$held_root" "$current" '.action == "none" and .core_bump == null'
expect_pins core-bump-requested "$held_root" "$(bump "$current")" \
  ".action == \"core-bump\" and .network == \"finney\" and .spec == $spec and .code == \"$code\" and .id == \"$core_id\"
   and .core == {from: \"$core_tag\", to: \"$parity_tag\", fingerprint: \"$held_fp\"} and .source == \"$parity_tag\" and .core_bump == \"requested\""
expect_pins core-bump-not-due "$free_root" "$(bump "$current")" '.action == "none" and (.core_bump | test("none is due"))'
expect_pins core-bump-waits-for-an-adoption "$held_root" "$(bump "$(observation "$next" "$other" "$testnet_spec" "$testnet_code" "$release")")" \
  '.action == "adopt-finney" and .deferred == [] and (.core_bump | test("waits"))'
expect_pins core-bump-waits-for-a-blocked-finney "$held_root" "$(bump "$(observation "$spec" "$other" "$testnet_spec" "$testnet_code")")" \
  '.action == "blocked" and .network == "finney" and (.core_bump | test("waits"))'
expect_pins core-bump-waits-for-an-unobserved-finney "$held_root" "$(bump "$(observation null "" "$testnet_spec" "$testnet_code")")" \
  '.action == "none" and (.core_bump | test("waits"))'
expect_pins core-bump-before-testnet "$held_root" "$(bump "$(observation "$spec" "$code" "$next" "$other")")" \
  '.action == "core-bump" and (.deferred | map(.action)) == ["record-testnet"]'
core_retry() { jq -c --argjson p "$1" --argjson force "${2:-false}" '.previous = $p | .inputs = "abc" | .force = $force' <<<"$(bump "$current")"; }
expect_pins core-bump-backs-off "$held_root" "$(core_retry "$(previous 1 0.5 abc "$core_id" lock)")" \
  '.action == "backoff" and .wanted == "core-bump"'
expect_pins core-bump-forced "$held_root" "$(core_retry "$(previous 1 0.5 abc "$core_id" lock)" true)" '.action == "core-bump"'
unreleased_root=$work/core-unreleased
mkdir -p "$unreleased_root"
ln -s "$root/vendor" "$unreleased_root/vendor"
parity_commit=$(toml_get "$root/upstream.toml" parity commit)
awk '{ print } /^\[/ { inside = ($0 == "[parity]") } inside && $1 == "core_fingerprint" { print "release = \"unreleased\""; print "source_branch = \"main\"" }' \
  "$held_root/upstream.toml" >"$unreleased_root/upstream.toml"
expect_pins core-bump-unreleased-source "$unreleased_root" "$(bump "$current")" \
  ".action == \"core-bump\" and .source == \"branch:main@$parity_commit\""

# The commit guard: a core bump may change manifests and locks, and in a
# manifest only the core's references; everything else stays refused.
sed -n -e '/^die() {/,/^}/p' -e '/^tooling_paths=/p' -e '/^core_bump_paths=/p' -e '/^core_reference=/p' \
  -e '/^vendored_sources=/p' -e '/^check_paths() {/,/^}/p' "$script" >"$work/paths.sh"
# shellcheck disable=SC1091
source "$work/paths.sh"
guard=$work/guard
git init -q "$guard"
upstream_url=https://github.com/RaoFoundation/subtensor.git
fork_url=https://github.com/sam0x17/subtensor.git
mkdir -p "$guard/ci/probe" "$guard/xtask"
cat >"$guard/Cargo.toml" <<TOML
[workspace.dependencies]
bittensor-core = { git = "$upstream_url", tag = "v478", default-features = false }
serde = "1"

[patch."$upstream_url"]
bittensor-core = { git = "$fork_url", rev = "$(printf '%040d' 1)" }
TOML
printf 'w3f-bls = { git = "%s", tag = "v478" }\n' "$upstream_url" >"$guard/ci/probe/Cargo.toml"
printf 'lock v478\n' >"$guard/Cargo.lock"
printf 'fn main() {}\n' >"$guard/xtask/main.rs"
mkdir -p "$guard/vendor/upstream/src/sdk/bittensor-core"
printf '[package]\nname = "bittensor-core"\n' >"$guard/vendor/upstream/src/sdk/bittensor-core/Cargo.toml"
printf '[workspace]\n' >"$guard/vendor/upstream/Cargo.toml"
guarded() { git -C "$guard" -c user.name=t -c user.email=t@invalid "$@"; }
guarded add -A
guarded commit -q -m base
base_commit=$(guarded rev-parse HEAD)
# guard_case <name> <expected: pass|refuse> <action> <edit command>
guard_case() {
  guarded checkout -q --detach "$base_commit"
  (cd "$guard" && eval "$4")
  guarded commit -q -am "$1"
  if (cd "$guard" && check_paths "$base_commit" HEAD "$3") >/dev/null 2>&1; then got=pass; else got=refuse; fi
  if [[ "$got" == "$2" ]]; then
    echo "ok   guard $1: $got"
  else
    echo "FAIL guard $1: expected $2, got $got"
    failures=$((failures + 1))
  fi
}
moved="sed -i 's/tag = \"v478\"/tag = \"v488\"/; s/rev = \"0*1\"/rev = \"$(printf '%040d' 2)\"/' Cargo.toml ci/probe/Cargo.toml && echo 'lock v488' >Cargo.lock"
guard_case core-references-move pass core-bump "$moved"
guard_case adoption-refuses-manifests refuse adopt-finney "$moved"
guard_case core-bump-refuses-other-manifest-lines refuse core-bump "$moved && sed -i 's/serde = \"1\"/serde = \"2\"/' Cargo.toml"
guard_case core-bump-refuses-tooling refuse core-bump "$moved && echo '// x' >>xtask/main.rs"
guard_case adoption-accepts-vendored-upstream-manifest pass adopt-finney "echo 'version = \"1\"' >>vendor/upstream/src/sdk/bittensor-core/Cargo.toml"
guard_case adoption-refuses-manifest-outside-vendored-sources refuse adopt-finney "echo '# x' >>vendor/upstream/Cargo.toml"
guard_case core-bump-refuses-another-repository refuse core-bump "sed -i 's|$fork_url|https://github.com/attacker/subtensor.git|' Cargo.toml"

# ── live-evidence verdict ─────────────────────────────────────────────
v15=$(toml_get "$root/upstream.toml" networks.finney.metadata v15_sha256)
v16=$(toml_get "$root/upstream.toml" networks.finney.metadata v16_sha256)
# endpoint <name> [field value]: one endpoint's evidence, optionally altered.
endpoint() {
  jq -cn --arg e "$1" --arg g "$genesis" --argjson s "$spec" --arg c "$code" --arg v15 "$v15" --arg v16 "$v16" \
    '{endpoint: $e, genesis: $g, spec: $s, spec_name: "node-subtensor", transaction_version: 1,
      code_reported: $c, code: $c, v15_sha256: $v15, v16_sha256: $v16, number: 9400000, canonical: true}' |
    if [[ -n "${2:-}" ]]; then jq -c --arg k "$2" --argjson v "$3" '.[$k] = $v'; else cat; fi
}
# judge <name> <release status> <jq predicate> <endpoint rows...>
judge() {
  local name=$1 status=$2 predicate=$3 live=$work/$1.live.json
  shift 3
  jq -n --arg g "$genesis" --argjson s "$spec" --arg c "$code" --arg v15 "$v15" --arg v16 "$v16" \
    --argjson rows "$(printf '%s\n' "$@" | jq -s .)" --arg status "$status" \
    '{block: "0x00", expected: {genesis: $g, spec: $s, spec_name: "node-subtensor", code: $c, v15_sha256: $v15, v16_sha256: $v16},
      endpoints: $rows, release: {status: $status, code: "0x01"}}' >"$live"
  if bash "$script" verdict "$live" | jq -e "$predicate" >/dev/null; then
    echo "ok   verdict $name: $(bash "$script" verdict "$live" | jq -c '{ok, agreeing, reason}')"
  else
    echo "FAIL verdict $name: $(bash "$script" verdict "$live")"
    failures=$((failures + 1))
  fi
}
down='{"endpoint": "https://down.invalid", "error": "metadata V16 unavailable"}'
judge consistent matches '.ok and .agreeing == 2' "$(endpoint a)" "$(endpoint b)" "$down"
judge no-release-published unavailable '.ok' "$(endpoint a)" "$(endpoint b)"
judge one-endpoint unavailable '(.ok | not) and (.reason | test("two are required"))' "$(endpoint a)" "$down"
judge code-bytes-differ matches '(.ok | not) and (.reason | test("BLAKE2b"))' "$(endpoint a)" "$(endpoint b code '"0x02"')"
judge reported-hash-differs matches '(.ok | not) and (.reason | test("reported"))' "$(endpoint a)" "$(endpoint b code_reported '"0x02"')"
judge metadata-differs matches '(.ok | not) and (.reason | test("V16"))' "$(endpoint a)" "$(endpoint b v16_sha256 '"00"')" "$(endpoint c)"
judge not-on-finalized-chain matches '(.ok | not) and (.reason | test("finalized another block"))' "$(endpoint a)" "$(endpoint b)" "$(endpoint c canonical false)"
judge release-differs differs '(.ok | not) and (.reason | test("release wasm"))' "$(endpoint a)" "$(endpoint b)"

# ── backoff bookkeeping ───────────────────────────────────────────────
# bookkeep <name> <decision action> <stage> <previous states or ""> <jq
# predicate on this id's new state> [deferred ids, comma-separated]
# [jq predicate on all states] [observed networks] [failure time] [inputs]
# [decision id]
bookkeep() {
  local name=$1 decision=$work/$1.state-decision.json prior=$work/$1.prior.json
  jq -n --arg action "$2" --arg id "${11:-$id}" --arg deferred "${6:-}" --argjson observed "${8:-{\}}" --arg inputs "${10:-abc}" \
    '{action: $action, id: $id, inputs: $inputs, observed_at: "2026-11-04T00:00:00Z", observed: $observed,
      deferred: ($deferred | split(",") | map(select(. != "") | {id: .}))}' >"$decision"
  printf '%s\n' "${4:-null}" >"$prior"
  local states
  states=$(RTAO_ADOPT_NOW=${9:-2026-11-04T00:00:00Z} bash "$script" state "$decision" failure "$3" "$prior")
  if jq -e --arg id "${11:-$id}" ".[\$id] | $5" <<<"$states" >/dev/null && jq -e "${7:-true}" <<<"$states" >/dev/null; then
    echo "ok   state $name: $states"
  else
    echo "FAIL state $name: $states"
    failures=$((failures + 1))
  fi
}
bookkeep first-failure adopt-finney build "" '.failures == 1 and .stage == "build" and .attempted_at == "2026-11-04T00:00:00Z" and .inputs == "abc"'
bookkeep repeated-failure adopt-finney build "$(previous 2 3)" '.failures == 3'
bookkeep later-stage-counts adopt-finney tests "$(previous 2 3)" '.failures == 3 and .stage == "tests"'
bookkeep attempt-time-is-failure-time adopt-finney tests "$(previous 1 1)" '.failures == 2 and .attempted_at == "2026-11-04T02:30:00Z"' "" true '{}' 2026-11-04T02:30:00Z
bookkeep changed-inputs-reset adopt-finney build "$(previous 4 3)" '.failures == 1 and .inputs == "def"' "" true '{}' "" def
bookkeep other-runtime-resets adopt-finney build "$(previous 2 3 abc finney-999-0x00000000)" '.failures == 1'
bookkeep blocked-does-not-count blocked decide "$(previous 1 5 abc "$id" decide)" '.failures == 1 and .attempted_at == "2026-11-03T19:00:00Z"'
bookkeep blocked-records-no-attempt blocked decide "" '.failures == 0 and .attempted_at == null'
bookkeep blocked-keeps-attempts blocked decide "$(previous 3 1)" '.failures == 3 and .stage == "decide" and .attempted_at == "2026-11-03T23:00:00Z"'
held=testnet-$spec-${other:0:10}
bookkeep deferred-state-kept blocked decide "$(jq -c --argjson t "$(state 2 3 abc "$held")" '. + {($t.id): $t}' <<<"$(previous 1 5 abc "$id" decide)")" \
  '.failures == 1' "$held" "keys == ([\"$id\", \"$held\"] | sort) and .[\"$held\"].failures == 2"
two=$(jq -c --argjson t "$(state 2 3 abc "$held")" '. + {($t.id): $t}' <<<"$(previous 2 3)")
bookkeep moved-on-state-dropped adopt-finney build "$two" \
  '.failures == 3' "" "keys == [\"$id\"]" "{\"testnet\": {\"spec\": $next, \"code\": \"$code\"}}"
bookkeep unobserved-state-kept adopt-finney build "$two" \
  '.failures == 3' "" "keys == ([\"$id\", \"$held\"] | sort)" '{"testnet": {"error": "unreachable"}}'
bookkeep same-runtime-state-kept adopt-finney build "$two" \
  '.failures == 3' "" "keys == ([\"$id\", \"$held\"] | sort)" "{\"testnet\": {\"spec\": $spec, \"code\": \"$other\"}}"
bookkeep decide-failure-keeps-states blocked decide "$two" '.failures == 0' "" \
  "keys == ([\"$id\", \"$held\", \"decide\"] | sort) and .[\"$id\"].failures == 2" '{}' "" "" decide
bookkeep decide-state-dropped-once-it-works adopt-finney build "$(jq -c '. + {decide: {id: "decide", stage: "decide", failures: 0}}' <<<"$(previous 2 3)")" \
  '.failures == 3' "" "keys == [\"$id\"]"

# ── metadata envelope ─────────────────────────────────────────────────
# The live V15 envelope is SCALE Option<Vec<u8>>; its payload must digest to
# the committed pin, whatever the compact length width.
blob=$root/$(toml_get "$root/upstream.toml" networks.finney.metadata v15)
size=$(stat -c %s "$blob")
compact=$(printf '%08x' $((size << 2 | 2)) | sed 's/\(..\)\(..\)\(..\)\(..\)/\4\3\2\1/')
printf '0x01%s%s' "$compact" "$(basenc --base16 -w0 "$blob" | tr A-F a-f)" >"$work/envelope.hex"
sed -n -e '/^unhex() {/p' -e '/^envelope_sha256() {/,/^}/p' "$script" >"$work/envelope.sh"
# shellcheck disable=SC1091
source "$work/envelope.sh"
if [[ "$(envelope_sha256 "$work/envelope.hex")" == "$v15" ]] &&
  printf '0x01%02x%s' $((3 << 2)) abcdef >"$work/short.hex" &&
  [[ "$(envelope_sha256 "$work/short.hex")" == "$(printf '\xab\xcd\xef' | sha256sum | cut -c1-64)" ]] &&
  printf '0x00' >"$work/none.hex" && [[ "$(envelope_sha256 "$work/none.hex")" == none ]] &&
  printf '0x01%02x%s' $((4 << 2)) abcdef >"$work/truncated.hex" && [[ "$(envelope_sha256 "$work/truncated.hex")" == invalid ]]; then
  echo "ok   envelope: the committed V15 pin, a one-byte length, None and a truncated payload"
else
  echo "FAIL envelope"
  failures=$((failures + 1))
fi

# The issue body round-trips the state the next observation reads.
sed -n '/^state_from_body() {/,/^}/p' "$script" >"$work/state.sh"
# shellcheck disable=SC1091
source "$work/state.sh"
recorded=$(previous 2 3)
if [[ "$(printf 'text\n<!-- rtao-runtime-adopt-state %s -->\nmore\n' "$recorded" | state_from_body)" == "$recorded" ]] &&
  [[ "$(printf '<!-- rtao-runtime-adopt-state %s -->\n' "$(state 2 3)" | state_from_body)" == "$recorded" ]] &&
  [[ "$(printf 'an issue without state\n' | state_from_body)" == null ]]; then
  echo "ok   issue state: recorded, a single legacy state and absent"
else
  echo "FAIL issue state"
  failures=$((failures + 1))
fi

# ── issue and alert loop ──────────────────────────────────────────────
# notify against stand-ins for gh (issues kept as files) and curl (alerts
# appended to a log): inconsistent evidence opens one issue and one alert;
# its hourly repeat adds neither; a failed adoption records its state, the
# next observation backs off on it, a repeat alerts no more, and the
# adoption closes the issue and resolves every episode.
fake=$work/fake
mkdir -p "$fake/bin" "$fake/issues"
cat >"$fake/bin/gh" <<'GH'
#!/usr/bin/env bash
set -euo pipefail
dir=$FAKE_GH
[[ "$1" == issue ]] || exit 1
verb=$2
shift 2
number="" title="" body="" jq_expr="" comment=""
while (($#)); do
  case "$1" in
    --repo | --state | --limit | --json) shift 2 ;;
    --title) title=$2; shift 2 ;;
    --body) body=$2; shift 2 ;;
    --jq) jq_expr=$2; shift 2 ;;
    --comment) comment=$2; shift 2 ;;
    *) number=$1; shift ;;
  esac
done
case "$verb" in
  list)
    for file in "$dir"/*.open; do
      [[ -e "$file" ]] || continue
      jq -n --argjson n "$(basename "$file" .open)" --rawfile b "$file" '{number: $n, body: $b}'
    done | jq -s . | jq -r "$jq_expr" ;;
  view) jq -n --rawfile b "$dir/$number.open" '{body: $b}' | jq -r "$jq_expr" ;;
  create)
    n=$(($(find "$dir" -name '*.open' -o -name '*.closed' | wc -l) + 1))
    printf '%s' "$body" >"$dir/$n.open"
    echo "create $n $title" >>"$dir/log" ;;
  edit)
    printf '%s' "$body" >"$dir/$number.open"
    echo "edit $number $title" >>"$dir/log" ;;
  close)
    mv "$dir/$number.open" "$dir/$number.closed"
    echo "close $number $comment" >>"$dir/log" ;;
esac
GH
cat >"$fake/bin/curl" <<'CURL'
#!/usr/bin/env bash
while (($#)); do
  if [[ "$1" == --data ]]; then printf '%s\n' "$2" >>"$FAKE_GH/alerts"; fi
  shift
done
CURL
chmod +x "$fake/bin/gh" "$fake/bin/curl"
sed -n -e '/^adopt_marker=/p' -e '/^open_issue() {/,/^}/p' -e '/^previous_state() {/,/^}/p' -e '/^state_from_body() {/,/^}/p' \
  "$script" >"$work/issue-state.sh"
# shellcheck disable=SC1091
source "$work/issue-state.sh"
loop() {
  # The simulated loop must not see the checkout's own due ports, which a
  # real adoption leaves in parity/port_due.toml (one informational issue).
  RTAO_ADOPT_PORT_DUE=$work/no-port-due.toml \
  FAKE_GH=$fake/issues PATH=$fake/bin:$PATH GITHUB_REPOSITORY=owner/repo GH_TOKEN=x \
    RUNTIME_ALERT_URL=https://alerts.invalid RUNTIME_ALERT_TOKEN=x "$@"
}
alerts() { jq -s -c --arg id "$1" 'map(select(.id == $id) | .open)' "$fake/issues/alerts" 2>/dev/null || echo '[]'; }
check() {
  if eval "$2"; then
    echo "ok   loop $1"
  else
    echo "FAIL loop $1 (issues: $(ls "$fake/issues" | tr '\n' ' '), alerts: $(cat "$fake/issues/alerts" 2>/dev/null | jq -s -c 'map({id, open})'))"
    failures=$((failures + 1))
  fi
}
# run <name> <observation> <outcome> [stage] [finished at]: decide on the
# observation with the stand-in issue's states, then notify the outcome at
# the time the run finished (default: when it observed).
run() {
  local observed=$work/loop-$1.observed.json decision=$work/loop-$1.decision.json evidence=$work/loop-$1.evidence
  mkdir -p "$evidence"
  loop jq -c --argjson p "$(loop previous_state)" '.previous = $p | .inputs = "abc" | .force = false' <<<"$2" >"$observed"
  bash "$script" decide "$observed" "$decision" >/dev/null
  if [[ -n "${4:-}" ]]; then
    jq -n --arg stage "$4" '{status: "running", stage: $stage}' >"$evidence/stage.json"
  fi
  RTAO_ADOPT_NOW=${5:-$(jq -r .observed_at "$observed")} loop bash "$script" notify "$decision" "$3" "$evidence" >/dev/null
  jq -r .action "$decision"
}
# decide_failed <at>: the workflow's report of a failed decide step.
decide_failed() {
  jq -n '{action: "blocked", network: "upstream", spec: null, code: null, id: "decide",
    reason: "the runtime-adopt decision step failed (observation or pins)", upcoming: []}' >"$work/decide-failed.json"
  RTAO_ADOPT_NOW=$1 loop bash "$script" notify "$work/decide-failed.json" failure >/dev/null
}
liar=$(witnessed "$fresh" "$agree" "$(witness https://liar.invalid "$next" "$code")")
at() { jq -c --arg at "$1" '.observed_at = $at' <<<"$2"; }
check inconsistent-opens-one-issue '[[ "$(run i1 "$(at 2026-11-04T00:00:00Z "$liar")" failure)" == blocked && -f $fake/issues/1.open && "$(alerts "$id")" == "[true]" ]]'
check inconsistent-repeat-is-quiet '[[ "$(run i2 "$(at 2026-11-04T01:00:00Z "$liar")" failure)" == blocked && "$(alerts "$id")" == "[true]" && ! -f $fake/issues/2.open && "$(grep -c "^edit" $fake/issues/log || true)" == 0 ]]'
good=$(witnessed "$fresh" "$agree")
check adoption-failure-records-state '[[ "$(run f1 "$(at 2026-11-04T02:00:00Z "$good")" failure build)" == adopt-finney && "$(alerts "$id")" == "[true,true]" && "$(loop previous_state | jq -c --arg id "$id" ".[\$id] | [.stage, .failures]")" == "[\"build\",1]" ]]'
check next-hour-backs-off '[[ "$(run b1 "$(at 2026-11-04T02:40:00Z "$good")" backoff)" == backoff && "$(alerts "$id")" == "[true,true]" ]]'
check retry-repeats-quietly '[[ "$(run f2 "$(at 2026-11-04T03:00:00Z "$good")" failure build)" == adopt-finney && "$(alerts "$id")" == "[true,true]" && "$(loop previous_state | jq -r --arg id "$id" ".[\$id].failures")" == 2 ]]'
check backoff-doubles-after-repeat '[[ "$(run b2 "$(at 2026-11-04T04:00:00Z "$good")" backoff)" == backoff && "$(jq -r .retry_at $work/loop-b2.decision.json)" == 2026-11-04T04:55:00Z ]]'
# A failed decide step in between reports itself once and keeps the backoff.
check decide-failure-keeps-backoff '{ decide_failed 2026-11-04T04:10:00Z; [[ "$(alerts decide)" == "[true]" && "$(loop previous_state | jq -r --arg id "$id" ".[\$id].failures")" == 2 && "$(run b3 "$(at 2026-11-04T04:20:00Z "$good")" backoff)" == backoff && "$(jq -r .retry_at $work/loop-b3.decision.json)" == 2026-11-04T04:55:00Z && "$(alerts "$id")" == "[true,true]" ]]; }'
# A long attempt backs off from when it failed, and drops the decide state.
check long-attempt-backs-off-from-its-end '[[ "$(run f3 "$(at 2026-11-04T05:00:00Z "$good")" failure build 2026-11-04T07:30:00Z)" == adopt-finney && "$(alerts "$id")" == "[true,true]" && "$(alerts decide)" == "[true,false]" && "$(run b4 "$(at 2026-11-04T07:40:00Z "$good")" backoff)" == backoff && "$(jq -r .retry_at $work/loop-b4.decision.json)" == 2026-11-04T11:25:00Z ]]'
check adoption-closes-and-resolves '[[ "$(run s1 "$(at 2026-11-04T11:30:00Z "$good")" success)" == adopt-finney && -f $fake/issues/1.closed && "$(alerts "$id")" == "[true,true,false]" && "$(alerts decide)" == "[true,false]" ]]'

# A failed core bump stays open through runs that find nothing to adopt.
core_fake=$work/core-fake
mkdir -p "$core_fake"
core_loop() { FAKE_GH=$core_fake RTAO_ADOPT_PORT_DUE=$work/no-port-due.toml PATH=$fake/bin:$PATH \
  RTAO_ADOPT_PINS_ROOT=${core_pins:-$held_root} \
  GITHUB_REPOSITORY=owner/repo GH_TOKEN=x RUNTIME_ALERT_URL=https://alerts.invalid RUNTIME_ALERT_TOKEN=x "$@"; }
jq -n --arg id "$core_id" --argjson spec "$spec" --arg code "$code" --arg from "$core_tag" --arg to "$parity_tag" \
  '{action: "core-bump", network: "finney", spec: $spec, code: $code, id: $id, reason: "core bump", upcoming: [], deferred: [],
    core: {from: $from, to: $to}, inputs: "abc", observed_at: "2026-11-04T00:00:00Z", observed: {}}' >"$work/core-failed.json"
core_loop bash "$script" notify "$work/core-failed.json" failure >/dev/null
jq -n '{action: "none", network: null, spec: null, code: null, id: "none", reason: "indexed", upcoming: [], deferred: [], unobserved: []}' \
  >"$work/core-none.json"
core_loop bash "$script" notify "$work/core-none.json" none >/dev/null
check core-bump-failure-survives-a-quiet-run '[[ -f $core_fake/1.open ]] && grep -q "Core bump failed" $core_fake/log && ! grep -q "^close" $core_fake/log'
# Once the pins no longer hold the core, the next quiet run closes it as bumped.
core_pins=$free_root core_loop bash "$script" notify "$work/core-none.json" none >/dev/null
check core-bump-issue-closes-once-the-core-moved '[[ -f $core_fake/1.closed ]] && grep -q "^close" $core_fake/log && grep -q "Core bumped" $core_fake/log'

if ((failures)); then
  echo "$failures decision checks failed" >&2
  exit 1
fi
echo "all decision checks passed"
