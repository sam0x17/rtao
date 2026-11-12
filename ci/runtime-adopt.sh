#!/usr/bin/env bash
# Automatic adoption of activated Finney and testnet runtimes (AGENTS.md's
# runtime adoption policy), run hourly by .github/workflows/runtime-adopt.yml.
#
#   ci/runtime-adopt.sh observe <observed.json>
#   ci/runtime-adopt.sh decide <observed.json> <decision.json> [<pins-root>]
#   ci/runtime-adopt.sh current <decision.json> <observed.json> [<ref>]
#   ci/runtime-adopt.sh adopt <decision.json> <candidate> <evidence-dir>
#   ci/runtime-adopt.sh commit <decision.json> <candidate> <bundle>
#   ci/runtime-adopt.sh push <decision.json> <bundle>
#   ci/runtime-adopt.sh notify <decision.json> <outcome> [<evidence-dir>]
#   ci/runtime-adopt.sh evidence <decision.json> <evidence-dir>
#   ci/runtime-adopt.sh verdict <live.json>
#   ci/runtime-adopt.sh state <decision.json> <outcome> <stage> [<previous-state.json>]
#   ci/runtime-adopt.sh forks-pending <upstream.toml>
#   ci/runtime-adopt.sh forks-package <out-dir>
#   ci/runtime-adopt.sh forks-plan <upstream.toml> <order.txt> <verified.txt> <commit> <plan.json>
#   ci/runtime-adopt.sh forks-publish <plan.json> <published.json>
#   ci/runtime-adopt.sh forks-record <plan.json> <published.json> <candidate> <evidence-dir>
#   ci/runtime-adopt.sh forks-notify <outcome> <stage> [<evidence-dir>]
#   ci/runtime-adopt.sh release-bump <tag> <tagged-upstream.toml> <upstream.toml>
#
# observe reads public chain RPC and the public upstream repository, re-reads
# the finalized runtime from independent witness endpoints, and records the
# open adoption issue's backoff state and a fingerprint of the adoption
# inputs. decide is a pure function of an observation and the committed pins:
# it writes the one action of this run and needs no network, so
# ci/runtime-adopt-test.sh checks it against recorded and simulated
# observations, as it checks verdict (the live-evidence rule) and state (the
# backoff bookkeeping).
#
# Actions:
#   none            Finney and testnet run indexed runtimes.
#   adopt-finney    Finney finalized a newer spec than the parity pin: pin it
#                   (published release vN, else the release-N merge on
#                   upstream main), import, extract, regenerate, reconcile and
#                   check; when every stage passes, commit, push and tag a
#                   snapshot with a changelog entry.
#   record-testnet  Testnet finalized a runtime its index lacks: record it
#                   with `drift --network testnet --apply` and check.
#   core-bump       Only when a dispatch asks for it (core_bump): the core is
#                   held at an older tag while the parity pin's core inputs
#                   moved ([parity] core_fingerprint, D-127) and Finney runs
#                   the parity runtime. Pin the parity source again with the
#                   core bump (core-patch rebase onto the parity tag, every
#                   downstream core reference, the lock), then check it like
#                   an adoption and commit, push and tag it (D-136).
#   blocked         Something only a maintainer can decide (same spec with
#                   new :code, no upstream source for a new spec, a pin ahead
#                   of its network, endpoints that disagree about the
#                   finalized runtime): nothing runs; an issue and an alert.
#   backoff         The same adoption failed before with the same inputs and
#                   its retry delay (1 h doubling per failed attempt, at most
#                   24 h, from when the last attempt failed) has not passed:
#                   nothing runs; the issue stays open. A
#                   change to the adoption inputs (xtask, ci, crates, parity,
#                   codegen, vendor, pins, lock, toolchain, this workflow) or a
#                   forced dispatch retries at once.
# A new runtime is acted on only when a second endpoint corroborates it at the
# same finalized block, which it holds on its own finalized chain; with none
# answering the run waits (no issue).
# A red stage leaves main untouched and reports like blocked. Upstream
# branches that declare a newer spec than any network runs are listed as
# upcoming (an informational issue, no alert) so they can be pre-staged.
#
# Republished core packages (D-188): every run also looks at main's
# upstream.toml [publish]. A core bump or a new core adaptation revision
# gives rtao-upstream-* packages new versions without a recorded checksum.
# Those are packaged and verified against their revisions (byte identity and
# the Ed25519 erasure check) in a read-only job, uploaded in dependency order
# by a job that holds the crates.io token and builds nothing (an upload
# happens only when the archive packaged there has the verified SHA256, and
# a version crates.io already holds is skipped when its checksum is that
# SHA256), checked again with forks verify-published, and their checksums are
# committed by the bot. Success opens an issue proposing an rtao release;
# rtao's own crates are never published here. Any failure commits nothing,
# opens an issue and alerts.
#
# A Finney adoption approves itself on evidence: the pinned runtime's :code
# (hashed here with BLAKE2b-256), its V15/V16 metadata digests and spec read
# at the pinned block, which each holds on its own finalized chain, from at
# least two endpoints that all agree, the release
# wasm when upstream published one, then the pin, import, extraction,
# regeneration, compatibility, drift, parity, build, tests and a live read
# with the built CLI. Parity runs in adoption mode: unchanged and cosmetic
# sources are reconciled, generated/extracted/vendored ones re-derived and
# checked, and changed hand ports are listed in parity/port_due.toml (one
# informational issue, no alert) instead of holding the runtime back.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
upstream_repo=RaoFoundation/subtensor
adopt_marker='<!-- rtao-runtime-adopt -->'
upcoming_marker='<!-- rtao-runtime-upcoming -->'
port_due_marker='<!-- rtao-parity-port-due -->'

die() {
  echo "runtime-adopt: $*" >&2
  exit 1
}

