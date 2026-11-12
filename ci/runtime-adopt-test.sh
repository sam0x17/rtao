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
  -e '/^vendored_sources=/p' -e '/^generated_packages=/p' -e '/^republished_dependency=/p' \
  -e '/^publish_record_files=/p' -e '/^publish_checksum=/p' -e '/^without_publish_checksums() {/,/^}/p' \
  -e '/^check_publish_record() {/,/^}/p' -e '/^record_checksums() {/,/^}/p' \
  -e '/^check_paths() {/,/^}/p' "$script" >"$work/paths.sh"
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

[workspace.dependencies.republished]
bittensor-core = { package = "rtao-upstream-core", path = "upstream/core", version = "=0.490.0", default-features = false }
w3f-bls = { package = "rtao-upstream-w3f-bls", path = "upstream/w3f-bls", version = "=0.490.0", default-features = false, features = ["std"] }
tle = { package = "rtao-upstream-tle", path = "upstream/tle", version = "=0.490.0", default-features = false }
TOML
mkdir -p "$guard/upstream/core" "$guard/crates/rtao-core" "$guard/crates/rtao-testkit"
printf '[package]\nname = "rtao-upstream-core"\nversion = "0.490.0"\n' >"$guard/upstream/core/Cargo.toml"
printf 'fn main() {}\n' >"$guard/upstream/core/build.rs"
cat >"$guard/upstream.toml" <<TOML
[core]
tag = "v490"

[publish.macros]
package = "rtao-upstream-macros"
version = "0.490.0"
path = "support/macros"
checksum = "$(printf 'a%.0s' {1..64})"

[publish.core]
package = "rtao-upstream-core"
version = "0.490.1"
path = "sdk/bittensor-core"

[parity]
tag = "v490"
TOML
cp "$guard/upstream.toml" "$guard/crates/rtao-core/upstream.toml"
cp "$guard/upstream.toml" "$guard/crates/rtao-testkit/upstream.toml"
printf '[{"key": "core", "package": "rtao-upstream-core", "version": "0.490.1", "sha256": "%s", "status": "published"}]\n' \
  "$(printf 'b%.0s' {1..64})" >"$work/published-core.json"
printf '# guard\n' >"$guard/README.md"
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
# A core bump moves the republished packages (D-188): their generated sources
# and, in the root manifest, only the version of each exact requirement.
republished="sed -i 's/version = \"=0.490.0\"/version = \"=0.491.0\"/' Cargo.toml && sed -i 's/0.490.0/0.491.0/' upstream/core/Cargo.toml && echo '// moved' >>upstream/core/build.rs"
guard_case core-bump-moves-republished-packages pass core-bump "$moved && $republished"
guard_case core-bump-refuses-a-republished-path-change refuse core-bump "$republished && sed -i 's|path = \"upstream/tle\"|path = \"elsewhere/tle\"|' Cargo.toml"
guard_case core-bump-refuses-a-republished-feature-change refuse core-bump "$republished && sed -i '/^tle = /s/default-features = false/default-features = true/' Cargo.toml"
guard_case core-bump-refuses-an-unpaired-republished-line refuse core-bump "sed -i '/^tle = /d' Cargo.toml"
guard_case adoption-refuses-generated-packages refuse adopt-finney "$republished"
# The [publish] record (D-188): checksum lines added to [publish.<key>] tables
# of upstream.toml and its byte-identical crate copies, nothing else.
copies="cp upstream.toml crates/rtao-core/upstream.toml && cp upstream.toml crates/rtao-testkit/upstream.toml"
record="record_checksums $work/published-core.json upstream.toml && $copies"
guard_case publish-record-passes pass publish-forks "$record"
guard_case publish-record-refuses-another-line refuse publish-forks "$record && sed -i 's/^tag = \"v490\"/tag = \"v491\"/' upstream.toml && $copies"
guard_case publish-record-refuses-a-diverging-copy refuse publish-forks "record_checksums $work/published-core.json upstream.toml && cp upstream.toml crates/rtao-core/upstream.toml"
guard_case publish-record-refuses-a-checksum-outside-publish refuse publish-forks "sed -i '/^\[parity\]/a checksum = \"$(printf 'c%.0s' {1..64})\"' upstream.toml && $copies"
guard_case publish-record-refuses-a-replaced-checksum refuse publish-forks "sed -i 's/^checksum = \"a*\"/checksum = \"$(printf 'd%.0s' {1..64})\"/' upstream.toml && $copies"
guard_case publish-record-refuses-another-file refuse publish-forks "$record && echo more >>README.md"
guard_case publish-record-refuses-a-manifest refuse publish-forks "$record && $republished"
guard_case publish-record-refuses-an-unrecorded-change refuse publish-forks "echo more >>README.md"
guarded show "$base_commit:upstream.toml" >"$work/record-twice.toml"
if (cd "$work" && record_checksums "$work/published-core.json" record-twice.toml) >/dev/null 2>&1 &&
  grep -q "^checksum = \"$(printf 'b%.0s' {1..64})\"$" "$work/record-twice.toml" &&
  (cd "$work" && record_checksums "$work/published-core.json" record-twice.toml) >/dev/null 2>&1; then
  echo "FAIL guard record-checksums-refuses-a-second-checksum"
  failures=$((failures + 1))
