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
ahead="{\"main\": {\"head\": \"$merge\", \"spec\": $next}, \"testnet\": {\"head\": \"$merge\", \"spec\": $testnet_spec}}"

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
  ".action == \"none\" and (.upcoming | map(.branch)) == [\"main\"] and .upcoming[0].spec == $next"
expect wrong-spec-name "$(observation "$spec" "$code" "$testnet_spec" "$testnet_code" | jq -c '.finney.spec_name = "other"')" \
  '.action == "blocked" and (.reason | test("spec_name"))'

# ── witnesses and backoff ─────────────────────────────────────────────
genesis=$(toml_get "$root/upstream.toml" networks.finney genesis)
# witness <endpoint> <spec> <code> [genesis]: one witness row; "error" for none.
witness() {
  if [[ "$2" == error ]]; then
    jq -cn --arg e "$1" '{endpoint: $e, error: "no complete runtime at the block"}'
  else
    jq -cn --arg e "$1" --argjson s "$2" --arg c "$3" --arg g "${4:-$genesis}" \
      '{endpoint: $e, genesis: $g, spec: $s, spec_name: "node-subtensor", code: $c}'
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

# ── live-evidence verdict ─────────────────────────────────────────────
v15=$(toml_get "$root/upstream.toml" networks.finney.metadata v15_sha256)
v16=$(toml_get "$root/upstream.toml" networks.finney.metadata v16_sha256)
# endpoint <name> [field value]: one endpoint's evidence, optionally altered.
endpoint() {
  jq -cn --arg e "$1" --arg g "$genesis" --argjson s "$spec" --arg c "$code" --arg v15 "$v15" --arg v16 "$v16" \
    '{endpoint: $e, genesis: $g, spec: $s, spec_name: "node-subtensor", transaction_version: 1,
      code_reported: $c, code: $c, v15_sha256: $v15, v16_sha256: $v16}' |
    if [[ -n "${2:-}" ]]; then jq -c --arg k "$2" --arg v "$3" '.[$k] = $v'; else cat; fi
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
judge code-bytes-differ matches '(.ok | not) and (.reason | test("BLAKE2b"))' "$(endpoint a)" "$(endpoint b code 0x02)"
judge reported-hash-differs matches '(.ok | not) and (.reason | test("reported"))' "$(endpoint a)" "$(endpoint b code_reported 0x02)"
judge metadata-differs matches '(.ok | not) and (.reason | test("V16"))' "$(endpoint a)" "$(endpoint b v16_sha256 00)" "$(endpoint c)"
judge release-differs differs '(.ok | not) and (.reason | test("release wasm"))' "$(endpoint a)" "$(endpoint b)"

# ── backoff bookkeeping ───────────────────────────────────────────────
# bookkeep <name> <decision action> <stage> <previous states or ""> <jq
# predicate on this id's new state> [deferred ids, comma-separated]
# [jq predicate on all states]
bookkeep() {
  local name=$1 decision=$work/$1.state-decision.json prior=$work/$1.prior.json
  jq -n --arg action "$2" --arg id "$id" --arg deferred "${6:-}" \
    '{action: $action, id: $id, inputs: "abc", observed_at: "2026-11-04T00:00:00Z",
      deferred: ($deferred | split(",") | map(select(. != "") | {id: .}))}' >"$decision"
  printf '%s\n' "${4:-null}" >"$prior"
  local states
  states=$(bash "$script" state "$decision" failure "$3" "$prior")
  if jq -e --arg id "$id" ".[\$id] | $5" <<<"$states" >/dev/null && jq -e "${7:-true}" <<<"$states" >/dev/null; then
    echo "ok   state $name: $states"
  else
    echo "FAIL state $name: $states"
    failures=$((failures + 1))
  fi
}
bookkeep first-failure adopt-finney build "" '.failures == 1 and .stage == "build" and .attempted_at == "2026-11-04T00:00:00Z" and .inputs == "abc"'
bookkeep repeated-failure adopt-finney build "$(previous 2 3)" '.failures == 3'
bookkeep later-stage-resets adopt-finney tests "$(previous 2 3)" '.failures == 1 and .stage == "tests"'
bookkeep other-runtime-resets adopt-finney build "$(previous 2 3 abc finney-999-0x00000000)" '.failures == 1'
bookkeep blocked-does-not-count blocked decide "$(previous 1 5 abc "$id" decide)" '.failures == 1 and .attempted_at == "2026-11-03T19:00:00Z"'
held=testnet-$spec-${other:0:10}
bookkeep deferred-state-kept blocked decide "$(jq -c --argjson t "$(state 2 3 abc "$held")" '. + {($t.id): $t}' <<<"$(previous 1 5 abc "$id" decide)")" \
  '.failures == 1' "$held" "keys == ([\"$id\", \"$held\"] | sort) and .[\"$held\"].failures == 2"
bookkeep stale-state-dropped adopt-finney build "$(jq -c --argjson t "$(state 2 3 abc "$held")" '. + {($t.id): $t}' <<<"$(previous 2 3)")" \
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
# run <name> <observation> <outcome> [stage]: decide on the observation with
# the stand-in issue's states, then notify the outcome.
run() {
  local observed=$work/loop-$1.observed.json decision=$work/loop-$1.decision.json evidence=$work/loop-$1.evidence
  mkdir -p "$evidence"
  loop jq -c --argjson p "$(loop previous_state)" '.previous = $p | .inputs = "abc" | .force = false' <<<"$2" >"$observed"
  bash "$script" decide "$observed" "$decision" >/dev/null
  if [[ -n "${4:-}" ]]; then
    jq -n --arg stage "$4" '{status: "running", stage: $stage}' >"$evidence/stage.json"
  fi
  loop bash "$script" notify "$decision" "$3" "$evidence" >/dev/null
  jq -r .action "$decision"
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
check adoption-closes-and-resolves '[[ "$(run s1 "$(at 2026-11-04T05:00:00Z "$good")" success)" == adopt-finney && -f $fake/issues/1.closed && "$(alerts "$id")" == "[true,true,false]" ]]'

if ((failures)); then
  echo "$failures decision checks failed" >&2
  exit 1
fi
echo "all decision checks passed"