# One scalar from a [section] of a flat, generated TOML file.
toml_get() {
  awk -v section="[$2]" -v key="$3" '
    skip { next }
    /^\[/ { inside = ($0 == section); next }
    inside && $1 == key && $2 == "=" {
      value = $0
      sub(/^[^=]*=[ \t]*/, "", value)
      gsub(/^"|"$/, "", value)
      print value
      found = 1
      # Read on instead of exiting: a producer piping into this (git show
      # in push) would otherwise die of SIGPIPE under pipefail.
      skip = 1
    }
    END { exit !found }
  ' "$1"
}

# The [[runtimes]] rows of a metadata index as a JSON array of {spec, code}.
index_rows() {
  if [[ ! -f "$1" ]]; then
    echo '[]'
    return
  fi
  awk '
    function flush() { if (spec != "") printf "%s %s\n", spec, code; spec = ""; code = "" }
    /^\[\[runtimes\]\]/ { flush(); next }
    $1 == "spec" && $2 == "=" { spec = $3 }
    $1 == "code_hash" && $2 == "=" { code = $3; gsub(/"/, "", code) }
    END { flush() }
  ' "$1" | jq -Rn '[inputs | split(" ") | {spec: (.[0] | tonumber), code: .[1]}]'
}

# ── observe ───────────────────────────────────────────────────────────

rpc() {
  curl -sS --fail --max-time 20 --retry 2 --retry-delay 3 \
    -H 'Content-Type: application/json' \
    --data "$(jq -cn --arg method "$2" --argjson params "$3" \
      '{id: 1, jsonrpc: "2.0", method: $method, params: $params}')" "$1" |
    jq -ce '.result // empty'
}

# A hex block number (0x...) as decimal; empty when malformed.
block_number() {
  if [[ "${1:-}" =~ ^0x[0-9a-fA-F]{1,15}$ ]]; then echo $((16#${1#0x})); fi
}

# Whether an endpoint holds a block on its own finalized chain: "true" when
# its canonical hash at the block's number is the block and it has finalized
# that number, "false" when it finalized another block at that number,
# "unfinalized" when it has not finalized that far (re-read once after a
# short pause, for an endpoint a block or two behind), empty when it cannot
# say. A single endpoint's finalized head is never taken on trust.
on_finalized_chain() {
  local endpoint=$1 block=$2 number=$3 canonical head final
  canonical=$(rpc "$endpoint" chain_getBlockHash "[$number]" 2>/dev/null | jq -r .) || return 0
  local attempt
  for attempt in 1 2; do
    head=$(rpc "$endpoint" chain_getFinalizedHead '[]' 2>/dev/null | jq -r .) || return 0
    final=$(block_number "$(rpc "$endpoint" chain_getHeader "[\"$head\"]" 2>/dev/null | jq -r .number)") || return 0
    [[ -n "$final" ]] || return 0
    if ((final >= number)); then
      [[ "$canonical" == "$block" ]] && echo true || echo false
      return 0
    fi
    ((attempt == 2)) || sleep "${RTAO_ADOPT_LAG_PAUSE:-12}"
    canonical=$(rpc "$endpoint" chain_getBlockHash "[$number]" 2>/dev/null | jq -r .) || return 0
  done
  echo unfinalized
}

# One witness: the runtime another endpoint reports at the primary's
# finalized block, or {"endpoint", "error"}. An endpoint that does not know
# the block (lagging, pruned) or has not finalized it yet is an error, not a
# disagreement; one that finalized another block at that height disagrees
# (canonical false). The first failed read ends the witness, so a hung
# endpoint costs one read's retries.
observe_witness() {
  local endpoint=$1 block=$2 number=$3 genesis version code canonical
  local none='no complete runtime at the block'
  genesis=$(rpc "$endpoint" chain_getBlockHash '[0]' 2>/dev/null | jq -r .) &&
    version=$(rpc "$endpoint" state_getRuntimeVersion "[\"$block\"]" 2>/dev/null) &&
    code=$(rpc "$endpoint" state_getStorageHash "[\"0x3a636f6465\", \"$block\"]" 2>/dev/null | jq -r .) || {
    jq -cn --arg endpoint "$endpoint" --arg error "$none" '{endpoint: $endpoint, error: $error}'
    return
  }
  if [[ ! "$genesis" =~ ^0x[0-9a-f]{64}$ || ! "$code" =~ ^0x[0-9a-f]{64}$ || -z "$version" ]]; then
    jq -cn --arg endpoint "$endpoint" --arg error "$none" '{endpoint: $endpoint, error: $error}'
    return
  fi
  canonical=$(on_finalized_chain "$endpoint" "$block" "$number")
  case "$canonical" in
    true | false) ;;
    unfinalized)
      jq -cn --arg endpoint "$endpoint" --argjson number "$number" \
        '{endpoint: $endpoint, error: "has not finalized block \($number) yet"}'
      return
      ;;
    *)
      jq -cn --arg endpoint "$endpoint" '{endpoint: $endpoint, error: "finalized chain unavailable"}'
      return
      ;;
  esac
  jq -cn --arg endpoint "$endpoint" --arg genesis "$genesis" --argjson version "$version" --arg code "$code" \
    --argjson canonical "$canonical" \
    '{endpoint: $endpoint, genesis: $genesis, spec: $version.specVersion, spec_name: $version.specName, code: $code,
      canonical: $canonical}'
}

# The finalized runtime of a network as JSON, or {"error": ...}, with the
# witnesses' view of the same block.
observe_network() {
  local name=$1 genesis=$2 endpoints=$3 witnesses=${4:-} endpoint="" candidate observed
  for candidate in $endpoints; do
    if observed=$(rpc "$candidate" chain_getBlockHash '[0]' 2>/dev/null | jq -r .); then
      if [[ "$observed" == "$genesis" ]]; then
        endpoint=$candidate
        break
      fi
    fi
  done
  if [[ -z "$endpoint" ]]; then
    jq -cn --arg error "no $name endpoint answered with genesis $genesis" '{error: $error}'
    return
  fi
  local finalized number version code
  finalized=$(rpc "$endpoint" chain_getFinalizedHead '[]' | jq -r .) || finalized=
  number=$(rpc "$endpoint" chain_getHeader "[\"$finalized\"]" | jq -r .number) || number=
  version=$(rpc "$endpoint" state_getRuntimeVersion "[\"$finalized\"]") || version=
  code=$(rpc "$endpoint" state_getStorageHash "[\"0x3a636f6465\", \"$finalized\"]" | jq -r .) || code=
  if [[ ! "$finalized" =~ ^0x[0-9a-f]{64}$ || ! "$code" =~ ^0x[0-9a-f]{64}$ || -z "$version" || -z "$number" ]]; then
    jq -cn --arg error "$name endpoint $endpoint gave no complete finalized runtime" '{error: $error}'
    return
  fi
  local seen='[]' witness decimal
  decimal=$(block_number "$number")
  [[ -n "$decimal" ]] || { jq -cn --arg error "$name endpoint $endpoint gave block number $number" '{error: $error}'; return; }
  for witness in $witnesses; do
    [[ "$witness" != "$endpoint" ]] || continue
    seen=$(jq -c --argjson row "$(observe_witness "$witness" "$finalized" "$decimal")" '. + [$row]' <<<"$seen")
  done
  jq -cn --arg endpoint "$endpoint" --arg finalized "$finalized" --arg number "$number" \
    --argjson version "$version" --arg code "$code" --arg genesis "$genesis" --argjson witnesses "$seen" \
    '{endpoint: $endpoint, genesis: $genesis, finalized: $finalized, number: ($number | ltrimstr("0x") | explode | reduce .[] as $c (0; . * 16 + (if $c >= 97 then $c - 87 elif $c >= 65 then $c - 55 else $c - 48 end))),
      spec: $version.specVersion, spec_name: $version.specName,
      transaction_version: $version.transactionVersion, code: $code, witnesses: $witnesses}'
}

# Public Finney endpoints that re-read the primary's finalized block. One is
# run by a different operator than the primary; agreement from two operators
# is what a normal run sees, but the rule needs only two agreeing endpoints
# and no disagreeing one, so one operator's outage does not stop adoption.
finney_witnesses=${FINNEY_WITNESSES:-https://archive.chain.opentensor.ai:443 https://bittensor-finney.api.onfinality.io/public https://lite.chain.opentensor.ai:443}
# Testnet's second public name, so a testnet record is corroborated too.
testnet_witnesses=${TESTNET_WITNESSES-https://test.chain.opentensor.ai:443}

# A fingerprint of everything an adoption's outcome depends on, so a fix
# merged to main retries a backed-off adoption at once.
adopt_inputs() {
  local path objects=""
  for path in xtask ci crates parity codegen vendor patches .cargo upstream.toml Cargo.toml Cargo.lock \
    rust-toolchain.toml .github/workflows/runtime-adopt.yml; do
    objects+="$path $(git -C "$root" rev-parse -q --verify "HEAD:$path" 2>/dev/null || echo absent)"$'\n'
  done
  printf '%s' "$objects" | sha256sum | cut -c1-16
}

# The backoff states recorded in the open adoption issue ({id: state}), or
# null. One issue can track a failing adoption and another network's block.
previous_state() {
  local number body
  if [[ -z "${GITHUB_REPOSITORY:-}" ]] || ! command -v gh >/dev/null; then
    echo null
    return
  fi
  number=$(open_issue "$adopt_marker" 2>/dev/null || true)
  [[ -n "$number" ]] || { echo null; return; }
  body=$(gh issue view "$number" --repo "$GITHUB_REPOSITORY" --json body --jq .body 2>/dev/null || true)
  state_from_body <<<"$body"
}

# A single recorded state (an issue written before states were keyed by id)
# reads as a map of one.
state_from_body() {
  sed -n 's/^<!-- rtao-runtime-adopt-state \({.*}\) -->$/\1/p' | head -1 |
    jq -c 'select(type == "object") | if (.id | type) == "string" then {(.id): .} else . end' 2>/dev/null | grep . || echo null
}

gh_raw() { gh api -H 'Accept: application/vnd.github.raw+json' "$@"; }

# The spec_version a commit's runtime source declares.
declared_spec() {
  local source
  source=$(gh_raw "repos/$upstream_repo/contents/runtime/src/lib.rs?ref=$1" 2>/dev/null) || return 1
  awk -F'spec_version:' 'NF > 1 && !found { sub(/^[ \t]*/, "", $2); sub(/[^0-9].*/, "", $2); print $2; found = 1 }' <<<"$source"
}

# What upstream offers for one spec: a published final release, and the
# newest release-N merge on main that declares it.
upstream_source() {
  local spec=$1 release=null merge=""
  if gh api "repos/$upstream_repo/releases/tags/v$spec" --jq '{tag: .tag_name, prerelease, draft}' >"$work/release.json" 2>/dev/null; then
    release=$(cat "$work/release.json")
  fi
  local sha parents subject
  while IFS=$'\t' read -r sha parents subject; do
    [[ -n "$sha" ]] || continue
    if ((parents > 1)) && [[ "$(declared_spec "$sha" || true)" == "$spec" ]]; then
      merge=$sha
      break
    fi
  done < <(gh api "repos/$upstream_repo/commits?sha=main&per_page=100" --jq \
    ".[] | select(.commit.message | test(\"release[-/_]?[^0-9]*$spec([^0-9]|$)\"; \"i\")) | [.sha, (.parents | length), (.commit.message | split(\"\\n\")[0])] | @tsv" 2>/dev/null || true)
  jq -cn --argjson release "$release" --arg merge "$merge" \
    '{release: $release, main_merge: (if $merge == "" then null else $merge end)}'
}

observe() {
  local out=${1:?observe <observed.json>}
  work=$(mktemp -d)
  trap 'rm -rf "$work"' RETURN
  local pins=$root/upstream.toml
  local finney testnet
  finney=$(observe_network finney "$(toml_get "$pins" networks.finney genesis)" \
    "${FINNEY_ENDPOINTS:-$(toml_get "$pins" networks.finney genesis_endpoint) https://archive.chain.opentensor.ai:443}" \
    "$finney_witnesses")
  testnet=$(observe_network testnet "$(toml_get "$pins" networks.testnet genesis)" \
    "${TESTNET_ENDPOINTS:-$(toml_get "$pins" networks.testnet genesis_endpoint)}" "$testnet_witnesses")
  local branches='{}' sources='{}' head ref spec
  if command -v gh >/dev/null; then
    while read -r head ref; do
      [[ -n "$head" ]] || continue
      spec=$(declared_spec "$head" || true)
      branches=$(jq -c --arg branch "${ref#refs/heads/}" --arg head "$head" --arg spec "$spec" \
        '. + {($branch): {head: $head, spec: (if $spec == "" then null else ($spec | tonumber) end)}}' <<<"$branches")
    done < <(git ls-remote --heads "https://github.com/$upstream_repo.git" main testnet devnet 2>/dev/null || true)
    local pinned
    pinned=$(toml_get "$pins" parity spec_version)
    spec=$(jq -r '.spec // empty' <<<"$finney")
    if [[ "$spec" =~ ^[0-9]+$ ]] && ((spec > pinned)); then
      sources=$(jq -c --arg spec "$spec" --argjson source "$(upstream_source "$spec")" '. + {($spec): $source}' <<<"$sources")
    fi
  fi
  jq -n --arg at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" --argjson finney "$finney" --argjson testnet "$testnet" \
    --argjson branches "$branches" --argjson sources "$sources" \
    --arg inputs "$(adopt_inputs)" --argjson previous "$(previous_state)" \
    --argjson force "$([[ "${RTAO_ADOPT_FORCE:-}" == true || "${RTAO_ADOPT_FORCE:-}" == 1 ]] && echo true || echo false)" \
    --argjson core_bump "$([[ "${RTAO_ADOPT_CORE_BUMP:-}" == true || "${RTAO_ADOPT_CORE_BUMP:-}" == 1 ]] && echo true || echo false)" \
    '{schema: 1, observed_at: $at, finney: $finney, testnet: $testnet, upstream: {branches: $branches, sources: $sources},
      inputs: $inputs, previous: $previous, force: $force, core_bump: $core_bump}' >"$out"
  jq -c '{finney: (.finney | {spec, code, number, error, witnesses: ((.witnesses // []) | map({endpoint, spec, code, error}))}),
          testnet: (.testnet | {spec, code, number, error}), upstream: .upstream.branches, inputs, previous, force, core_bump}' "$out"
}

# ── decide ────────────────────────────────────────────────────────────

decide() {
  local observed=${1:?decide <observed.json> <decision.json> [<pins-root>]}
  local out=${2:?decide <observed.json> <decision.json> [<pins-root>]}
  local base=${3:-$root}
  local pins=$base/upstream.toml
  local parity_spec parity_tag parity_network finney_spec finney_code core_tag core_held core_source
  parity_spec=$(toml_get "$pins" parity spec_version)
  parity_tag=$(toml_get "$pins" parity tag)
  parity_network=$(toml_get "$pins" parity network || echo finney)
  core_tag=$(toml_get "$pins" core tag)
  # Present only while the core is held behind the parity pin (D-127).
  core_held=$(toml_get "$pins" parity core_fingerprint || true)
  # The core bump re-pins the parity pin's own source.
  if [[ "$(toml_get "$pins" parity release || true)" == unreleased ]]; then
    core_source="branch:$(toml_get "$pins" parity source_branch)@$(toml_get "$pins" parity commit)"
  else
    core_source=$parity_tag
  fi
  finney_spec=$(toml_get "$pins" networks.finney.metadata spec)
  finney_code=$(toml_get "$pins" networks.finney.metadata code_hash)
  jq -n \
    --slurpfile observed "$observed" \
    --argjson parity_spec "$parity_spec" --arg parity_tag "$parity_tag" --arg parity_network "$parity_network" \
    --argjson finney_spec "$finney_spec" --arg finney_code "$finney_code" \
    --arg core_tag "$core_tag" --arg core_held "$core_held" --arg core_source "$core_source" \
    --argjson finney_rows "$(index_rows "$base/vendor/metadata/finney/index.toml")" \
    --argjson testnet_rows "$(index_rows "$base/vendor/metadata/testnet/index.toml")" \
    -f /dev/stdin >"$out" <<'JQ'
($observed[0]) as $o
| ($o.finney) as $f
| ($o.testnet) as $t
| def short: .[0:10];
  def known($rows; $spec; $code): any($rows[]; .spec == $spec and .code == $code);
  def spec_known($rows; $spec): any($rows[]; .spec == $spec);
  def result($action; $network; $spec; $code; $reason):
    {action: $action, network: $network, spec: $spec, code: $code, reason: $reason,
     id: (if $network == null then "none" else "\($network)-\($spec)-\($code | short)" end)};
  # Witnesses re-read the primary's finalized block and check it is on their
  # own finalized chain. A result is acted on only when one corroborates it
  # (when witnesses are configured); any answering witness that disagrees,
  # or finalized another block at that height, blocks it. Unanswered ones
  # (unreachable, pruned, not finalized that far yet) are ignored.
  def corroborate($n; $name):
    if . == null then null
    else
      (($n.witnesses // []) | map(select((.error // null) == null))) as $answered
      | def agrees: .genesis == ($n.genesis // .genesis) and .spec == $n.spec and .code == $n.code
          and (.spec_name // $n.spec_name) == $n.spec_name and .canonical == true;
        ($answered | map(select(agrees))) as $agree
      | ($answered | map(select(agrees | not))) as $disagree
      | if ($disagree | length) > 0 then
          result("blocked"; $name; $n.spec; $n.code;
            "endpoints disagree about \($name)'s finalized runtime at block \($n.number) (\($n.finalized | short)…): \($n.endpoint) reports spec \($n.spec) with :code \($n.code | short)…, " +
            ($disagree | map("\(.endpoint) reports spec \(.spec) with :code \(.code | short)…"
              + (if .genesis != ($n.genesis // .genesis) then " on another genesis" else "" end)
              + (if .canonical != true then " and finalized another block at that height" else "" end)) | join("; ")))
          + {evidence: "inconsistent"}
        elif (($n.witnesses // []) | length) > 0 and ($agree | length) == 0 then
          {wait: "\($name): no witness endpoint corroborated spec \($n.spec) at block \($n.number) yet"}
        else . + {witnesses: ($agree | map(.endpoint))} end
    end;
  # A retry of an adoption that failed before, with the same inputs, waits
  # 1 h doubling per failed attempt (at most 24 h) from the end of the last
  # attempt, less five minutes of slack. A blocked report is no attempt.
  def backoff:
    (($o.previous // {})[.id] // null) as $p
    | if (.action == "adopt-finney" or .action == "record-testnet" or .action == "core-bump")
       and (($o.force // false) | not)
       and $p != null and ($p.attempted_at // null) != null and ($p.failures // 1) >= 1
       and ($p.inputs // null) == ($o.inputs // null)
    then
      ([pow(2; (($p.failures // 1) - 1)) * 3600, 86400] | min) as $delay
      | (($p.attempted_at | fromdateiso8601) + $delay - 300) as $retry
      | if ($o.observed_at | fromdateiso8601) < $retry then
          . + {action: "backoff", wanted: .action, retry_at: ($retry | todate),
               reason: "\(.reason); the last attempt failed at stage \($p.stage) (\($p.failures) failed attempt(s) with these inputs), so the next attempt waits until \($retry | todate) or a change to the adoption inputs"}
        else . end
    else . end;
  # Finney first: the parity pin follows the runtime mainnet finalizes.
  (if ($f.error // null) != null then null
   elif $f.spec_name != null and $f.spec_name != "node-subtensor" then
     result("blocked"; "finney"; $f.spec; $f.code; "Finney's finalized spec_name is \($f.spec_name), not node-subtensor")
   elif $f.spec == $parity_spec and $f.spec == $finney_spec and $f.code == $finney_code then null
   elif $f.spec > $parity_spec then
     ($o.upstream.sources[($f.spec | tostring)] // {}) as $s
     | if ($s.release // null) != null and ($s.release.prerelease | not) and ($s.release.draft | not) then
         result("adopt-finney"; "finney"; $f.spec; $f.code; "Finney finalized spec \($f.spec); parity pins \($parity_tag)")
         + {source: "v\($f.spec)"}
       elif ($s.release // null) != null then
         # pin refuses a prerelease as a release, and refuses the unreleased
         # merge while any release (a prerelease included) carries the tag.
         result("blocked"; "finney"; $f.spec; $f.code; "Finney finalized spec \($f.spec), but upstream's v\($f.spec) is a prerelease or draft: pin accepts only a final release, and no unreleased merge while that release exists")
       elif ($s.main_merge // null) != null then
         result("adopt-finney"; "finney"; $f.spec; $f.code; "Finney finalized spec \($f.spec); parity pins \($parity_tag); upstream has not published v\($f.spec), so the release-\($f.spec) merge on main is pinned")
         + {source: "branch:main@\($s.main_merge)"}
       else
         result("blocked"; "finney"; $f.spec; $f.code; "Finney finalized spec \($f.spec), but upstream offers neither a final v\($f.spec) release nor a release-\($f.spec) merge on main to pin")
       end
   elif $f.spec == $parity_spec then
     result("blocked"; "finney"; $f.spec; $f.code; "Finney's :code changed at spec \($f.spec) (pinned \($finney_code | short)…, finalized \($f.code | short)…); a same-spec code change needs a reviewed index migration")
   elif $parity_network == "finney" then
     result("blocked"; "finney"; $f.spec; $f.code; "the parity pin (spec \($parity_spec)) is ahead of Finney's finalized spec \($f.spec)")
   else null end | corroborate($f; "finney")) as $finney_raw
| (if ($t.error // null) != null then null
   elif known($testnet_rows; $t.spec; $t.code) then null
   elif spec_known($testnet_rows; $t.spec) then
     result("blocked"; "testnet"; $t.spec; $t.code; "testnet's :code changed at indexed spec \($t.spec); immutable metadata cannot be overwritten")
   elif $parity_network == "testnet" and $t.spec > $parity_spec then
     result("blocked"; "testnet"; $t.spec; $t.code; "the parity pin follows testnet; spec \($t.spec) needs a pin, not an index record")
   else
     result("record-testnet"; "testnet"; $t.spec; $t.code; "testnet finalized spec \($t.spec) (:code \($t.code | short)…), absent from its index")
   end | corroborate($t; "testnet")) as $testnet_raw
| ([$finney_raw, $testnet_raw] | map(select(. != null and .wait != null) | .wait)) as $waits
| (if ($finney_raw.wait // null) != null then null else $finney_raw end | if . == null then null else backoff end) as $finney
| (if ($testnet_raw.wait // null) != null then null else $testnet_raw end | if . == null then null else backoff end) as $testnet
# A requested core bump runs only while the core is held and Finney runs the
# parity runtime, so the live evidence reads the pinned runtime (D-136).
| (if ($o.core_bump // false) != true or $core_held == "" then null
   elif ($f.error // null) != null or $finney_raw != null then null
   else result("core-bump"; "finney"; $parity_spec; $finney_code;
          "a dispatch requested the core bump: the core is held at \($core_tag) while \($parity_tag)'s core inputs moved (D-127)")
        + {id: "core-\($parity_tag)-\($core_held | ltrimstr("b3:") | .[0:10])", source: $core_source,
           core: {from: $core_tag, to: $parity_tag, fingerprint: $core_held}}
     | backoff end) as $core
| (if ($o.core_bump // false) != true then null
   elif $core != null then "requested"
   elif $core_held == "" then "requested, but none is due: the core already matches the parity pin's core inputs"
   else "requested, but it waits until Finney is observed on the parity runtime" end) as $core_note
| ([$f.spec, $t.spec, $parity_spec] | map(select(. != null)) | max) as $running
| ($o.upstream.branches // {} | to_entries
   | map(select(.value.spec != null and .value.spec > $running)
         | {branch: .key, head: .value.head, spec: .value.spec})) as $upcoming
| ([$finney, $testnet, $core] | map(select(. != null))) as $wanted
# Progress first: a blocked network never holds up the other's adoption. A
# requested core bump goes before a testnet record (the next run records it).
| ($wanted | map(select(.action == "adopt-finney")) + map(select(.action == "core-bump"))
   + map(select(.action == "record-testnet"))
   + map(select(.action == "blocked")) + map(select(.action == "backoff"))) as $ordered
| ($ordered[0] // result("none"; null; null; null; "Finney and testnet run indexed runtimes"))
| . + {deferred: ($ordered[1:] | map({action, network, spec, id, reason})),
       upcoming: $upcoming,
       unobserved: ([(if ($f.error // null) != null then "finney: \($f.error)" else empty end),
                     (if ($t.error // null) != null then "testnet: \($t.error)" else empty end)] + $waits),
       pins: {parity_tag: $parity_tag, parity_spec: $parity_spec, finney_spec: $finney_spec, finney_code: $finney_code},
       observed: {finney: ($f | {spec, code, number, finalized, endpoint, error, witnesses}),
                  testnet: ($t | {spec, code, number, finalized, endpoint, error, witnesses})},
       inputs: ($o.inputs // null), previous: ($o.previous // null),
       core_bump: $core_note, observed_at: $o.observed_at}
JQ
  jq -c '{action, network, spec, source, id, reason, retry_at, deferred: (.deferred | map(.id)), upcoming: (.upcoming | map("\(.branch)=\(.spec)")), unobserved, core_bump}' "$out"
}

# ── current ───────────────────────────────────────────────────────────

# Whether a decision still stands against the pins on the run's ref now
# (D-168). A run decides on its checkout's pins; a scheduled run that queued
# behind another (the concurrency group) or one the ref moved under can
# decide, fail and report an adoption that the ref already carries, and its
# notify job would read the hand ports due from that stale checkout. Prints
# "current" or "superseded: <reason>". Only when the same observation decided
# against the ref's present pins no longer calls for this decision's id; an
# unrelated commit leaves the id, and so the report, unchanged. A ref that
# cannot be read counts as current: the run reports as it always has.
current() {
  local decision=${1:?current <decision.json> <observed.json> [<ref>]}
  local observed=${2:?current <decision.json> <observed.json> [<ref>]}
  local ref=${3:-${GITHUB_REF:-refs/heads/main}}
  local checkout=${RTAO_ADOPT_CHECKOUT:-$root}
  local id tip work now path
  id=$(jq -r .id "$decision")
  case "$(jq -r .action "$decision")" in
    adopt-finney | record-testnet | core-bump | blocked) ;;
    *) echo current && return 0 ;;
  esac
  if ! git -C "$checkout" fetch -q --depth=1 --no-tags origin "$ref" 2>/dev/null; then
    echo "runtime-adopt: could not read $ref; reporting this run as decided" >&2
    echo current
    return 0
  fi
  tip=$(git -C "$checkout" rev-parse FETCH_HEAD)
  if [[ "$tip" == "$(git -C "$checkout" rev-parse HEAD)" ]]; then
    echo current
    return 0
  fi
  work=$(mktemp -d)
  mkdir -p "$work/vendor/metadata/finney" "$work/vendor/metadata/testnet"
  for path in upstream.toml vendor/metadata/finney/index.toml vendor/metadata/testnet/index.toml; do
    git -C "$checkout" show "$tip:$path" >"$work/$path" 2>/dev/null || rm -f "$work/$path"
  done
  now=""
  if [[ -f "$work/upstream.toml" ]]; then
    now=$( (decide "$observed" "$work/decision.json" "$work" >/dev/null && jq -r .id "$work/decision.json") 2>/dev/null) || now=""
  fi
  rm -rf "$work"
  if [[ -n "$now" && "$now" != "$id" ]]; then
    echo "superseded: $ref moved to ${tip:0:12} since this run's checkout, and its pins no longer call for $id (they decide $now)"
  else
    echo current
  fi
}

# ── adopt (hosted runner) ─────────────────────────────────────────────

stage_file=
stage() {
  echo "::group::runtime-adopt stage $1"
  jq -n --arg stage "$1" '{status: "running", stage: $stage}' >"$stage_file"
}
stage_done() { echo "::endgroup::"; }

xtask() { cargo run --locked -q -p xtask -- "$@"; }

# ── live evidence ─────────────────────────────────────────────────────

# Hex (with or without 0x) on stdin to bytes on stdout; coreutils only.
unhex() { sed 's/^0x//' | tr -d '\n' | tr a-f A-F | basenc --base16 -d; }

# SHA-256 of the metadata inside a Metadata_metadata_at_version envelope
# (SCALE Option<Vec<u8>>: 0x01, a compact length, the bytes) read from a hex
# file; the bytes a metadata pin's v15_sha256/v16_sha256 digest.
envelope_sha256() {
  local file=$1 bin
  bin=$(mktemp)
  unhex <"$file" >"$bin"
  local head mode length skip size
  head=$(od -An -tu1 -N5 "$bin" | tr -s ' ' | sed 's/^ //')
  read -r -a byte <<<"$head"
  if [[ "${byte[0]:-}" != 1 ]]; then
    rm -f "$bin"
    echo none
    return
  fi
  mode=$((byte[1] & 3))
  case "$mode" in
    0) length=$((byte[1] >> 2)) skip=2 ;;
    1) length=$(((byte[1] | byte[2] << 8) >> 2)) skip=3 ;;
    2) length=$(((byte[1] | byte[2] << 8 | byte[3] << 16 | byte[4] << 24) >> 2)) skip=5 ;;
    *) rm -f "$bin"; echo invalid; return ;;
  esac
  size=$(stat -c %s "$bin")
  if ((size != skip + length)); then
    rm -f "$bin"
    echo invalid
    return
  fi
  tail -c +$((skip + 1)) "$bin" | sha256sum | cut -c1-64
  rm -f "$bin"
}

# What one endpoint serves at a block: genesis, runtime version, the :code
# storage hash it reports, the BLAKE2b-256 of the :code bytes computed here,
# the V15/V16 metadata digests, and whether the block is on the endpoint's
# own finalized chain (the pin took its finalized head from one endpoint).
# Errors are recorded, not raised.
endpoint_evidence() {
  local endpoint=$1 block=$2 dir
  dir=$(mktemp -d)
  local genesis version reported code_bytes v15 v16 error=""
  genesis=$(rpc "$endpoint" chain_getBlockHash '[0]' 2>/dev/null | jq -r .) || error=${error:-"genesis unavailable"}
  version=$(rpc "$endpoint" state_getRuntimeVersion "[\"$block\"]" 2>/dev/null) || error=${error:-"runtime version unavailable"}
  reported=$(rpc "$endpoint" state_getStorageHash "[\"0x3a636f6465\", \"$block\"]" 2>/dev/null | jq -r .) || error=${error:-":code hash unavailable"}
  if rpc "$endpoint" state_getStorage "[\"0x3a636f6465\", \"$block\"]" 2>/dev/null | jq -r . >"$dir/code.hex" && [[ -s "$dir/code.hex" ]]; then
    code_bytes=0x$(unhex <"$dir/code.hex" | b2sum -l 256 | cut -c1-64)
  else
    error=${error:-":code unavailable"}
  fi
  for v in 15 16; do
    if rpc "$endpoint" state_call "[\"Metadata_metadata_at_version\", \"0x$(printf '%02x' "$v")000000\", \"$block\"]" 2>/dev/null |
      jq -r . >"$dir/v$v.hex" && [[ -s "$dir/v$v.hex" ]]; then
      printf -v "v$v" '%s' "$(envelope_sha256 "$dir/v$v.hex")"
    else
      error=${error:-"metadata V$v unavailable"}
    fi
  done
  rm -rf "$dir"
  local number canonical=""
  if [[ -z "$error" ]]; then
    number=$(block_number "$(rpc "$endpoint" chain_getHeader "[\"$block\"]" 2>/dev/null | jq -r .number)")
    if [[ -n "$number" ]]; then
      canonical=$(on_finalized_chain "$endpoint" "$block" "$number")
    fi
    case "$canonical" in
      true | false) ;;
      unfinalized) error="has not finalized block $number" ;;
      *) error="finalized chain unavailable" ;;
    esac
  fi
  if [[ -n "$error" ]]; then
    jq -cn --arg endpoint "$endpoint" --arg error "$error" '{endpoint: $endpoint, error: $error}'
    return
  fi
  jq -cn --arg endpoint "$endpoint" --arg genesis "$genesis" --argjson version "$version" \
    --arg reported "$reported" --arg code "$code_bytes" --arg v15 "$v15" --arg v16 "$v16" \
    --argjson number "$number" --argjson canonical "$canonical" \
    '{endpoint: $endpoint, genesis: $genesis, spec: $version.specVersion, spec_name: $version.specName,
      transaction_version: $version.transactionVersion, code_reported: $reported, code: $code,
      v15_sha256: $v15, v16_sha256: $v16, number: $number, canonical: $canonical}'
}

# The rule live evidence must meet, as a pure function of live.json:
# {expected: {genesis, spec, code, v15_sha256, v16_sha256}, endpoints: [...],
#  release: {status: matches|differs|unavailable, ...}}. Every answering
# endpoint must hold the pinned block on its own finalized chain and match
# every expected field (its reported :code hash and the hash of the bytes
# alike), at least two must answer, and a published release wasm must hash
# to the same :code.
verdict() {
  local live=${1:?verdict <live.json>}
  jq -c '
    .expected as $x
    | (.endpoints | map(select((.error // null) == null))) as $answered
    | def mismatches: [
        (if .genesis != $x.genesis then "genesis \(.genesis)" else empty end),
        (if .spec != $x.spec then "spec \(.spec)" else empty end),
        (if .spec_name != "node-subtensor" then "spec_name \(.spec_name)" else empty end),
        (if .canonical != true then "finalized another block at height \(.number), not the pinned block" else empty end),
        (if .code_reported != $x.code then "reported :code hash \(.code_reported)" else empty end),
        (if .code != $x.code then "BLAKE2b-256 of :code \(.code)" else empty end),
        (if .v15_sha256 != $x.v15_sha256 then "V15 metadata sha256 \(.v15_sha256)" else empty end),
        (if .v16_sha256 != $x.v16_sha256 then "V16 metadata sha256 \(.v16_sha256)" else empty end)];
      ($answered | map({endpoint, mismatches: mismatches}) | map(select(.mismatches != []))) as $bad
    | ((.release.status // "unavailable") == "differs") as $release_bad
    | {ok: (($bad | length) == 0 and ($answered | length) >= 2 and ($release_bad | not)),
       agreeing: (($answered | length) - ($bad | length)),
       disagreeing: $bad,
       unanswered: (.endpoints | map(select((.error // null) != null) | {endpoint, error})),
       release: (.release.status // "unavailable"),
       reason: (if ($bad | length) > 0 then
                  "live evidence disagrees with the pin: " + ($bad | map("\(.endpoint): \(.mismatches | join(", "))") | join("; "))
                elif $release_bad then
                  "the release wasm hashes to \(.release.code), not the finalized :code \($x.code)"
                elif ($answered | length) < 2 then
                  "only \($answered | length) endpoint(s) served the pinned block; two are required"
                else "consistent" end)}' "$live"
}

# Check the pinned Finney runtime against the live chain at the pinned block,
# from every evidence endpoint, and against the release wasm when upstream
# published one. Writes live.json and verdict.json; fails unless verdict ok.
live_evidence() {
  local decision=$1 evidence=$2
  local block spec code v15 v16 genesis source endpoints rows='[]' endpoint release
  block=$(toml_get upstream.toml networks.finney.metadata finalized_head)
  spec=$(toml_get upstream.toml networks.finney.metadata spec)
  code=$(toml_get upstream.toml networks.finney.metadata code_hash)
  v15=$(toml_get upstream.toml networks.finney.metadata v15_sha256)
  v16=$(toml_get upstream.toml networks.finney.metadata v16_sha256)
  genesis=$(toml_get upstream.toml networks.finney genesis)
  source=$(jq -r '.source // empty' "$decision")
  [[ "$spec" == "$(jq -r .spec "$decision")" ]] || die "the pin recorded spec $spec, not the decided $(jq -r .spec "$decision")"
  [[ "$code" == "$(jq -r .code "$decision")" ]] || die "the pin recorded :code $code, not the observed $(jq -r .code "$decision")"
  endpoints=${FINNEY_EVIDENCE_ENDPOINTS:-$(toml_get upstream.toml networks.finney genesis_endpoint) $finney_witnesses}
  for endpoint in $endpoints; do
    rows=$(jq -c --argjson row "$(endpoint_evidence "$endpoint" "$block")" '. + [$row]' <<<"$rows")
  done
  release='{"status": "unavailable", "detail": "no published release for this pin source"}'
  if [[ "$source" == v* ]]; then
    local dir
    dir=$(mktemp -d)
    if gh release download "$source" --repo "$upstream_repo" --pattern subtensor.wasm --dir "$dir" >/dev/null 2>&1; then
      local hash
      hash=0x$(b2sum -l 256 "$dir/subtensor.wasm" | cut -c1-64)
      release=$(jq -cn --arg tag "$source" --arg code "$hash" --arg expected "$code" \
        '{status: (if $code == $expected then "matches" else "differs" end), tag: $tag, code: $code}')
    else
      release=$(jq -cn --arg tag "$source" '{status: "unavailable", tag: $tag, detail: "release has no subtensor.wasm asset"}')
    fi
    rm -rf "$dir"
  fi
  jq -n --arg block "$block" --arg genesis "$genesis" --argjson spec "$spec" --arg code "$code" \
    --arg v15 "$v15" --arg v16 "$v16" --argjson endpoints "$rows" --argjson release "$release" \
    '{block: $block, expected: {genesis: $genesis, spec: $spec, code: $code, v15_sha256: $v15, v16_sha256: $v16},
      endpoints: $endpoints, release: $release}' >"$evidence/live.json"
  verdict "$evidence/live.json" | tee "$evidence/verdict.json"
  [[ "$(jq -r .ok "$evidence/verdict.json")" == true ]] || die "$(jq -r .reason "$evidence/verdict.json")"
}

# A read-only call through the built CLI against live Finney: the compiled
# surface, the compatibility guard and the transport on the real chain.
# A constant, a storage value and a runtime-API read with decoding.
live_reads=("existential-deposit" "timestamp" "subnet-hyperparameters --netuid 1")
live_read() {
  local evidence=$1 rtao=$CARGO_TARGET_DIR/debug/rtao read name
  [[ -x "$rtao" ]] || die "the workspace build produced no rtao binary"
  for read in "${live_reads[@]}"; do
    name=${read%% *}
    # shellcheck disable=SC2086 # the read carries its own arguments
    timeout 120s "$rtao" query $read --network finney --json >"$evidence/live-$name.json" ||
      die "rtao query $read --network finney failed"
    jq -e . "$evidence/live-$name.json" >/dev/null || die "rtao query $read returned no JSON"
  done
}

# Newest index row's metadata paths for a spec, relative to the index.
index_blob() {
  awk -v want="$2" -v version="$3" '
    /^\[\[runtimes\]\]/ { spec = "" ; next }
    $1 == "spec" && $2 == "=" { spec = $3 }
    spec == want && $1 == version && $2 == "=" { match($0, /path = "[^"]+"/); print substr($0, RSTART + 8, RLENGTH - 9) }
  ' "$1" | tail -1
}

adopt() {
  local decision=${1:?adopt <decision.json> <candidate> <evidence-dir>}
  local candidate=${2:?adopt <decision.json> <candidate> <evidence-dir>}
  local evidence=${3:?adopt <decision.json> <candidate> <evidence-dir>}
  : "${RTAO_TEMP_ROOT:?task-scoped scratch}"
  : "${CARGO_TARGET_DIR:?shared build directory}"
  if [[ "${RTAO_ADOPT_LOCAL:-}" != 1 ]]; then
    [[ "${GITHUB_ACTIONS:-}" == true && "${RUNNER_ENVIRONMENT:-}" == github-hosted ]] ||
      die "adopt runs on a GitHub-hosted runner (RTAO_ADOPT_LOCAL=1 rehearses it in a disposable candidate)"
  fi
  mkdir -p "$evidence"
  stage_file=$evidence/stage.json
  local action spec source
  action=$(jq -r .action "$decision")
  spec=$(jq -r .spec "$decision")
  source=$(jq -r '.source // empty' "$decision")
  [[ "$spec" =~ ^[0-9]+$ ]] || die "decision names no spec"
  # The candidate is a detached worktree of this checkout inside the scratch
  # root, as pin candidate preparation requires; main is never written here.
  git -C "$root" worktree add --detach "$candidate" HEAD >/dev/null
  cd "$candidate"
  case "$action" in
    adopt-finney)
      [[ -n "$source" ]] || die "adopt-finney without a pin source"
      local old_spec
      old_spec=$(toml_get upstream.toml networks.finney.metadata spec)
      stage provenance
      RTAO_PIN_CANDIDATE=1 xtask pin --parity "$source" --prepare-candidate
      stage_done
      stage evidence
      live_evidence "$decision" "$evidence"
      stage_done
      stage import
      # Pin preparation removed the prior copies; restore them with its binary
      # before recompiling consumers of those files (ci/pin-pipeline.sh).
      "$CARGO_TARGET_DIR/debug/xtask" import-upstream
      stage_done
      stage core
      # pin holds the core when its inputs moved (D-127) and records the
      # bump as due; the core itself must therefore never move here.
      if ! cmp -s "$root/Cargo.toml" Cargo.toml ||
        [[ "$(toml_get "$root/upstream.toml" core commit)" != "$(toml_get upstream.toml core commit)" ]]; then
        die "the core input moved with spec $spec: a core bump (core-patch rebase, lock update, core-surface) needs a maintainer"
      fi
      stage_done
      stage extract
      xtask extract
      stage_done
      stage regen
      # Operations new to the catalog are pending ports, not a regeneration
      # failure (D-128); the parity report below lists what is due.
      xtask parity pending --base "$root"
      xtask regen
      stage_done
      stage compat
      xtask compat --measure-build-weight --build-target aarch64-apple-darwin
      xtask compat
      stage_done
      stage parity
      # Adoption mode: reconcile what evidence proves, re-derive generated,
      # extracted and vendored sources (checked below), list due hand ports.
      xtask parity report --base "$root" --out "$evidence/porting-report.json" --adopt
      xtask parity matrix >/dev/null
      xtask parity check >"$evidence/parity.json"
      # Strict parity names what remains due; it is recorded, not a gate.
      xtask parity check --strict >/dev/null 2>"$evidence/parity-strict.txt" || true
      if [[ -f parity/port_due.toml ]]; then
        cp parity/port_due.toml "$evidence/port_due.toml"
      fi
      : >"$evidence/port_due.checked"
      stage_done
      stage drift
      local index=vendor/metadata/finney/index.toml
      xtask drift diff \
        "vendor/metadata/finney/$(index_blob "$index" "$old_spec" v15)" \
        "vendor/metadata/finney/$(index_blob "$index" "$spec" v15)" \
        --old-v16 "vendor/metadata/finney/$(index_blob "$index" "$old_spec" v16)" \
        --new-v16 "vendor/metadata/finney/$(index_blob "$index" "$spec" v16)" \
        --out "$evidence/drift" >"$evidence/drift.json"
      local class envelope
      class=$(jq -r .classification "$evidence/drift.json")
      # A breaking call, storage, event or model shape is what regeneration
      # absorbs (checked below by regen --check, the build and the tests) and
      # is listed in the changelog. A breaking extrinsic envelope (versions,
      # signed extensions) or runtime identity (spec name, transaction
      # version) needs a signing port first and still stops here (D-128).
      envelope=$(jq -r '[.changes[] | select(.class == "breaking" and ((.surface | startswith("extrinsic/")) or .surface == "System.Version")) | .surface] | join(", ")' "$evidence/drift.json")
      [[ -z "$envelope" ]] || die "metadata drift $old_spec -> $spec breaks the signing envelope ($envelope); see the drift report"
      stage_done
      stage probe
      xtask probe pins
      stage_done
      stage checks
      xtask pin --check --source-only
      xtask import-upstream --verify-only
      xtask extract --check
      xtask regen --check
      xtask compat --check
      xtask probe pins --check
      xtask drift --network finney --check
      cargo fmt --all --check
      stage_done
      stage build
      cargo build --locked --workspace --all-targets
      stage_done
      stage tests
      cargo test --locked -p rtao-runtime -p rtao-reads -p rtao-intents -p rtao-client
      stage_done
      stage live
      live_read "$evidence"
      stage_done
      stage record
      record_adoption "$decision" "$old_spec" "$class" "$evidence"
      stage_done
      ;;
    core-bump)
      # The explicit core bump of D-127, run through the same isolated
      # candidate as an adoption (D-136). The parity pin, the runtime and
      # its metadata stay as they are; only the core moves to the parity tag.
      [[ -n "$source" ]] || die "core-bump without a pin source"
      local core_tag fingerprint
      core_tag=$(jq -r .core.to "$decision")
      fingerprint=$(jq -r .core.fingerprint "$decision")
      [[ "$(toml_get upstream.toml parity core_fingerprint || true)" == "$fingerprint" ]] ||
        die "the parity pin no longer records the decided core inputs $fingerprint"
      stage provenance
      # pin rebases the reviewed core patch onto the parity tag and adopts
      # the fork branch rtao-core-patch/<tag> when it carries exactly that
      # tree, rewrites every downstream core reference and moves [core]. With
      # no such branch it retains a publication candidate and stops: the
      # fork is published with existing authorized access (hosted CI never
      # pushes it) and the dispatch is repeated.
      if ! RTAO_PIN_CANDIDATE=1 RTAO_PIN_CORE_BUMP=1 xtask pin --parity "$source" --prepare-candidate; then
        if [[ -d "$RTAO_TEMP_ROOT/core-patch/candidate" ]]; then
          cp -R "$RTAO_TEMP_ROOT/core-patch/candidate" "$evidence/core-patch-candidate"
        fi
        die "the core bump's pin failed (see the log; a core-patch publication candidate, when one was prepared, is in the evidence under core-patch-candidate)"
      fi
      stage_done
      stage core
      [[ "$(toml_get upstream.toml core tag)" == "$core_tag" ]] ||
        die "pin left the core at $(toml_get upstream.toml core tag), not $core_tag"
      [[ "$(toml_get upstream.toml core fingerprint)" == "$fingerprint" ]] ||
        die "the moved core's fingerprint is not the parity pin's recorded core inputs"
      ! toml_get upstream.toml parity core_fingerprint >/dev/null ||
        die "the parity pin still records held core inputs after the bump"
      local key
      for key in tag commit spec_version; do
        [[ "$(toml_get upstream.toml parity "$key")" == "$(toml_get "$root/upstream.toml" parity "$key")" ]] ||
          die "the core bump moved the parity pin's $key"
      done
      for key in spec code_hash v15_sha256 v16_sha256; do
        [[ "$(toml_get upstream.toml networks.finney.metadata "$key")" == "$(toml_get "$root/upstream.toml" networks.finney.metadata "$key")" ]] ||
          die "the core bump changed the pinned Finney runtime's $key"
      done
      stage_done
      stage evidence
      live_evidence "$decision" "$evidence"
      stage_done
      stage import
      "$CARGO_TARGET_DIR/debug/xtask" import-upstream
      stage_done
      stage lock
      # The moved core sources resolve again; nothing else is updated.
      cargo update --workspace
      stage_done
      stage core-surface
      xtask core-surface
      stage_done
      stage extract
      xtask extract
      stage_done
      stage regen
      xtask parity pending --base "$root"
      xtask regen
      stage_done
      stage compat
      xtask compat --measure-build-weight --build-target aarch64-apple-darwin
      xtask compat
      stage_done
      stage parity
      xtask parity report --base "$root" --out "$evidence/porting-report.json" --adopt
      xtask parity matrix >/dev/null
      xtask parity check >"$evidence/parity.json"
      xtask parity check --strict >/dev/null 2>"$evidence/parity-strict.txt" || true
      if [[ -f parity/port_due.toml ]]; then
        cp parity/port_due.toml "$evidence/port_due.toml"
      fi
      : >"$evidence/port_due.checked"
      stage_done
      stage checks
      # Full pin verification on the hosted runner: the patched and
      # unpatched core API build probes, the patch's review identity and its
      # Ed25519 erasure check, the manifests and the lock.
      xtask pin --check --prepare-cache
      xtask import-upstream --verify-only
      xtask core-surface --check
      xtask extract --check
      xtask regen --check
      xtask compat --check
      xtask probe pins --check
      xtask drift --network finney --check
      cargo fmt --all --check
      stage_done
      stage build
      cargo build --locked --workspace --all-targets
      stage_done
      stage tests
      cargo test --locked -p rtao-core -p rtao-runtime -p rtao-reads -p rtao-intents -p rtao-client -p rtao-signer -p rtao-wallet
      stage_done
      stage live
      live_read "$evidence"
      stage_done
      stage record
      record_core_bump "$decision" "$evidence"
      stage_done
      ;;
    record-testnet)
      stage drift
      xtask drift --network testnet --apply --out "$evidence/drift"
      stage_done
      stage probe
      xtask probe pins
      stage_done
      stage checks
      xtask drift --network testnet --check
      xtask probe pins --check
      xtask compat --measure-build-weight --build-target aarch64-apple-darwin
      xtask compat
      xtask compat --check
      stage_done
      stage record
      record_testnet "$decision" "$evidence"
      stage_done
      ;;
    *) die "nothing to adopt for action $action" ;;
  esac
  jq -n '{status: "validated", stage: "commit"}' >"$stage_file"
  git status --short >"$evidence/changed.txt"
  [[ -s "$evidence/changed.txt" ]] || die "the $action pipeline changed nothing"
}

# Insert text after the "## Unreleased" heading of CHANGELOG.md.
changelog_entry() {
  local text=$1 file=CHANGELOG.md
  awk -v entry="$text" '
    !done && /^## Unreleased[ \t]*$/ { print; print ""; print entry; done = 1; next }
    { print }
    END { if (!done) exit 1 }
  ' "$file" >"$file.new" || die "CHANGELOG.md has no Unreleased section"
  mv "$file.new" "$file"
}

# Insert an entry before "## Recording the next change" of the upstream log.
upstream_log_entry() {
  local text=$1 file=book/src/upstream-log.md
  awk -v entry="$text" '
    !done && /^## Recording the next change/ { print entry; print ""; done = 1 }
    { print }
    END { if (!done) exit 1 }
  ' "$file" >"$file.new" || die "upstream-log.md has no recording section"
  mv "$file.new" "$file"
}

record_adoption() {
  local decision=$1 old_spec=$2 class=$3 evidence=$4
  local spec code number finalized source tag commit run blessed rederived due changed today agreeing release due_text core_text breaking_text
  spec=$(jq -r .spec "$decision")
  code=$(jq -r .code "$decision")
  number=$(jq -r .observed.finney.number "$decision")
  finalized=$(jq -r .observed.finney.finalized "$decision")
  source=$(jq -r .source "$decision")
  tag=$(toml_get upstream.toml parity tag)
  commit=$(toml_get upstream.toml parity commit)
  run=${GITHUB_SERVER_URL:-https://github.com}/${GITHUB_REPOSITORY:-local}/actions/runs/${GITHUB_RUN_ID:-local}
  changed=$(jq -r .changed "$evidence/porting-report.json")
  blessed=$(jq -r '.auto_blessed | length' "$evidence/porting-report.json")
  rederived=$(jq -r '.rederived | length' "$evidence/porting-report.json")
  due=$(jq -r '.port_due | length' "$evidence/porting-report.json")
  agreeing=$(jq -r .agreeing "$evidence/verdict.json")
  release=$(jq -r .release "$evidence/verdict.json")
  today=$(date -u +%Y-%m-%d)
  if ((due > 0)); then
    due_text="$due hand port(s) remain due and are listed in \`parity/port_due.toml\` ($(jq -r '.port_due | map("`\(.path)`") | join(", ")' "$evidence/porting-report.json")); strict parity names them until they are ported and blessed."
  else
    due_text="No hand port is due."
  fi
  breaking_text=$(jq -r '[.changes[] | select(.class == "breaking") | "`\(.surface)`"] | if length == 0 then "" else " Breaking shapes, regenerated: " + join(", ") + "." end' "$evidence/drift.json")
  if [[ -n "$(toml_get upstream.toml parity core_fingerprint || true)" ]]; then
    core_text="The core's inputs differ at $tag; the core stays at $(toml_get upstream.toml core tag) and its bump is due (\`[parity] core_fingerprint\`, D-127)."
  else
    core_text="Core and its patch are unchanged."
  fi
  changelog_entry "### Finney runtime $spec adopted automatically

- The parity pin moves from spec $old_spec to $tag (\`$commit\`, pinned as \`$source\`), the runtime Finney finalized at block $number with \`:code\` \`$code\`.
- Live evidence: $agreeing public endpoints agree at the pinned block on genesis, spec, the \`:code\` hash (and the BLAKE2b-256 of its bytes) and the V15/V16 metadata digests; release wasm: $release.
- Metadata drift $old_spec -> $spec: \`$class\`.$breaking_text Code generation, extracted tables and catalogs are regenerated from the pinned sources; watched sources changed: $changed, of which $blessed were reconciled automatically and $rederived re-derived and checked. $due_text
- $core_text
- Checked by the hosted runtime-adopt workflow ([run]($run)): pin source identity, live evidence, imports, extraction, regeneration, compatibility, probe pins, drift, parity, formatting, a workspace build, the runtime, reads, intents and client tests, and live reads with the built CLI. No oracle comparison, localnet e2e or image execution; the validated range is unchanged."
  upstream_log_entry "## $today: Finney runtime $spec adopted automatically

The hosted runtime-adopt workflow observed Finney finalize spec $spec at block
$number (\`$finalized\`), \`:code\` \`$code\`, confirmed it from $agreeing public
endpoints (release wasm: $release), and advanced parity from spec $old_spec to
\`$tag\` (\`$commit\`, pin source \`$source\`). Metadata drift is \`$class\`. The
porting report lists $changed changed watched sources: $blessed auto-blessed,
$rederived re-derived and checked. $due_text $core_text The validated range
is unchanged. Workflow run: <$run>."
}

record_core_bump() {
  local decision=$1 evidence=$2 from to fingerprint tag rev run today due due_text
  from=$(jq -r .core.from "$decision")
  to=$(toml_get upstream.toml core tag)
  fingerprint=$(toml_get upstream.toml core fingerprint)
  tag=$(toml_get upstream.toml parity tag)
  rev=$(toml_get upstream.toml core.patch rev || echo none)
  run=${GITHUB_SERVER_URL:-https://github.com}/${GITHUB_REPOSITORY:-local}/actions/runs/${GITHUB_RUN_ID:-local}
  today=$(date -u +%Y-%m-%d)
  due=$(jq -r '.port_due | length' "$evidence/porting-report.json")
  due_text="$due hand port(s) remain listed in \`parity/port_due.toml\`."
  changelog_entry "### Core bumped from $from to $to (D-127, D-136)

- \`bittensor-core\` and its companions move from $from to $to, the parity pin's core (\`$fingerprint\`); \`[parity] core_fingerprint\` is gone. The reviewed core patch is carried onto $to (fork revision \`$rev\`), and every downstream \`[patch]\` line names $to: applications that copy rtao's patch set must update theirs.
- The parity pin ($tag) and the runtime are unchanged. $due_text
- Checked by the hosted runtime-adopt workflow ([run]($run)): pin provenance and live evidence for the pinned runtime, the core patch's review identity, Ed25519 erasure check and patched/unpatched API probes, the lock, imports, the core surface, extraction, regeneration, compatibility, parity, formatting, a workspace build, the core, runtime, reads, intents, client, signer and wallet tests, and live reads with the built CLI. No oracle comparison, localnet e2e or image execution."
  upstream_log_entry "## $today: core bumped from $from to $to

A dispatched runtime-adopt run moved the held core from $from to $to, the core
of the parity pin \`$tag\` (D-127, D-136), carrying the reviewed core patch to
fork revision \`$rev\`. The parity pin and the runtime are unchanged.
Workflow run: <$run>."
}

record_testnet() {
  local decision=$1 evidence=$2 spec code number run class source_text
  spec=$(jq -r .spec "$decision")
  code=$(jq -r .code "$decision")
  number=$(jq -r .observed.testnet.number "$decision")
  class=$(jq -r '.classification // "recorded"' "$evidence/drift/report.json" 2>/dev/null || echo recorded)
  run=${GITHUB_SERVER_URL:-https://github.com}/${GITHUB_REPOSITORY:-local}/actions/runs/${GITHUB_RUN_ID:-local}
  # drift --apply proves a source for a runtime other than the parity one
  # only from upstream's release wasm (D-162); the parity runtime carries the
  # parity tag and commit (D-126).
  source_text=$(jq -r '
    if .proven == true then
      " Source: upstream commit `\(.commit)`, whose \(.release)\(if .prerelease then " prerelease" else "" end) wasm is this `:code`."
    elif .proven == false then " No upstream source is recorded: \(.reason)."
    else "" end' "$evidence/drift/source.json" 2>/dev/null || true)
  changelog_entry "### Testnet runtime $spec recorded automatically

- Testnet finalized spec $spec (\`:code\` \`$code\`) at block $number; its V15/V16 metadata is indexed under \`vendor/metadata/testnet/\` (drift against the previous testnet row: \`$class\`) and the probe pins are regenerated ([run]($run)).$source_text The parity pin is unchanged."
}

# ── commit and push ───────────────────────────────────────────────────

bot() {
  git -c user.name="${RTAO_ADOPT_NAME:-github-actions[bot]}" \
    -c user.email="${RTAO_ADOPT_EMAIL:-41898282+github-actions[bot]@users.noreply.github.com}" "$@"
}

# An adoption writes pins, vendored metadata and sources, generated code,
# parity and compatibility records and the changelog; it never changes the
# tooling that validated it (scripts, xtask, workflows, manifests, the lock,
# the toolchain, Cargo configuration or the core patch). A commit that does
# was not produced by the pipeline as checked: refuse it, here and again in
# the push job, which trusts nothing the adopt job built.
tooling_paths='^(\.github/|ci/|xtask/|\.cargo/|\.config/|patches/|rust-toolchain\.toml$|deny\.toml$|clippy\.toml$|AGENTS\.md$)|(^|/)(Cargo\.toml|Cargo\.lock|build\.rs)$'
# Vendored upstream sources (`import-upstream`, each a pinned artifact whose
# bytes the checks stage verifies with `import-upstream --verify-only`) are
# not workspace members or tooling, whatever their file name: v490 changed
# the vendored `sdk/bittensor-core/Cargo.toml` (D-166).
vendored_sources='^vendor/upstream/src/'
# A core bump (D-136) must move the core's references in manifests and
# locks; every changed manifest line must be one of those references.
core_bump_paths='(^|/)Cargo\.(toml|lock)$'
core_reference='^[-+]([A-Za-z0-9_-]+ = \{ git = "https://github\.com/(RaoFoundation|sam0x17)/subtensor\.git", (tag = "v[0-9]+"|rev = "[0-9a-f]{40}")(, [^{}]*)? \}|\[patch\."https://github\.com/(RaoFoundation|sam0x17)/subtensor\.git"\]|)$'
# A core bump also moves the generated republished packages (D-183), which
# the checks stage regenerates from the recorded revisions and requires equal
# (pin --check), and the root manifest's exact requirements on them; each such
# line may change only its version (D-188).
generated_packages='^upstream/(macros|fips204|w3f-bls|hashed|tle|core)/'
# Of their tooling files only the manifests may change, and no added manifest
# line may bring in a git source, a build script, links, another registry, a
# [patch]/[replace]/[workspace] table or a path outside the packages
# themselves: a build script or a new source in a republished package needs a
# reviewed commit, not the bump's guard.
generated_manifest='^upstream/(macros|fips204|w3f-bls|hashed|tle|core)/Cargo\.toml$'
generated_manifest_refused='(^|[{,] *)(git|build|links|registry|registry-index|workspace) *=|^ *\[+ *(patch|replace|workspace)'
generated_manifest_path='^path *= *"(src/lib\.rs|\.\./(macros|fips204|w3f-bls|hashed|tle|core))"$'
republished_dependency='^[-+](bittensor-core|w3f-bls|tle) = \{ package = "rtao-upstream-(core|w3f-bls|tle)", path = "upstream/(core|w3f-bls|tle)", version = "=0\.[0-9]+\.[0-9]+", default-features = false(, features = \["std"\])? \}$'
# The record of published packages (D-188) adds one checksum line to
# [publish.<key>] tables of upstream.toml and of its two crate copies, which
# stay byte-identical; nothing else.
publish_record_files='^(upstream\.toml|crates/rtao-core/upstream\.toml|crates/rtao-testkit/upstream\.toml)$'
publish_checksum='^\+checksum = "[0-9a-f]{64}"$'
# upstream.toml without the checksum lines of its [publish.<key>] tables.
without_publish_checksums() {
  awk '/^\[/ { publish = ($0 ~ /^\[publish\.[A-Za-z0-9_-]+\]$/) } !(publish && $1 == "checksum") { print }'
}
check_publish_record() {
  local changed stray path blob
  changed=$(git diff --name-only "$1" "$2")
  stray=$(grep -Ev "$publish_record_files" <<<"$changed" || true)
  [[ -z "$stray" ]] || die "the [publish] record changes files other than upstream.toml and its crate copies: $(tr '\n' ' ' <<<"$stray")"
  grep -qx 'upstream.toml' <<<"$changed" || die "the [publish] record does not change upstream.toml"
  blob=$(git rev-parse -q --verify "$2:upstream.toml") || die "the [publish] record removes upstream.toml"
  for path in crates/rtao-core/upstream.toml crates/rtao-testkit/upstream.toml; do
    [[ "$(git rev-parse -q --verify "$2:$path" || true)" == "$blob" ]] || die "the [publish] record leaves $path different from upstream.toml"
  done
  stray=$(git diff --no-color --no-ext-diff -U0 "$1" "$2" -- upstream.toml |
    grep -E '^[-+]' | grep -Ev '^(\+\+\+|---) ' | grep -Ev "$publish_checksum" || true)
  [[ -z "$stray" ]] || die "the [publish] record changes upstream.toml beyond added checksum lines: $stray"
  cmp -s <(git show "$1:upstream.toml" | without_publish_checksums) <(git show "$2:upstream.toml" | without_publish_checksums) ||
    die "the [publish] record adds checksum lines outside the [publish.<key>] tables"
}
check_paths() {
  local touched manifest stray added
  if [[ "${3:-}" == publish-forks ]]; then
    check_publish_record "$1" "$2"
    return
  fi
  touched=$(git diff --name-only "$1" "$2" | grep -Ev "$vendored_sources" | grep -E "$tooling_paths" || true)
  if [[ "${3:-}" == core-bump ]]; then
    for manifest in $(grep -E "$generated_packages" <<<"$touched" || true); do
      [[ "$manifest" =~ $generated_manifest ]] ||
        die "the core bump changes $manifest; of the republished packages' tooling only their manifests may move"
      added=$(git diff --no-color --no-ext-diff -U0 "$1" "$2" -- "$manifest" | grep -E '^\+' | grep -Ev '^\+\+\+ ' | cut -c2- || true)
      stray=$(grep -E "$generated_manifest_refused" <<<"$added" || true)
      stray+=$(grep -oE 'path *= *"[^"]*"' <<<"$added" | grep -Ev "$generated_manifest_path" || true)
      [[ -z "$stray" ]] || die "the core bump's $manifest adds what the guard does not admit: $stray"
    done
    touched=$(grep -Ev "$generated_packages" <<<"$touched" || true)
    for manifest in $(grep -E '(^|/)Cargo\.toml$' <<<"$touched" || true); do
      stray=$(git diff --no-color --no-ext-diff -U0 "$1" "$2" -- "$manifest" |
        grep -E '^[-+]' | grep -Ev '^(\+\+\+|---) ' | grep -Ev "$core_reference" || true)
      if [[ "$manifest" == Cargo.toml && -n "$stray" ]] && ! grep -Evq "$republished_dependency" <<<"$stray" &&
        [[ "$(grep '^-' <<<"$stray" | cut -c2- | sed 's/version = "=[^"]*"/version/' | sort)" == \
          "$(grep '^+' <<<"$stray" | cut -c2- | sed 's/version = "=[^"]*"/version/' | sort)" ]]; then
        stray=""
      fi
      [[ -z "$stray" ]] || die "the core bump changes $manifest beyond the core references: $stray"
    done
    touched=$(grep -Ev "$core_bump_paths" <<<"$touched" || true)
  fi
  [[ -z "$touched" ]] || die "the adoption commit changes its own tooling, which the pipeline never writes: $(tr '\n' ' ' <<<"$touched")"
}

# Commit the validated candidate and write it as a git bundle. Runs in the
# adopt job, whose token cannot write: nothing here pushes.
commit() {
  local decision=${1:?commit <decision.json> <candidate> <bundle>}
  local candidate=${2:?commit <decision.json> <candidate> <bundle>}
  local bundle=${3:?commit <decision.json> <candidate> <bundle>}
  local action spec title body
  action=$(jq -r .action "$decision")
  spec=$(jq -r .spec "$decision")
  cd "$candidate"
  case "$action" in
    adopt-finney)
      title="Adopt Finney runtime $spec automatically ($(toml_get upstream.toml parity tag))"
      body="Finney finalized spec $spec; the hosted runtime-adopt workflow pinned it,
regenerated and checked every stage (see CHANGELOG.md and
book/src/upstream-log.md)."
      ;;
    record-testnet)
      title="Record testnet runtime $spec automatically"
      body="Testnet finalized spec $spec; its metadata is indexed with
drift --apply and the probe pins regenerated."
      ;;
    core-bump)
      title="Bump the core from $(jq -r .core.from "$decision") to $(toml_get upstream.toml core tag) (D-127)"
      body="A dispatched runtime-adopt run moved the held core to the parity pin's
core, carried the reviewed core patch and checked every stage (see
CHANGELOG.md and book/src/upstream-log.md)."
      ;;
    *) die "nothing to commit for action $action" ;;
  esac
  git add -A
  bot commit -q -m "$title [skip ci]" -m "$body"
  check_paths HEAD~1 HEAD "$action"
  git bundle create -q "$bundle" HEAD~1..HEAD
  git rev-parse HEAD
}

# Push a bundle's one commit to main, with a snapshot tag for a Finney
# adoption. Runs in a job that builds nothing. Exit 0 and print
# "superseded" when main moved since the candidate was cut: the next
# scheduled run starts again from the new main; that is not a failure.
push() {
  local decision=${1:?push <decision.json> <bundle>}
  local bundle=${2:?push <decision.json> <bundle>}
  local action spec network commit parent today tag n
  action=$(jq -r .action "$decision")
  spec=$(jq -r .spec "$decision")
  network=$(jq -r .network "$decision")
  case "$action" in adopt-finney | record-testnet | core-bump | publish-forks) ;; *) die "nothing to push for action $action" ;; esac
  git bundle verify -q "$bundle" >/dev/null 2>&1 || die "the bundle does not apply to this checkout"
  git fetch -q "$bundle" HEAD
  commit=$(git rev-parse FETCH_HEAD)
  [[ "$(git rev-list --count --no-walk=unsorted "$commit^@")" == 1 ]] || die "the bundle's commit is not a single-parent commit"
  parent=$(git rev-parse "$commit^")
  check_paths "$parent" "$commit" "$action"
  git fetch -q origin main
  if [[ "$(git rev-parse origin/main)" != "$parent" ]]; then
    echo "superseded: main moved from $parent during the run; the next scheduled run starts again from the new main"
    return 0
  fi
  if [[ "$action" == adopt-finney || "$action" == core-bump ]]; then
    today=$(date -u +%Y-%m-%d)
    git fetch -q --tags origin
    tag=snapshot-$today
    n=1
    while git rev-parse -q --verify "refs/tags/$tag" >/dev/null; do
      n=$((n + 1))
      tag=snapshot-$today.$n
    done
    local what
    if [[ "$action" == core-bump ]]; then
      what="core bumped to $(git show "$commit:upstream.toml" | toml_get /dev/stdin core tag)"
    else
      what="Finney runtime $spec adopted automatically ($(git show "$commit:upstream.toml" | toml_get /dev/stdin parity tag))"
    fi
    bot tag -a "$tag" -m "Snapshot of main: $what. Not a versioned release; see CHANGELOG.md (Unreleased)." "$commit"
    if [[ "${RTAO_ADOPT_DRY_RUN:-}" == 1 ]]; then
      echo "dry run: would push $commit to main and tag $tag"
      return 0
    fi
    # Atomic: main and the tag land together or not at all.
    git push -q --atomic origin "$commit:refs/heads/main" "refs/tags/$tag"
    echo "pushed $commit and $tag"
  else
    if [[ "${RTAO_ADOPT_DRY_RUN:-}" == 1 ]]; then
      echo "dry run: would push $commit to main"
      return 0
    fi
    git push -q origin "$commit:refs/heads/main"
    if [[ "$action" == publish-forks ]]; then
      echo "pushed $commit (the republished packages' checksums)"
    else
      echo "pushed $commit ($network runtime $spec)"
    fi
  fi
}

# ── republished core packages (crates.io, D-188) ──────────────────────

# The republished packages in dependency order (`cargo xtask forks order`,
# which forks-package checks this list against).
fork_order=(macros fips204 w3f-bls hashed tle core)
forks_marker='<!-- rtao-forks-publish -->'
release_marker='<!-- rtao-release-proposal -->'
crates_index=${RTAO_CRATES_INDEX:-https://index.crates.io}

# The [publish.<key>] entries of an upstream.toml in dependency order, one
# per line: key, package, version and checksum ("-" when none is recorded).
publish_entries() {
  local pins=$1 key
  for key in "${fork_order[@]}"; do
    awk -v key="$key" '
      /^\[/ { inside = ($0 == "[publish." key "]"); next }
      inside && $2 == "=" && ($1 == "package" || $1 == "version" || $1 == "checksum") {
        value = $0; sub(/^[^=]*=[ \t]*/, "", value); gsub(/^"|"$/, "", value); field[$1] = value
      }
      END {
        if (field["package"] == "" || field["version"] == "") exit 1
        printf "%s\t%s\t%s\t%s\n", key, field["package"], field["version"], (field["checksum"] == "" ? "-" : field["checksum"])
      }
    ' "$pins" || die "$pins has no complete [publish.$key]"
  done
}

# The republished packages main records without a checksum, in dependency
# order, as [{key, package, version}]: new versions of a core bump or of a new
# core adaptation revision (forks refresh drops the checksum with the version),
# or versions an earlier run published without recording them.
forks_pending() {
  local pins=${1:?forks-pending <upstream.toml>} key package version checksum rows='[]' entries
  # Command substitutions run without errexit: check each one.
  entries=$(publish_entries "$pins") || die "$pins has no complete [publish] record"
  while IFS=$'\t' read -r key package version checksum; do
    [[ "$package" == "rtao-upstream-$key" ]] || die "[publish.$key] names $package, not rtao-upstream-$key"
    [[ "$version" =~ ^0\.[0-9]+\.[0-9]+$ ]] || die "[publish.$key] version $version is not 0.<release>.<revision>"
    if [[ "$checksum" == - ]]; then
      rows=$(jq -c --arg key "$key" --arg package "$package" --arg version "$version" \
        '. + [{key: $key, package: $package, version: $version}]' <<<"$rows")
    else
      [[ "$checksum" =~ ^[0-9a-f]{64}$ ]] || die "[publish.$key] checksum is not a lowercase SHA256"
    fi
  done <<<"$entries"
  printf '%s\n' "$rows"
}

# The publication plan, a pure function of the pins, `forks order` and
# `forks verify-packaged`'s output: {commit, entries: [{key, package,
# version, sha256}]} for the pending packages, each with the SHA256 of the
# archive verified against its revision. Fails closed when the order differs
# from this script's or a pending package was not verified.
forks_plan() {
  local pins=${1:?forks-plan <upstream.toml> <order.txt> <verified.txt> <commit> <plan.json>}
  local order=${2:?} verified=${3:?} commit=${4:?} out=${5:?}
  local expected key package version sha entries='[]' pending
  expected=$(printf 'rtao-upstream-%s\n' "${fork_order[@]}")
  [[ "$(cat "$order")" == "$expected" ]] || die "cargo xtask forks order differs from ci/runtime-adopt.sh's publication order"
  [[ "$commit" =~ ^[0-9a-f]{40}$ ]] || die "the plan needs a full commit, not $commit"
  pending=$(forks_pending "$pins" | jq -r '.[] | [.key, .package, .version] | @tsv') || die "no plan without a complete [publish] record"
  while IFS=$'\t' read -r key package version; do
    [[ -n "$key" ]] || continue
    sha=$(awk -v prefix="$package $version: packaged archive verified, sha256 " \
      'index($0, prefix) == 1 { print substr($0, length(prefix) + 1); exit }' "$verified")
    [[ "$sha" =~ ^[0-9a-f]{64}$ ]] || die "$package $version was not verified by forks verify-packaged"
    entries=$(jq -c --arg key "$key" --arg package "$package" --arg version "$version" --arg sha "$sha" \
      '. + [{key: $key, package: $package, version: $version, sha256: $sha}]' <<<"$entries")
  done <<<"$pending"
  jq -n --arg commit "$commit" --argjson entries "$entries" '{commit: $commit, entries: $entries}' >"$out"
  jq -c '{commit, entries: (.entries | map("\(.package) \(.version)"))}' "$out"
}

# Read-only job: list what awaits publication; when anything does, check the
# generated packages against the revisions, package all six (the verifying
# build, against a local overlay of the unpublished ones), verify each archive
# against its revision and write the plan.
forks_package() {
  local out=${1:?forks-package <out-dir>} checkout=${RTAO_ADOPT_CHECKOUT:-$root} key args=()
  : "${RTAO_TEMP_ROOT:?task-scoped scratch}"
  mkdir -p "$out"
  cd "$checkout"
  forks_pending upstream.toml >"$out/pending.json"
  if [[ "$(jq length "$out/pending.json")" == 0 ]]; then
    echo "every republished package has a recorded checksum; nothing to publish"
    return 0
  fi
  echo "awaiting publication: $(jq -r 'map("\(.package) \(.version)") | join(", ")' "$out/pending.json")"
  xtask forks check
  xtask forks order >"$out/order.txt"
  for key in "${fork_order[@]}"; do args+=(-p "rtao-upstream-$key"); done
  # forks verify-packaged reads the archives under the workspace target/.
  CARGO_TARGET_DIR=$checkout/target cargo package --locked "${args[@]}"
  xtask forks verify-packaged | tee "$out/verified.txt"
  forks_plan upstream.toml "$out/order.txt" "$out/verified.txt" "$(git rev-parse HEAD)" "$out/plan.json"
}

# The index path of a crate name (crates.io's sparse index layout).
index_path() {
  local name=${1,,}
  case ${#name} in
    1) echo "1/$name" ;;
    2) echo "2/$name" ;;
    3) echo "3/${name:0:1}/$name" ;;
    *) echo "${name:0:2}/${name:2:2}/$name" ;;
  esac
}

# The checksum crates.io's index records for one version: printed when the
# version is published, nothing when the crate or version is absent, and a
# failure when the index cannot be read.
index_checksum() {
  local name=$1 version=$2 body status
  body=$(mktemp)
  if ! status=$(curl -sS --max-time 30 --retry 3 --retry-delay 5 -H 'Cache-Control: no-cache' \
    -o "$body" -w '%{http_code}' "$crates_index/$(index_path "$name")"); then
    rm -f "$body"
    return 1
  fi
  case "$status" in
    200)
      if ! jq -r --arg version "$version" 'select(.vers == $version) | .cksum' "$body" | head -1; then
        rm -f "$body"
        return 1
      fi
      ;;
    404) ;;
    *)
      rm -f "$body"
      return 1
      ;;
  esac
  rm -f "$body"
}

# Upload one package unless crates.io already holds it; prints "published" or
# "existing". The archive is packaged here without building (the read-only
# job built and verified it) and uploaded only when its SHA256 is the
# verified one; a held version must carry that SHA256 too. A rate limit (429)
# waits and retries, doubling from RTAO_PUBLISH_PAUSE seconds (60: crates.io
# allows a burst of 30 new versions, then one a minute) up to ten minutes.
publish_one() {
  local checkout=$1 package=$2 version=$3 sha=$4
  local found actual log attempt=1 pause=${RTAO_PUBLISH_PAUSE:-60} attempts=${RTAO_PUBLISH_ATTEMPTS:-8}
  local archive=${CARGO_TARGET_DIR:-$checkout/target}/package/$package-$version.crate
  found=$(index_checksum "$package" "$version") || die "crates.io's index could not be read for $package"
  if [[ -n "$found" ]]; then
    [[ "$found" == "$sha" ]] || die "crates.io already holds $package $version with SHA256 $found, not the verified $sha"
    echo "$package $version: already on crates.io with the verified SHA256; not uploaded again" >&2
    echo existing
    return 0
  fi
  rm -f "$archive"
  (cd "$checkout" && cargo package --locked --no-verify -q -p "$package") >&2 ||
    die "cargo package $package failed; nothing uploaded"
  [[ -f "$archive" ]] || die "cargo package wrote no $archive; nothing uploaded"
  actual=$(sha256sum "$archive" | cut -c1-64)
  [[ "$actual" == "$sha" ]] || die "$package $version packages to SHA256 $actual here, not the verified $sha; nothing uploaded"
  log=$(mktemp)
  while :; do
    if (cd "$checkout" && cargo publish --locked --no-verify -p "$package") >"$log" 2>&1; then
      cat "$log" >&2
      break
    fi
    cat "$log" >&2
    # Uploaded despite the error (an index wait that timed out, or a
    # version an earlier attempt uploaded that a cached index page did not
    # show yet): the checksum is compared below.
    found=$(index_checksum "$package" "$version" || true)
    [[ -z "$found" ]] || break
    if grep -Eqi 'already (exists|uploaded)' "$log"; then
      break
    fi
    if grep -Eqi '429|too many requests|rate limit|try again after' "$log" && ((attempt < attempts)); then
      echo "$package $version: rate limited by crates.io; retrying in $pause s (attempt $attempt of $attempts)" >&2
      sleep "$pause"
      pause=$((pause * 2 > 600 ? 600 : pause * 2))
      attempt=$((attempt + 1))
      continue
    fi
    rm -f "$log"
    die "cargo publish $package $version failed (attempt $attempt)"
  done
  rm -f "$log"
  for attempt in $(seq 1 "${RTAO_INDEX_ATTEMPTS:-30}"); do
    found=$(index_checksum "$package" "$version" || true)
    [[ -z "$found" ]] || break
    sleep "${RTAO_INDEX_PAUSE:-10}"
  done
  [[ "$found" == "$sha" ]] ||
    die "after publishing, crates.io's index records $package $version with SHA256 ${found:-nothing}, not the verified $sha"
  echo published
}

# Token job: publish the plan's packages in order from the plan's commit.
# Builds nothing; refuses a plan that is not exactly the checkout's pending
# packages, and anything but rtao-upstream-* packages.
forks_publish() {
  local plan=${1:?forks-publish <plan.json> <published.json>}
  local out=${2:?forks-publish <plan.json> <published.json>}
  local checkout=${RTAO_ADOPT_CHECKOUT:-$root} key package version sha status results='[]'
  : "${CARGO_REGISTRY_TOKEN:?forks-publish needs CARGO_REGISTRY_TOKEN (the CRATES_IO_TOKEN secret)}"
  [[ "$(git -C "$checkout" rev-parse HEAD)" == "$(jq -r .commit "$plan")" ]] ||
    die "the checkout is not the plan's commit $(jq -r .commit "$plan")"
  [[ -z "$(git -C "$checkout" status --porcelain --untracked-files=no)" ]] || die "the checkout has local changes"
  [[ "$(jq -c '[.entries[] | {key, package, version}]' "$plan")" == "$(forks_pending "$checkout/upstream.toml" | jq -c .)" ]] ||
    die "the plan is not the checkout's packages awaiting publication"
  [[ "$(jq '.entries | length' "$plan")" -gt 0 ]] || die "the plan publishes nothing"
  printf '%s\n' "$results" >"$out"
  while IFS=$'\t' read -r key package version sha; do
    [[ "$package" == "rtao-upstream-$key" && " ${fork_order[*]} " == *" $key "* ]] ||
      die "refusing to publish $package: only the republished rtao-upstream-* packages are published here"
    [[ "$sha" =~ ^[0-9a-f]{64}$ ]] || die "the plan gives $package no verified SHA256"
    status=$(publish_one "$checkout" "$package" "$version" "$sha")
    results=$(jq -c --arg key "$key" --arg package "$package" --arg version "$version" --arg sha "$sha" --arg status "$status" \
      '. + [{key: $key, package: $package, version: $version, sha256: $sha, status: $status}]' <<<"$results")
    printf '%s\n' "$results" >"$out"
    echo "$package $version: $status"
  done < <(jq -r '.entries[] | [.key, .package, .version, .sha256] | @tsv' "$plan")
}

# Add each published package's checksum to its [publish.<key>] table, after
# its path line; the version must be the published one and no checksum
# recorded yet.
record_checksums() {
  local published=$1 pins=$2 key version sha
  while IFS=$'\t' read -r key version sha; do
    awk -v key="$key" -v version="$version" -v sha="$sha" '
      /^\[/ { inside = ($0 == "[publish." key "]") }
      inside && $1 == "version" && $2 == "=" { seen = ($3 == "\"" version "\"") }
      inside && $1 == "checksum" { bad = 1 }
      { print }
      inside && $1 == "path" && $2 == "=" && seen && !bad { print "checksum = \"" sha "\""; done = 1 }
      END { exit !(done && !bad) }
    ' "$pins" >"$pins.new" || die "[publish.$key] is not version $version without a checksum"
    mv "$pins.new" "$pins"
  done < <(jq -r '.[] | [.key, .version, .sha256] | @tsv' "$published")
}

# Read-only job: record the checksums in a candidate, check every published
# archive against its revision with forks verify-published, commit as the bot
# and write the bundle and its decision for the push job.
forks_record() {
  local plan=${1:?forks-record <plan.json> <published.json> <candidate> <evidence-dir>}
  local published=${2:?} candidate=${3:?} evidence=${4:?} checkout=${RTAO_ADOPT_CHECKOUT:-$root} list
  : "${RTAO_TEMP_ROOT:?task-scoped scratch}"
  mkdir -p "$evidence"
  [[ "$(git -C "$checkout" rev-parse HEAD)" == "$(jq -r .commit "$plan")" ]] ||
    die "the checkout is not the plan's commit $(jq -r .commit "$plan")"
  [[ "$(jq -c '[.entries[] | {key, package, version, sha256}]' "$plan")" == "$(jq -c '[.[] | {key, package, version, sha256}]' "$published")" ]] ||
    die "the published packages are not the plan's"
  git -C "$checkout" worktree add --detach "$candidate" HEAD >/dev/null
  cd "$candidate"
  record_checksums "$published" upstream.toml
  cp upstream.toml crates/rtao-core/upstream.toml
  cp upstream.toml crates/rtao-testkit/upstream.toml
  [[ "$(forks_pending upstream.toml | jq length)" == 0 ]] || die "packages still lack a recorded checksum"
  xtask crate-inputs --check
  xtask forks verify-published | tee "$evidence/verify-published.txt"
  cp upstream.toml "$evidence/upstream.toml"
  list=$(jq -r 'map("\(.package) \(.version)") | join(", ")' "$published")
  git add -A
  bot commit -q -m "Record the republished packages' crates.io checksums automatically (D-188) [skip ci]" \
    -m "Published by the runtime-adopt workflow from $(jq -r .commit "$plan"): $list. Each archive's
SHA256 equals forks verify-packaged's value for that commit, and cargo xtask
forks verify-published passes."
  check_paths HEAD~1 HEAD publish-forks
  git bundle create -q "$evidence/forks.bundle" HEAD~1..HEAD
  jq -n --argjson published "$(jq -c . "$published")" \
    '{action: "publish-forks", network: "crates.io", spec: null, code: null,
      id: "forks", packages: ($published | map("\(.package)@\(.version)"))}' >"$evidence/forks-decision.json"
  git rev-parse HEAD
}

# The release a newer set of republished packages calls for, as JSON, from
# the last release tag and its upstream.toml: a core release that moved (the
# minor of a package version) is a minor bump before 1.0 (major after),
# since rtao-core's public API carries the core's types; adaptation
# revisions alone are a patch bump. Before 1.0 a minor release may break
# public APIs and a patch release may not (docs/releases/README.md).
release_bump() {
  local tag=${1:?release-bump <tag> <tagged-upstream.toml> <upstream.toml>}
  local old=${2:?} new=${3:?} changed='[]' key package before after kind checksum entries previous
  [[ "$tag" =~ ^rtao-v([0-9]+)\.([0-9]+)\.([0-9]+)$ ]] || die "$tag is not an rtao-vX.Y.Z tag"
  local major=${BASH_REMATCH[1]} minor=${BASH_REMATCH[2]} patch=${BASH_REMATCH[3]}
  entries=$(publish_entries "$new") || die "$new has no complete [publish] record"
  previous=$(publish_entries "$old") || die "$old has no complete [publish] record"
  while IFS=$'\t' read -r key package after checksum; do
    before=$(awk -F'\t' -v key="$key" '$1 == key { print $3 }' <<<"$previous")
    [[ "$before" == "$after" ]] && continue
    changed=$(jq -c --arg package "$package" --arg before "$before" --arg after "$after" \
      '. + [{package: $package, from: $before, to: $after,
             release: (($before | split(".")[1]) != ($after | split(".")[1]))}]' <<<"$changed")
  done <<<"$entries"
  kind=$(jq -r 'if length == 0 then "none" elif any(.release) then "breaking" else "patch" end' <<<"$changed")
  local version
  case "$kind:$major" in
    none:*) version="" ;;
    breaking:0) version=0.$((minor + 1)).0 ;;
    breaking:*) version=$((major + 1)).0.0 ;;
    patch:*) version=$major.$minor.$((patch + 1)) ;;
  esac
  jq -n --arg tag "$tag" --arg version "$version" --arg kind "$kind" --argjson changed "$changed" \
    '{tag: $tag, from: ($tag | ltrimstr("rtao-v")), version: (if $version == "" then null else $version end),
      bump: (if $kind == "none" then null elif $kind == "patch" then "patch"
             elif ($tag | ltrimstr("rtao-v") | startswith("0.")) then "minor" else "major" end),
      changed: $changed}'
}

# Open or refresh the issue proposing an rtao release for the republished
# packages now on crates.io. Never tags or publishes: that is the
# maintainer's decision (crates-io.md section 5).
propose_release() {
  local evidence=$1 run=$2 tag tagged proposal number title body
  # RTAO_RELEASE_TAG and RTAO_RELEASE_TAGGED_PINS stand in for the remote's
  # last release tag and its upstream.toml (ci/runtime-adopt-test.sh).
  tag=${RTAO_RELEASE_TAG:-$(git -C "$root" ls-remote --tags --refs origin 'refs/tags/rtao-v*' 2>/dev/null |
    sed 's|.*refs/tags/||' | grep -E '^rtao-v[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -1 || true)}
  if [[ -z "$tag" ]]; then
    echo "no rtao-v release tag; no release to propose"
    return 0
  fi
  [[ -f "$evidence/upstream.toml" ]] || die "the record's upstream.toml is missing from the evidence"
  tagged=$(mktemp)
  if [[ -n "${RTAO_RELEASE_TAGGED_PINS:-}" ]]; then
    cp "$RTAO_RELEASE_TAGGED_PINS" "$tagged"
  else
    git -C "$root" fetch -q --depth=1 origin "refs/tags/$tag:refs/tags/$tag" 2>/dev/null || true
    git -C "$root" show "$tag:upstream.toml" >"$tagged" || { rm -f "$tagged"; die "could not read upstream.toml at $tag"; }
  fi
  proposal=$(release_bump "$tag" "$tagged" "$evidence/upstream.toml")
  rm -f "$tagged"
  if [[ "$(jq -r .version <<<"$proposal")" == null ]]; then
    echo "the republished packages equal $tag's; no release to propose"
    return 0
  fi
  title="Release proposal: rtao v$(jq -r .version <<<"$proposal") for the republished core packages"
  body="$release_marker
<!-- rtao-release-proposal version=$(jq -r .version <<<"$proposal") -->
New republished core packages are on crates.io, verified against their
revisions (\`cargo xtask forks verify-published\`) and recorded in
\`upstream.toml\` \`[publish]\` ([run]($run)). Since $tag:

$(jq -r '.changed | map("- `\(.package)` \(.from) -> \(.to)" + (if .release then " (core release)" else " (adaptation revision)" end)) | join("\n")' <<<"$proposal")

rtao's published crates still require the versions $tag recorded
(\`rtao-core\` pins each exactly), so applications get these packages only
through an rtao release. Proposed: **rtao v$(jq -r .version <<<"$proposal")**, a $(jq -r .bump <<<"$proposal") bump.
$(jq -r 'if .bump == "patch" then "Only adaptation revisions of the same core release changed; a patch release must not break public APIs (docs/releases/README.md)." else "The core release changed, and rtao-core'"'"'s public API carries the core'"'"'s types; before 1.0 a minor release may break public APIs (docs/releases/README.md). If `cargo semver-checks` finds the public API unchanged, a patch release is enough." end' <<<"$proposal")

This workflow never tags or publishes rtao crates: the release is the
maintainer's decision. Follow docs/releases/README.md and
docs/releases/crates-io.md section 5 (version bump of
\`[workspace.package]\` and every \`rtao-*\` entry of
\`[workspace.dependencies]\`, curated notes, CHANGELOG, the D-121 steps,
then \`cargo publish\`). Refreshed by the runtime-adopt workflow after each
publication; close it when the release is out or declined."
  number=$(open_issue "$release_marker")
  if [[ -z "$number" ]]; then
    gh issue create --repo "$GITHUB_REPOSITORY" --title "$title" --body "$body" >/dev/null
  else
    gh issue edit "$number" --repo "$GITHUB_REPOSITORY" --title "$title" --body "$body" >/dev/null
  fi
  echo "proposed rtao v$(jq -r .version <<<"$proposal")"
}

# Report the publication: a failure opens or refreshes one issue and alerts
# once per (packages, stage) through the existing alert endpoint; success
# closes that issue, resolves its alert and proposes a release; none (nothing
# awaits publication) closes an issue a maintainer resolved by hand;
# superseded (main moved before the record was pushed) reports nothing, and
# the next run records what this one published.
forks_notify() {
  local outcome=${1:?forks-notify <outcome> <stage> [<evidence-dir>]}
  local stage=${2:?forks-notify <outcome> <stage> [<evidence-dir>]} evidence=${3:-}
  : "${GITHUB_REPOSITORY:?forks-notify needs GITHUB_REPOSITORY and GH_TOKEN}"
  local run number open_body open_id open_stage packages id
  run=${GITHUB_SERVER_URL:-https://github.com}/$GITHUB_REPOSITORY/actions/runs/${GITHUB_RUN_ID:-}
  number=$(open_issue "$forks_marker")
  open_id="" open_stage=""
  if [[ -n "$number" ]]; then
    open_body=$(gh issue view "$number" --repo "$GITHUB_REPOSITORY" --json body --jq .body)
    open_id=$(sed -n 's/^<!-- rtao-forks-publish id=\([A-Za-z0-9._@,-]*\) stage=\([a-z-]*\) -->$/\1/p' <<<"$open_body" | head -1)
    open_stage=$(sed -n 's/^<!-- rtao-forks-publish id=\([A-Za-z0-9._@,-]*\) stage=\([a-z-]*\) -->$/\2/p' <<<"$open_body" | head -1)
  fi
  packages=""
  if [[ -n "$evidence" && -f "$evidence/pending.json" ]]; then
    packages=$(jq -r 'map("\(.package)@\(.version)") | join(",")' "$evidence/pending.json")
  fi
  id=forks-$(printf '%s' "${packages:-unknown}" | sha256sum | cut -c1-10)
  case "$outcome" in
    failure)
      local title body tail_text=""
      if [[ -n "$evidence" ]]; then
        tail_text=$(cat "$evidence/package.log" "$evidence/publish.log" "$evidence/record.log" 2>/dev/null | tail -n 60 || true)
      fi
      title="Republished core packages: publication failed at stage $stage"
      body="$forks_marker
<!-- rtao-forks-publish id=$id stage=$stage -->
The runtime-adopt workflow could not publish and record the republished
core packages awaiting publication on main: ${packages:-(unknown; the package stage failed before listing them)}.

| | |
|---|---|
| Stage | \`$stage\` (package: verify and package in the read-only job; publish: upload; verify: forks verify-published and the record; push: the record commit) |
| Run | $run |

Nothing was committed. A version already on crates.io stays (crates.io
versions are immutable; yank and publish a new revision if one is wrong). The
next scheduled run retries: a version crates.io holds with the verified
SHA256 is not uploaded again, and one with another checksum stops the run.

<details><summary>Log tail</summary>

\`\`\`text
$tail_text
\`\`\`
</details>"
      if [[ -z "$number" ]]; then
        gh issue create --repo "$GITHUB_REPOSITORY" --title "$title" --body "$body" >/dev/null
      elif [[ "$open_id" != "$id" || "$open_stage" != "$stage" ]]; then
        gh issue edit "$number" --repo "$GITHUB_REPOSITORY" --title "$title" --body "$body" >/dev/null
      fi
      if [[ -n "$open_id" && "$open_id" != "$id" ]]; then
        alert "$open_id" false "republished packages: superseded by another publication" "$run"
      fi
      if [[ "$open_id" != "$id" || "$open_stage" != "$stage" ]]; then
        alert "$id" true "$title" "packages: ${packages:-unknown}; $run"
      else
        echo "same publication failure as the open issue records; no new alert"
      fi
      ;;
    success)
      if [[ -n "$number" ]]; then
        gh issue close "$number" --repo "$GITHUB_REPOSITORY" \
          --comment "Published and recorded: $packages ([run]($run))." >/dev/null
        [[ -z "$open_id" ]] || alert "$open_id" false "republished packages published and recorded" "$run"
      fi
      propose_release "$evidence" "$run"
      ;;
    none)
      if [[ -n "$number" ]]; then
        gh issue close "$number" --repo "$GITHUB_REPOSITORY" \
          --comment "Every republished package on main has a recorded checksum ([run]($run))." >/dev/null
        [[ -z "$open_id" ]] || alert "$open_id" false "republished packages recorded" "$run"
      fi
      ;;
    superseded)
      echo "main moved before the record was pushed; the next run records the published packages"
      ;;
    *) die "unknown outcome $outcome" ;;
  esac
}

# ── notify ────────────────────────────────────────────────────────────

# POST one alert episode as JSON to the optional endpoint RUNTIME_ALERT_URL
# (the full URL) with bearer RUNTIME_ALERT_TOKEN, both repository secrets;
# unset, nothing is posted. Fields: id (stable per network, spec and :code,
# so an hourly repeat of one failure is the same episode), code
# (runtime.adopt_failed), source, severity, open (false resolves it after
# an adoption), summary and detail.
alert() {
  local id=$1 open=$2 summary=$3 detail=$4
  if [[ -z "${RUNTIME_ALERT_URL:-}" || -z "${RUNTIME_ALERT_TOKEN:-}" ]]; then
    echo "no alert endpoint configured; skipping the alert"
    return 0
  fi
  local body
  body=$(jq -cn --arg id "$id" --argjson open "$open" --arg summary "$summary" --arg detail "$detail" \
    '{id: $id, code: "runtime.adopt_failed", source: "github_actions", severity: "high",
      open: $open, summary: $summary, detail: $detail}')
  # The token goes in a header read from stdin, never on the command line.
  if ! printf 'Authorization: Bearer %s\n' "$RUNTIME_ALERT_TOKEN" |
    curl -sS --fail --max-time 20 --retry 3 --retry-delay 5 -o /dev/null \
      -H @- -H 'Content-Type: application/json' --data "$body" \
      "$RUNTIME_ALERT_URL"; then
    echo "::warning::the runtime alert could not be posted"
  fi
}

open_issue() {
  gh issue list --repo "$GITHUB_REPOSITORY" --state open --limit 100 \
    --json number,body --jq "map(select(.body | contains(\"$1\"))) | .[0].number // empty"
}

# The backoff states after this run ({id: state}), from the decision, the
# outcome, the failing stage and the states the open issue recorded (or
# null). failures counts the failed attempts of this id with the same
# adoption inputs, whatever stage each failed at (a flaky stage must not
# reset the backoff); changed inputs start again. attempted_at is when the
# attempt failed (RTAO_ADOPT_NOW, else now), not when it was decided: an
# adoption can run for hours, and the delay is measured from its end. A
# blocked decision runs nothing: it keeps the recorded attempts and adds
# none, so it never delays a later adoption. A recorded state is dropped
# only when its network was observed this run on another runtime (it moved
# on) or, for the decide step's own state, when the decide step worked; an
# unobserved or unconfirmed network, another network's report or a failed
# decide step never resets a backoff. stage is the last reported stage (one
# alert per change of stage).
next_state() {
  local decision=${1:?state <decision.json> <outcome> <stage> [<previous-state.json>]}
  local outcome=${2:?state <decision.json> <outcome> <stage> [<previous-state.json>]}
  local stage=${3:?state <decision.json> <outcome> <stage> [<previous-state.json>]}
  local previous=null
  if [[ -n "${4:-}" && -s "${4:-}" ]]; then
    previous=$(jq -c . "$4")
  fi
  jq -c --arg outcome "$outcome" --arg stage "$stage" --argjson previous "$previous" \
    --arg now "${RTAO_ADOPT_NOW:-$(date -u +%Y-%m-%dT%H:%M:%SZ)}" '
    ($previous // {}) as $all
    | ($all[.id] // null) as $p
    | ($p != null and ($p.inputs // null) == (.inputs // null)) as $same
    | (if $same then ($p.failures // 1) else 0 end) as $before
    | . as $d
    | def current($network):
        ($d.observed[$network] // null) as $n
        | if $n == null or ($n.error // null) != null or $n.spec == null then null
          else "\($network)-\($n.spec)-\($n.code[0:10])" end;
      def kept($key):
        ($key | split("-")[0]) as $network
        | if $key == "decide" then $d.id == "decide"
          elif $network == "finney" or $network == "testnet" then
            current($network) as $now | $now == null or $now == $key
          else true end;
      ($all | with_entries(select(kept(.key))))
      + {(.id): {id: .id, stage: $stage, inputs: (.inputs // null),
                 failures: (if .action == "blocked" then $before else $before + 1 end),
                 attempted_at: (if .action != "blocked" then $now
                                elif $same then ($p.attempted_at // null) else null end)}}
  ' "$decision"
}

# Open, refresh or close the informational issue for hand ports that an
# adoption left due. Never alerts: the runtime itself is adopted.
notify_port_due() {
  local file=$1 number body rows
  number=$(open_issue "$port_due_marker")
  rows=""
  if [[ -s "$file" ]]; then
    rows=$(awk '
      /^\[\[file\]\]/ { if (py != "") print "- `" py "` (" kind ", due since " tag ")" symbols; py = ""; kind = ""; tag = ""; symbols = "" }
      $1 == "py" { py = $3; gsub(/"/, "", py) }
      $1 == "kind" { kind = $3; gsub(/"/, "", kind) }
      $1 == "tag" { tag = $3; gsub(/"/, "", tag) }
      $1 == "symbols" { sub(/^[^=]*=[ \t]*/, ""); if ($0 != "[]") symbols = ": " $0 }
      END { if (py != "") print "- `" py "` (" kind ", due since " tag ")" symbols }
    ' "$file")
  fi
  if [[ -n "$rows" ]]; then
    body="$port_due_marker
Automatic runtime adoption moved the parity pin, and these upstream sources
changed in ways the adoption could not reconcile on evidence alone. The
runtime is adopted; these hand ports are due:

$rows

Port each change, then \`cargo xtask parity bless <path>\`, which removes its
entry from \`parity/port_due.toml\`. Until then ordinary parity accepts the
listed exact changes and strict parity (pr-gates) names them. Refreshed by the
runtime-adopt workflow; closed when no port is due."
    if [[ -z "$number" ]]; then
      gh issue create --repo "$GITHUB_REPOSITORY" --title "Python parity: $(grep -c '^- ' <<<"$rows") hand port(s) due after runtime adoption" --body "$body" >/dev/null
    elif ! gh issue view "$number" --repo "$GITHUB_REPOSITORY" --json body --jq .body | cmp -s - <(printf '%s\n' "$body"); then
      gh issue edit "$number" --repo "$GITHUB_REPOSITORY" --body "$body" >/dev/null
    fi
  elif [[ -n "$number" ]]; then
    gh issue close "$number" --repo "$GITHUB_REPOSITORY" --comment "No hand port is due." >/dev/null
  fi
}

# The alert episodes an open issue tracks: its recorded states' ids and the
# id its body reports, each once.
tracked_ids() {
  { jq -r '(. // {}) | keys[]' <<<"$1"; [[ -z "$2" ]] || echo "$2"; } | sort -u
}

notify() {
  local decision=${1:?notify <decision.json> <outcome> [<evidence-dir>]}
  local outcome=${2:?notify <decision.json> <outcome> [<evidence-dir>]}
  local evidence=${3:-}
  : "${GITHUB_REPOSITORY:?notify needs GITHUB_REPOSITORY and GH_TOKEN}"
  local action id reason spec network run stage number
  action=$(jq -r .action "$decision")
  id=$(jq -r .id "$decision")
  reason=$(jq -r .reason "$decision")
  spec=$(jq -r .spec "$decision")
  network=$(jq -r .network "$decision")
  run=${GITHUB_SERVER_URL:-https://github.com}/$GITHUB_REPOSITORY/actions/runs/${GITHUB_RUN_ID:-}
  stage=$(jq -r '.stage // "decide"' "$evidence/stage.json" 2>/dev/null || echo decide)

  # Upcoming upstream runtimes: one informational issue, refreshed or closed.
  local upcoming number_up body_up
  upcoming=$(jq -r '.upcoming | map("- `\(.branch)` declares spec \(.spec) at `\(.head)`") | join("\n")' "$decision")
  number_up=$(open_issue "$upcoming_marker")
  if [[ -n "$upcoming" ]]; then
    body_up="$upcoming_marker
Upstream branches declare a runtime newer than any network runs:

$upcoming

Nothing is pinned until a network finalizes it (the parity pin follows Finney).
Pre-stage with \`cargo xtask upstream-next --branch <branch>\` and the drift
preview. Refreshed by the runtime-adopt workflow; closed when no branch is ahead."
    if [[ -z "$number_up" ]]; then
      gh issue create --repo "$GITHUB_REPOSITORY" --title "Upcoming runtime on upstream: $(jq -r '.upcoming | map("\(.branch) \(.spec)") | join(", ")' "$decision")" --body "$body_up" >/dev/null
    else
      gh issue edit "$number_up" --repo "$GITHUB_REPOSITORY" --body "$body_up" >/dev/null
    fi
  elif [[ -n "$number_up" ]]; then
    gh issue close "$number_up" --repo "$GITHUB_REPOSITORY" --comment "No upstream branch is ahead of the networks now." >/dev/null
  fi

  # Due hand ports: the adopted candidate's list after a successful adoption,
  # otherwise the list on main; a superseded run's checkout is not main
  # (D-168), so it leaves the issue to the next run.
  if [[ "$outcome" == superseded ]]; then
    :
  elif [[ "$outcome" == success && -n "$evidence" && -f "$evidence/port_due.checked" ]]; then
    notify_port_due "$evidence/port_due.toml"
  else
    notify_port_due "${RTAO_ADOPT_PORT_DUE:-$root/parity/port_due.toml}"
  fi

  number=$(open_issue "$adopt_marker")
  # previous: every state the open issue records; open_id: the failure its
  # body currently reports; prior_stage: the stage recorded for this id.
  local previous=null open_id="" prior_stage=""
  if [[ -n "$number" ]]; then
    local open_body
    open_body=$(gh issue view "$number" --repo "$GITHUB_REPOSITORY" --json body --jq .body)
    previous=$(state_from_body <<<"$open_body")
    open_id=$(sed -n 's/^<!-- rtao-runtime-adopt id=\([A-Za-z0-9._:-]*\) stage=.*/\1/p' <<<"$open_body" | head -1)
    prior_stage=$(jq -r --arg id "$id" '(. // {})[$id].stage // empty' <<<"$previous")
  fi
  case "$outcome" in
    success)
      # Close the issue when it tracks this runtime, or when nothing else
      # (a blocked or backed-off network) is still outstanding.
      if [[ -n "$number" ]] && { [[ -z "$open_id" || "$open_id" == "$id" ]] ||
        [[ "$(jq '[.deferred[] | select(.action == "blocked" or .action == "backoff")] | length' "$decision")" == 0 ]]; }; then
        gh issue close "$number" --repo "$GITHUB_REPOSITORY" \
          --comment "Adopted: $action for $network spec $spec ([run]($run))." >/dev/null
        local other
        for other in $(tracked_ids "$previous" "$open_id"); do
          [[ "$other" == "$id" ]] || alert "$other" false "runtime adoption resolved" "$run"
        done
      fi
      alert "$id" false "$network runtime $spec adopted" "$run"
      ;;
    none)
      # An unobserved network proves nothing: keep the issue (and its alert)
      # open until both networks are seen on indexed runtimes.
      if [[ "$(jq '.unobserved // [] | length' "$decision")" != 0 ]]; then
        echo "not closing: $(jq -r '.unobserved | join("; ")' "$decision")"
      elif [[ "$open_id" == core-* ]] &&
        toml_get "${RTAO_ADOPT_PINS_ROOT:-.}/upstream.toml" parity core_fingerprint >/dev/null; then
        # A failed core bump is resolved by a core bump (D-136), never by
        # the networks running indexed runtimes.
        echo "not closing: the open issue reports a core bump ($open_id)"
      elif [[ "$open_id" == core-* && -n "$number" ]]; then
        # The pins no longer hold the core: the core bump landed (D-139).
        gh issue close "$number" --repo "$GITHUB_REPOSITORY" \
          --comment "Core bumped: \`[core]\` is at $(toml_get "${RTAO_ADOPT_PINS_ROOT:-.}/upstream.toml" core tag), the parity pin's core, and no core is held ([run]($run))." >/dev/null
        local other
        for other in $(tracked_ids "$previous" "$open_id"); do
          alert "$other" false "core bump resolved: the core is no longer held" "$run"
        done
      elif [[ -n "$number" ]]; then
        gh issue close "$number" --repo "$GITHUB_REPOSITORY" \
          --comment "Finney and testnet run indexed runtimes again ([run]($run))." >/dev/null
        # Resolve every episode the issue tracked (a maintainer finished them).
        local other
        for other in $(tracked_ids "$previous" "$open_id"); do
          alert "$other" false "runtime adoption resolved: Finney and testnet run indexed runtimes" "$run"
        done
      fi
      ;;
    superseded)
      echo "main moved since this run's checkout; the next scheduled run decides again from the new main"
      ;;
    backoff)
      echo "backing off: $reason"
      ;;
    failure)
      local tail_text=""
      if [[ -n "$evidence" && -f "$evidence/log.txt" ]]; then
        tail_text=$(tail -n 60 "$evidence/log.txt")
      fi
      local title body state retry
      if [[ "$action" == blocked ]]; then
        title="Runtime adoption blocked: $network spec $spec"
        stage=decide
      elif [[ "$action" == core-bump ]]; then
        title="Core bump failed: $(jq -r '"\(.core.from) to \(.core.to)"' "$decision") at stage $stage"
      else
        title="Runtime adoption failed: $network spec $spec at stage $stage"
      fi
      local previous_file
      previous_file=$(mktemp)
      printf '%s\n' "$previous" >"$previous_file"
      state=$(next_state "$decision" failure "$stage" "$previous_file")
      rm -f "$previous_file"
      if [[ "$action" == blocked ]]; then
        retry="Nothing runs while it is blocked; the next scheduled run re-evaluates it."
      else
        retry="The next attempt waits $(jq -r --arg id "$id" '[pow(2; .[$id].failures - 1), 24] | min' <<<"$state") h after this one failed (doubling per failed attempt, at most 24 h) unless the adoption inputs change (a fix merged to main) or the workflow is dispatched with force."
      fi
      body="$adopt_marker
<!-- rtao-runtime-adopt id=$id stage=$stage -->
<!-- rtao-runtime-adopt-state $state -->
$reason.

| | |
|---|---|
| Action | \`$action\` |
| Network and spec | $network $spec, \`:code\` \`$(jq -r .code "$decision")\` |
| Stage | \`$stage\` |
| Failed attempts with these inputs | $(jq -r --arg id "$id" '.[$id].failures' <<<"$state") |
| Run | $run |
$(jq -r 'if (.deferred // []) == [] then "" else "\nAlso outstanding:\n\n" + (.deferred | map("- `\(.action)` \(.network) spec \(.spec): \(.reason)") | join("\n")) + "\n" end' "$decision")

Main is unchanged. $retry The runtime adoption policy (AGENTS.md) applies:
make the focused fix the failing stage names, rerun the stage commands locally
(the workflow's adopt step lists them in order), and commit; the next run
closes this issue once Finney and testnet run indexed runtimes.

<details><summary>Log tail</summary>

\`\`\`text
$tail_text
\`\`\`
</details>"
      if [[ -z "$number" ]]; then
        gh issue create --repo "$GITHUB_REPOSITORY" --title "$title" --body "$body" >/dev/null
      elif [[ "$action" != blocked || "$open_id" != "$id" || "$prior_stage" != "$stage" ]]; then
        gh issue edit "$number" --repo "$GITHUB_REPOSITORY" --title "$title" --body "$body" >/dev/null
      fi
      # A state dropped above (its network moved to another runtime, or the
      # decide step works again) is no longer outstanding: resolve its alert.
      local dropped
      for dropped in $(tracked_ids "$previous" "$open_id"); do
        if [[ "$dropped" != "$id" && "$(jq --arg k "$dropped" 'has($k)' <<<"$state")" != true ]]; then
          alert "$dropped" false "runtime adoption no longer outstanding: $dropped" "$run"
        fi
      done
      # One alert per episode (network, spec, :code and stage); repeats of
      # the same failure update the issue only, even when another network's
      # report shared the issue in between.
      if [[ "$prior_stage" != "$stage" ]]; then
        alert "$id" true "$title" "$reason; $run"
      else
        echo "same failure as the open issue records; no new alert"
      fi
      ;;
    *) die "unknown outcome $outcome" ;;
  esac
}

command=${1:-}
shift || true
case "$command" in
  observe) observe "$@" ;;
  decide) decide "$@" ;;
  current) current "$@" ;;
  adopt) adopt "$@" ;;
  commit) commit "$@" ;;
  push) push "$@" ;;
  notify) notify "$@" ;;
  evidence)
    # Rehearse the live-evidence stage for the checkout's Finney pin.
    mkdir -p "${2:?evidence <decision.json> <evidence-dir>}"
    cd "$root"
    live_evidence "${1:?evidence <decision.json> <evidence-dir>}" "$2"
    ;;
  verdict) verdict "$@" ;;
  state) next_state "$@" ;;
  forks-pending) forks_pending "$@" ;;
  forks-package) forks_package "$@" ;;
  forks-plan) forks_plan "$@" ;;
  forks-publish) forks_publish "$@" ;;
  forks-record) forks_record "$@" ;;
  forks-notify) forks_notify "$@" ;;
  release-bump) release_bump "$@" ;;
  *)
    sed -n '2,22p' "$0" >&2
    exit 2
    ;;
esac