else
  echo "ok   guard record-checksums-refuses-a-second-checksum"
fi

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

# ── a decision against the ref's present pins (D-168) ────────────────
# A run decided on an older checkout is superseded only when the ref's
# present pins no longer call for its decision; an unrelated commit is not.
stale=$work/stale
mkdir -p "$stale/origin/vendor/metadata/finney" "$stale/origin/vendor/metadata/testnet"
cp "$root/upstream.toml" "$stale/origin/"
cp "$root/vendor/metadata/finney/index.toml" "$stale/origin/vendor/metadata/finney/"
cp "$root/vendor/metadata/testnet/index.toml" "$stale/origin/vendor/metadata/testnet/"
staled() { git -C "$stale/origin" -c user.name=t -c user.email=t@invalid "$@"; }
git -C "$stale/origin" init -q -b main
staled add -A
staled commit -q -m base
git clone -q "$stale/origin" "$stale/checkout"
printf '%s\n' "$(observation "$next" "$other" "$testnet_spec" "$testnet_code" "$release")" >"$stale/observed.json"
bash "$script" decide "$stale/observed.json" "$stale/decision.json" >/dev/null
stale_current() { RTAO_ADOPT_CHECKOUT=$stale/checkout bash "$script" current "$stale/decision.json" "$stale/observed.json" refs/heads/main; }
stale_check() {
  if [[ "$2" == "$3" ]]; then
    echo "ok   current $1: $2"
  else
    echo "FAIL current $1: $2 (expected $3)"
    failures=$((failures + 1))
  fi
}
stale_check unmoved-ref "$(stale_current)" current
echo '# unrelated' >>"$stale/origin/upstream.toml"
staled commit -q -am unrelated
stale_check unrelated-commit "$(stale_current)" current
# The ref now carries the adoption this run decided on.
awk -v next_spec="$next" -v code="$other" '
  /^\[/ { section = $0 }
  section == "[parity]" && $1 == "spec_version" { $0 = "spec_version = " next_spec }
  section == "[networks.finney.metadata]" && $1 == "spec" { $0 = "spec = " next_spec }
  section == "[networks.finney.metadata]" && $1 == "code_hash" { $0 = "code_hash = \"" code "\"" }
  { print }
' "$stale/origin/upstream.toml" >"$stale/upstream.toml"
mv "$stale/upstream.toml" "$stale/origin/upstream.toml"
staled commit -q -am adopted
stale_check adopted-meanwhile "$(stale_current | cut -d: -f1)" superseded
stale_check other-actions-stand "$(RTAO_ADOPT_CHECKOUT=$stale/checkout bash "$script" current "$work/core-none.json" "$stale/observed.json" refs/heads/main)" current
stale_check unreadable-ref "$(RTAO_ADOPT_CHECKOUT=$stale/checkout bash "$script" current "$stale/decision.json" "$stale/observed.json" refs/heads/absent 2>/dev/null)" current

# ── republished packages (D-188) ──────────────────────────────────────
sed -n -e '/^fork_order=/p' -e '/^forks_marker=/p' -e '/^release_marker=/p' -e '/^crates_index=/p' \
  -e '/^publish_entries() {/,/^}/p' -e '/^forks_pending() {/,/^}/p' -e '/^forks_plan() {/,/^}/p' \
  -e '/^release_bump() {/,/^}/p' "$script" >"$work/forks.sh"
# shellcheck disable=SC1091
source "$work/forks.sh"
# forks_case <name> <expected: pass|refuse> <command> [jq predicate on its output]
forks_case() {
  local output got=refuse
  if output=$(eval "$3" 2>/dev/null) && jq -e "${4:-true}" <<<"$output" >/dev/null 2>&1; then got=pass; fi
  if [[ "$got" == "$2" ]]; then
    echo "ok   forks $1: $got"
  else
    echo "FAIL forks $1: expected $2, got $got (${output:-no output})"
    failures=$((failures + 1))
  fi
}
# pins_with <key> <version>: the committed pins, one package at a new version
# without a checksum (what forks refresh writes for a new revision).
pins_with() {
  awk -v key="$1" -v version="$2" '
    /^\[/ { inside = ($0 == "[publish." key "]") }
    inside && $1 == "checksum" { next }
    inside && $1 == "version" { $0 = "version = \"" version "\"" }
    { print }
  ' "$3"
}
pins=$root/upstream.toml
pins_core=$work/pins-core.toml
pins_core_bump=$work/pins-core-bump.toml
pins_with core 0.490.1 "$pins" >"$pins_core"
cp "$pins" "$pins_core_bump"
for key in "${fork_order[@]}"; do
  pins_with "$key" 0.491.0 "$pins_core_bump" >"$work/pins.next" && mv "$work/pins.next" "$pins_core_bump"
done
forks_case nothing-pending-on-main pass 'forks_pending "$pins"' '. == []'
forks_case adaptation-revision-pending pass 'forks_pending "$pins_core"' \
  '. == [{key: "core", package: "rtao-upstream-core", version: "0.490.1"}]'
forks_case core-bump-pending-in-order pass 'forks_pending "$pins_core_bump"' \
  'map(.package) == ["rtao-upstream-macros", "rtao-upstream-fips204", "rtao-upstream-w3f-bls", "rtao-upstream-hashed", "rtao-upstream-tle", "rtao-upstream-core"] and all(.version == "0.491.0")'
sed 's/^package = "rtao-upstream-core"/package = "rtao"/' "$pins_core" >"$work/pins-foreign.toml"
grep -v '^package = "rtao-upstream-tle"' "$pins_core" >"$work/pins-incomplete.toml"
forks_case refuses-a-foreign-package refuse 'forks_pending "$work/pins-foreign.toml"'
forks_case refuses-an-incomplete-record refuse 'forks_pending "$work/pins-incomplete.toml"'
forks_case release-refuses-an-incomplete-record refuse 'release_bump rtao-v0.1.0 "$work/pins-incomplete.toml" "$pins_core"'
printf 'rtao-upstream-%s\n' "${fork_order[@]}" >"$work/order.txt"
core_sha=$(printf 'core archive' | sha256sum | cut -c1-64)
printf 'rtao-upstream-macros 0.490.0: packaged archive verified, sha256 %s\nrtao-upstream-core 0.490.1: packaged archive verified, sha256 %s\n' \
  "$(printf 'a%.0s' {1..64})" "$core_sha" >"$work/verified.txt"
commit40=$(printf '%040d' 4)
forks_case plan-names-the-verified-archive pass 'forks_plan "$pins_core" "$work/order.txt" "$work/verified.txt" "$commit40" "$work/plan.json" >/dev/null && cat "$work/plan.json"' \
  ".commit == \"$commit40\" and .entries == [{key: \"core\", package: \"rtao-upstream-core\", version: \"0.490.1\", sha256: \"$core_sha\"}]"
forks_case plan-refuses-an-unverified-package refuse 'forks_plan "$pins_core_bump" "$work/order.txt" "$work/verified.txt" "$commit40" "$work/plan-bump.json"'
forks_case plan-refuses-another-order refuse 'forks_plan "$pins_core" <(tac "$work/order.txt") "$work/verified.txt" "$commit40" "$work/plan-order.json"'
# The proposed release: a moved core release is a minor bump before 1.0.
forks_case release-after-a-core-bump pass 'release_bump rtao-v0.1.0 "$pins" "$pins_core_bump"' \
  '.version == "0.2.0" and .bump == "minor" and (.changed | length) == 6 and all(.changed[]; .release)'
forks_case release-after-an-adaptation-revision pass 'release_bump rtao-v0.1.0 "$pins" "$pins_core"' \
  '.version == "0.1.1" and .bump == "patch" and .changed == [{package: "rtao-upstream-core", from: "0.490.0", to: "0.490.1", release: false}]'
forks_case release-after-1.0 pass 'release_bump rtao-v1.2.3 "$pins" "$pins_core_bump"' '.version == "2.0.0" and .bump == "major"'
forks_case no-release-without-new-packages pass 'release_bump rtao-v0.1.0 "$pins" "$pins"' '.version == null and .changed == []'

# Publication against stand-ins for cargo (archives and uploads) and curl
# (crates.io's sparse index kept as files): uploads in order, skips a version
# crates.io holds with the verified checksum, waits out rate limits and
# refuses anything that does not match the plan.
pub=$work/publish
mkdir -p "$pub/bin" "$pub/checkout"
cp "$pins_core" "$pub/checkout/upstream.toml"
git -C "$pub/checkout" init -q -b main
git -C "$pub/checkout" -c user.name=t -c user.email=t@invalid add -A
git -C "$pub/checkout" -c user.name=t -c user.email=t@invalid commit -q -m pending
pub_commit=$(git -C "$pub/checkout" rev-parse HEAD)
cat >"$pub/bin/cargo" <<'CARGO'
#!/usr/bin/env bash
set -euo pipefail
d=$FAKE_CARGO
echo "$1" >>"$d/calls"
package="" previous=""
for argument in "$@"; do
  [[ "$previous" != -p ]] || package=$argument
  previous=$argument
done
case "$1" in
  package)
    mkdir -p "$CARGO_TARGET_DIR/package"
    cp "$d/crate" "$CARGO_TARGET_DIR/package/$package-$(cat "$d/version").crate"
    ;;
  publish)
    response=$(head -1 "$d/responses" 2>/dev/null || true)
    sed -i 1d "$d/responses" 2>/dev/null || true
    if [[ "$response" == stale ]]; then
      # An earlier upload the index page did not show yet.
      printf '{"name":"%s","vers":"%s","cksum":"%s"}\n' "$package" "$(cat "$d/version")" \
        "$(sha256sum "$d/crate" | cut -c1-64)" >>"$d/index-$package"
      echo "error: crate version \`$(cat "$d/version")\` is already uploaded" >&2
      exit 101
    fi
    if [[ -z "$response" || "$response" == ok ]]; then
      printf '{"name":"%s","vers":"%s","cksum":"%s"}\n' "$package" "$(cat "$d/version")" \
        "$(sha256sum "$d/crate" | cut -c1-64)" >>"$d/index-$package"
      exit 0
    fi
    echo "error: failed to publish to registry at https://crates.io: $response" >&2
    exit 101
    ;;
esac
CARGO
cat >"$pub/bin/curl" <<'CURL'
#!/usr/bin/env bash
out="" url=""
while (($#)); do
  case "$1" in
    -o) out=$2; shift 2 ;;
    -w | -H | --max-time | --retry | --retry-delay) shift 2 ;;
    -*) shift ;;
    *) url=$1; shift ;;
  esac
done
name=${url##*/}
if [[ -f "$FAKE_CARGO/index-$name" ]]; then cp "$FAKE_CARGO/index-$name" "$out"; printf 200; else : >"$out"; printf 404; fi
CURL
chmod +x "$pub/bin/cargo" "$pub/bin/curl"
# publish_case <name> <expected: pass|refuse> <responses> <index line or ""> <crate bytes> <jq predicate on published.json> [plan edit]
publish_case() {
  local d=$pub/$1 got=refuse
  mkdir -p "$d/state"
  printf '%s' "$5" >"$d/state/crate"
  echo 0.490.1 >"$d/state/version"
  printf '%s' "$3" | tr ',' '\n' >"$d/state/responses"
  [[ -z "$4" ]] || printf '%s\n' "$4" >"$d/state/index-rtao-upstream-core"
  jq -n --arg commit "$pub_commit" --arg sha "$core_sha" \
    '{commit: $commit, entries: [{key: "core", package: "rtao-upstream-core", version: "0.490.1", sha256: $sha}]}' |
    jq "${7:-.}" >"$d/plan.json"
  if FAKE_CARGO=$d/state PATH=$pub/bin:$PATH CARGO_REGISTRY_TOKEN=x RTAO_ADOPT_CHECKOUT=$pub/checkout \
    CARGO_TARGET_DIR=$d/target RTAO_PUBLISH_PAUSE=0 RTAO_INDEX_PAUSE=0 RTAO_PUBLISH_ATTEMPTS=${PUBLISH_ATTEMPTS:-8} \
    bash "$script" forks-publish "$d/plan.json" "$d/published.json" >"$d/log" 2>&1 &&
    jq -e "$6" "$d/published.json" >/dev/null 2>&1; then
    got=pass
  fi
  # Checks on what was called, whatever the outcome.
  if [[ "$got" == "$2" ]] && jq -e -n --rawfile calls <(cat "$d/state/calls" 2>/dev/null) "${8:-true}" >/dev/null; then
    echo "ok   publish $1: $got ($(tr '\n' ' ' <"$d/state/calls" 2>/dev/null))"
  else
    echo "FAIL publish $1: expected $2, got $got; $(tail -3 "$d/log" | tr '\n' ' ')"
    failures=$((failures + 1))
  fi
}
uploads='[$calls | splits("\n") | select(. == "publish")] | length'
publish_case fresh pass ok "" 'core archive' '.[0].status == "published" and .[0].sha256 == "'"$core_sha"'"' . "$uploads == 1"
publish_case rate-limited pass '429 Too Many Requests,429 Too Many Requests,ok' "" 'core archive' '.[0].status == "published"' . "$uploads == 3"
publish_case uploaded-by-an-earlier-attempt pass stale "" 'core archive' '.[0].status == "published"' . "$uploads == 1"
publish_case already-published pass "" "{\"name\":\"rtao-upstream-core\",\"vers\":\"0.490.1\",\"cksum\":\"$core_sha\"}" 'core archive' \
  '.[0].status == "existing"' . '$calls == ""'
publish_case foreign-checksum refuse "" "{\"name\":\"rtao-upstream-core\",\"vers\":\"0.490.1\",\"cksum\":\"$(printf 'e%.0s' {1..64})\"}" 'core archive' \
  true . '$calls == ""'
publish_case archive-differs refuse ok "" 'other archive' true . "$uploads == 0"
publish_case other-error refuse '500 Internal Server Error,ok' "" 'core archive' true . "$uploads == 1"
PUBLISH_ATTEMPTS=2 publish_case rate-limit-exhausted refuse '429 Too Many Requests,429 Too Many Requests,ok' "" 'core archive' true . "$uploads == 2"
publish_case wrong-commit refuse ok "" 'core archive' true ".commit = \"$commit40\"" '$calls == ""'
publish_case plan-not-pending refuse ok "" 'core archive' true \
  '.entries[0] += {key: "macros", package: "rtao-upstream-macros", version: "0.490.0"}' '$calls == ""'
publish_case empty-plan refuse ok "" 'core archive' true '.entries = []' '$calls == ""'

# The publication's issue, alert and release proposal (stand-in gh and curl
# from the issue loop above).
forks_fake=$work/forks-fake
mkdir -p "$forks_fake" "$work/forks-evidence"
forks_pending "$pins_core_bump" >"$work/forks-evidence/pending.json"
record_checksums_of() { awk '{ print } /^\[publish\./ { key = $0 } $1 == "path" && key != "" { print "checksum = \"" sprintf("%064d", 9) "\"" }' "$1"; }
record_checksums_of "$pins_core_bump" >"$work/forks-evidence/upstream.toml"
forks_loop() { FAKE_GH=$forks_fake PATH=$fake/bin:$PATH GITHUB_REPOSITORY=owner/repo GH_TOKEN=x \
  RUNTIME_ALERT_URL=https://alerts.invalid RUNTIME_ALERT_TOKEN=x RTAO_RELEASE_TAG=rtao-v0.1.0 RTAO_RELEASE_TAGGED_PINS=$pins \
  bash "$script" forks-notify "$@" "$work/forks-evidence" >/dev/null; }
forks_alerts() { jq -s -c 'map(select(.id | startswith("forks-")) | .open)' "$forks_fake/alerts" 2>/dev/null || echo '[]'; }
forks_check() {
  if eval "$2"; then
    echo "ok   forks-notify $1"
  else
    echo "FAIL forks-notify $1 (issues: $(ls "$forks_fake" | tr '\n' ' '), alerts: $(forks_alerts))"
    failures=$((failures + 1))
  fi
}
forks_loop failure publish
forks_check failure-opens-one-issue-and-alerts '[[ -f $forks_fake/1.open && "$(forks_alerts)" == "[true]" ]] && grep -q "failed at stage publish" $forks_fake/log'
forks_loop failure publish
forks_check repeat-is-quiet '[[ "$(forks_alerts)" == "[true]" && "$(grep -c "^edit" $forks_fake/log || true)" == 0 ]]'
forks_loop failure verify
forks_check another-stage-alerts-again '[[ "$(forks_alerts)" == "[true,true]" ]] && grep -q "^edit 1 .*stage verify" $forks_fake/log'
forks_loop success none
forks_check success-closes-resolves-and-proposes '[[ -f $forks_fake/1.closed && "$(forks_alerts)" == "[true,true,false]" && -f $forks_fake/2.open ]] &&
  grep -q "^create 2 Release proposal: rtao v0.2.0" $forks_fake/log && grep -q "rtao-upstream-core. 0.490.0 -> 0.491.0 (core release)" $forks_fake/2.open &&
  ! grep -qi "cargo publish -p" $forks_fake/2.open'
forks_loop success none
forks_check a-later-publication-refreshes-the-proposal '[[ ! -f $forks_fake/3.open ]] && grep -q "^edit 2 Release proposal" $forks_fake/log'
forks_loop none none
forks_check nothing-pending-is-quiet '[[ "$(forks_alerts)" == "[true,true,false]" && "$(grep -c "^close" $forks_fake/log)" == 1 ]]'

if ((failures)); then
  echo "$failures decision checks failed" >&2
  exit 1
fi
echo "all decision checks passed"
