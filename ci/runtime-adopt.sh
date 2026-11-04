#!/usr/bin/env bash
# Automatic adoption of activated Finney and testnet runtimes (AGENTS.md's
# runtime adoption policy), run hourly by .github/workflows/runtime-adopt.yml.
#
#   ci/runtime-adopt.sh observe <observed.json>
#   ci/runtime-adopt.sh decide <observed.json> <decision.json> [<pins-root>]
#   ci/runtime-adopt.sh adopt <decision.json> <candidate> <evidence-dir>
#   ci/runtime-adopt.sh commit <decision.json> <candidate> <bundle>
#   ci/runtime-adopt.sh push <decision.json> <bundle>
#   ci/runtime-adopt.sh notify <decision.json> <outcome> [<evidence-dir>]
#
# observe reads public chain RPC and the public upstream repository. decide
# is a pure function of an observation and the committed pins: it writes the
# one action of this run and needs no network, so ci/runtime-adopt-test.sh
# checks it against recorded and simulated observations.
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
#   blocked         Something only a maintainer can decide (same spec with
#                   new :code, no upstream source for a new spec, a pin ahead
#                   of its network): nothing runs; an issue and an alert.
# A red stage leaves main untouched and reports like blocked. Upstream
# branches that declare a newer spec than any network runs are listed as
# upcoming (an informational issue, no alert) so they can be pre-staged.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
upstream_repo=RaoFoundation/subtensor
adopt_marker='<!-- rtao-runtime-adopt -->'
upcoming_marker='<!-- rtao-runtime-upcoming -->'

die() {
  echo "runtime-adopt: $*" >&2
  exit 1
}

# One scalar from a [section] of a flat, generated TOML file.
toml_get() {
  awk -v section="[$2]" -v key="$3" '
    /^\[/ { inside = ($0 == section); next }
    inside && $1 == key && $2 == "=" {
      value = $0
      sub(/^[^=]*=[ \t]*/, "", value)
      gsub(/^"|"$/, "", value)
      print value
      found = 1
      exit
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

# The finalized runtime of a network as JSON, or {"error": ...}.
observe_network() {
  local name=$1 genesis=$2 endpoints=$3 endpoint="" candidate observed
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
  jq -cn --arg endpoint "$endpoint" --arg finalized "$finalized" --arg number "$number" \
    --argjson version "$version" --arg code "$code" \
    '{endpoint: $endpoint, finalized: $finalized, number: ($number | ltrimstr("0x") | explode | reduce .[] as $c (0; . * 16 + (if $c >= 97 then $c - 87 elif $c >= 65 then $c - 55 else $c - 48 end))),
      spec: $version.specVersion, spec_name: $version.specName,
      transaction_version: $version.transactionVersion, code: $code}'
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
    "${FINNEY_ENDPOINTS:-$(toml_get "$pins" networks.finney genesis_endpoint) https://archive.chain.opentensor.ai:443}")
  testnet=$(observe_network testnet "$(toml_get "$pins" networks.testnet genesis)" \
    "${TESTNET_ENDPOINTS:-$(toml_get "$pins" networks.testnet genesis_endpoint)}")
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
    '{schema: 1, observed_at: $at, finney: $finney, testnet: $testnet, upstream: {branches: $branches, sources: $sources}}' >"$out"
  jq -c '{finney: (.finney | {spec, code, number, error}), testnet: (.testnet | {spec, code, number, error}), upstream: .upstream.branches}' "$out"
}

# ── decide ────────────────────────────────────────────────────────────

decide() {
  local observed=${1:?decide <observed.json> <decision.json> [<pins-root>]}
  local out=${2:?decide <observed.json> <decision.json> [<pins-root>]}
  local base=${3:-$root}
  local pins=$base/upstream.toml
  local parity_spec parity_tag parity_network finney_spec finney_code
  parity_spec=$(toml_get "$pins" parity spec_version)
  parity_tag=$(toml_get "$pins" parity tag)
  parity_network=$(toml_get "$pins" parity network || echo finney)
  finney_spec=$(toml_get "$pins" networks.finney.metadata spec)
  finney_code=$(toml_get "$pins" networks.finney.metadata code_hash)
  jq -n \
    --slurpfile observed "$observed" \
    --argjson parity_spec "$parity_spec" --arg parity_tag "$parity_tag" --arg parity_network "$parity_network" \
    --argjson finney_spec "$finney_spec" --arg finney_code "$finney_code" \
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
   else null end) as $finney
| (if ($t.error // null) != null then null
   elif known($testnet_rows; $t.spec; $t.code) then null
   elif spec_known($testnet_rows; $t.spec) then
     result("blocked"; "testnet"; $t.spec; $t.code; "testnet's :code changed at indexed spec \($t.spec); immutable metadata cannot be overwritten")
   elif $parity_network == "testnet" and $t.spec > $parity_spec then
     result("blocked"; "testnet"; $t.spec; $t.code; "the parity pin follows testnet; spec \($t.spec) needs a pin, not an index record")
   else
     result("record-testnet"; "testnet"; $t.spec; $t.code; "testnet finalized spec \($t.spec) (:code \($t.code | short)…), absent from its index")
   end) as $testnet
| ([$f.spec, $t.spec, $parity_spec] | map(select(. != null)) | max) as $running
| ($o.upstream.branches // {} | to_entries
   | map(select(.value.spec != null and .value.spec > $running)
         | {branch: .key, head: .value.head, spec: .value.spec})) as $upcoming
| ([$finney, $testnet] | map(select(. != null))) as $wanted
# Progress first: a blocked network never holds up the other's adoption.
| ($wanted | map(select(.action == "adopt-finney")) + map(select(.action == "record-testnet")) + map(select(.action == "blocked"))) as $ordered
| ($ordered[0] // result("none"; null; null; null; "Finney and testnet run indexed runtimes"))
| . + {deferred: ($ordered[1:] | map({action, network, spec, id, reason})),
       upcoming: $upcoming,
       unobserved: ([(if ($f.error // null) != null then "finney: \($f.error)" else empty end),
                     (if ($t.error // null) != null then "testnet: \($t.error)" else empty end)]),
       pins: {parity_tag: $parity_tag, parity_spec: $parity_spec, finney_spec: $finney_spec, finney_code: $finney_code},
       observed: {finney: ($f | {spec, code, number, finalized, endpoint, error}),
                  testnet: ($t | {spec, code, number, finalized, endpoint, error})},
       observed_at: $o.observed_at}
JQ
  jq -c '{action, network, spec, source, id, reason, deferred: (.deferred | map(.id)), upcoming: (.upcoming | map("\(.branch)=\(.spec)")), unobserved}' "$out"
}

# ── adopt (hosted runner) ─────────────────────────────────────────────

stage_file=
stage() {
  echo "::group::runtime-adopt stage $1"
  jq -n --arg stage "$1" '{status: "running", stage: $stage}' >"$stage_file"
}
stage_done() { echo "::endgroup::"; }

xtask() { cargo run --locked -q -p xtask -- "$@"; }

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
      stage import
      # Pin preparation removed the prior copies; restore them with its binary
      # before recompiling consumers of those files (ci/pin-pipeline.sh).
      "$CARGO_TARGET_DIR/debug/xtask" import-upstream
      stage_done
      stage core
      if ! cmp -s "$root/Cargo.toml" Cargo.toml ||
        [[ "$(toml_get "$root/upstream.toml" core commit)" != "$(toml_get upstream.toml core commit)" ]]; then
        die "the core input moved with spec $spec: a core bump (core-patch rebase, lock update, core-surface) needs a maintainer"
      fi
      stage_done
      stage extract
      xtask extract
      stage_done
      stage regen
      xtask regen
      stage_done
      stage compat
      xtask compat --measure-build-weight --build-target aarch64-apple-darwin
      xtask compat
      stage_done
      stage parity
      xtask parity report --base "$root" --out "$evidence/porting-report.json" --autobless
      xtask parity check --strict
      stage_done
      stage drift
      local index=vendor/metadata/finney/index.toml
      xtask drift diff \
        "vendor/metadata/finney/$(index_blob "$index" "$old_spec" v15)" \
        "vendor/metadata/finney/$(index_blob "$index" "$spec" v15)" \
        --old-v16 "vendor/metadata/finney/$(index_blob "$index" "$old_spec" v16)" \
        --new-v16 "vendor/metadata/finney/$(index_blob "$index" "$spec" v16)" \
        --out "$evidence/drift" >"$evidence/drift.json"
      local class
      class=$(jq -r .classification "$evidence/drift.json")
      [[ "$class" != breaking ]] || die "metadata drift $old_spec -> $spec is breaking; see the drift report"
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
      stage record
      record_adoption "$decision" "$old_spec" "$class" "$evidence"
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
  local spec code number finalized source tag commit run blessed changed today
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
  today=$(date -u +%Y-%m-%d)
  changelog_entry "### Finney runtime $spec adopted automatically

- The parity pin moves from spec $old_spec to $tag (\`$commit\`, pinned as \`$source\`), the runtime Finney finalized at block $number with \`:code\` \`$code\`.
- Metadata drift $old_spec -> $spec: \`$class\`. Code generation, extracted tables and catalogs are regenerated from the pinned sources; watched sources changed: $changed, of which $blessed were reconciled automatically (unchanged symbols, extraction and oracle evidence).
- Checked by the hosted runtime-adopt workflow ([run]($run)): pin source identity, imports, extraction, regeneration, compatibility, probe pins, drift, strict parity, formatting, a workspace build and the runtime, reads, intents and client tests. No oracle comparison, localnet e2e or image execution; the validated range is unchanged."
  upstream_log_entry "## $today: Finney runtime $spec adopted automatically

The hosted runtime-adopt workflow observed Finney finalize spec $spec at block
$number (\`$finalized\`), \`:code\` \`$code\`, and advanced parity from spec
$old_spec to \`$tag\` (\`$commit\`, pin source \`$source\`). Metadata drift is
\`$class\`. The porting report lists $changed changed watched sources, $blessed of
them auto-blessed, and strict parity passes, so none needed a port. Core, its
patch and the validated range are unchanged. Workflow run: <$run>."
}

record_testnet() {
  local decision=$1 evidence=$2 spec code number run class
  spec=$(jq -r .spec "$decision")
  code=$(jq -r .code "$decision")
  number=$(jq -r .observed.testnet.number "$decision")
  class=$(jq -r '.classification // "recorded"' "$evidence/drift/report.json" 2>/dev/null || echo recorded)
  run=${GITHUB_SERVER_URL:-https://github.com}/${GITHUB_REPOSITORY:-local}/actions/runs/${GITHUB_RUN_ID:-local}
  changelog_entry "### Testnet runtime $spec recorded automatically

- Testnet finalized spec $spec (\`:code\` \`$code\`) at block $number; its V15/V16 metadata is indexed under \`vendor/metadata/testnet/\` (drift against the previous testnet row: \`$class\`) and the probe pins are regenerated ([run]($run)). The parity pin is unchanged."
}

# ── commit and push ───────────────────────────────────────────────────

bot() {
  git -c user.name="${RTAO_ADOPT_NAME:-github-actions[bot]}" \
    -c user.email="${RTAO_ADOPT_EMAIL:-41898282+github-actions[bot]@users.noreply.github.com}" "$@"
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
    *) die "nothing to commit for action $action" ;;
  esac
  git add -A
  bot commit -q -m "$title [skip ci]" -m "$body"
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
  case "$action" in adopt-finney | record-testnet) ;; *) die "nothing to push for action $action" ;; esac
  git bundle verify -q "$bundle" >/dev/null 2>&1 || die "the bundle does not apply to this checkout"
  git fetch -q "$bundle" HEAD
  commit=$(git rev-parse FETCH_HEAD)
  [[ "$(git rev-list --count --no-walk=unsorted "$commit^@")" == 1 ]] || die "the bundle's commit is not a single-parent commit"
  parent=$(git rev-parse "$commit^")
  git fetch -q origin main
  if [[ "$(git rev-parse origin/main)" != "$parent" ]]; then
    echo "superseded: main moved from $parent during the run; the next scheduled run starts again from the new main"
    return 0
  fi
  if [[ "$action" == adopt-finney ]]; then
    today=$(date -u +%Y-%m-%d)
    git fetch -q --tags origin
    tag=snapshot-$today
    n=1
    while git rev-parse -q --verify "refs/tags/$tag" >/dev/null; do
      n=$((n + 1))
      tag=snapshot-$today.$n
    done
    bot tag -a "$tag" -m "Snapshot of main: Finney runtime $spec adopted automatically ($(git show "$commit:upstream.toml" | toml_get /dev/stdin parity tag)). Not a versioned release; see CHANGELOG.md (Unreleased)." "$commit"
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
    echo "pushed $commit ($network runtime $spec)"
  fi
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

  number=$(open_issue "$adopt_marker")
  case "$outcome" in
    success)
      if [[ -n "$number" ]]; then
        gh issue close "$number" --repo "$GITHUB_REPOSITORY" \
          --comment "Adopted: $action for $network spec $spec ([run]($run))." >/dev/null
      fi
      alert "$id" false "$network runtime $spec adopted" "$run"
      ;;
    none)
      # An unobserved network proves nothing: keep the issue (and its alert)
      # open until both networks are seen on indexed runtimes.
      if [[ "$(jq '.unobserved // [] | length' "$decision")" != 0 ]]; then
        echo "not closing: $(jq -r '.unobserved | join("; ")' "$decision")"
      elif [[ -n "$number" ]]; then
        local open_id
        open_id=$(gh issue view "$number" --repo "$GITHUB_REPOSITORY" --json body --jq .body |
          sed -n 's/^<!-- rtao-runtime-adopt id=\([A-Za-z0-9._:-]*\) stage=.*/\1/p' | head -1)
        gh issue close "$number" --repo "$GITHUB_REPOSITORY" \
          --comment "Finney and testnet run indexed runtimes again ([run]($run))." >/dev/null
        # Resolve the episode the issue opened (a maintainer finished it).
        alert "${open_id:-none}" false "runtime adoption resolved: Finney and testnet run indexed runtimes" "$run"
      fi
      ;;
    superseded)
      echo "main moved during the run; the next scheduled run retries from the new main"
      ;;
    failure)
      local tail_text=""
      if [[ -n "$evidence" && -f "$evidence/log.txt" ]]; then
        tail_text=$(tail -n 60 "$evidence/log.txt")
      fi
      local title body
      if [[ "$action" == blocked ]]; then
        title="Runtime adoption blocked: $network spec $spec"
        stage=decide
      else
        title="Runtime adoption failed: $network spec $spec at stage $stage"
      fi
      body="$adopt_marker
<!-- rtao-runtime-adopt id=$id stage=$stage -->
$reason.

| | |
|---|---|
| Action | \`$action\` |
| Network and spec | $network $spec, \`:code\` \`$(jq -r .code "$decision")\` |
| Stage | \`$stage\` |
| Run | $run |

Main is unchanged. The runtime adoption policy (AGENTS.md) applies: make the
focused port the failing stage names, rerun the stage commands locally (the
workflow's adopt step lists them in order), and commit; the next scheduled run
closes this issue once Finney and testnet run indexed runtimes.

<details><summary>Log tail</summary>

\`\`\`text
$tail_text
\`\`\`
</details>"
      if [[ -z "$number" ]]; then
        gh issue create --repo "$GITHUB_REPOSITORY" --title "$title" --body "$body" >/dev/null
      elif ! gh issue view "$number" --repo "$GITHUB_REPOSITORY" --json body --jq .body |
        grep -qF "<!-- rtao-runtime-adopt id=$id stage=$stage -->"; then
        gh issue edit "$number" --repo "$GITHUB_REPOSITORY" --title "$title" --body "$body" >/dev/null
      fi
      alert "$id" true "$title" "$reason; $run"
      ;;
    *) die "unknown outcome $outcome" ;;
  esac
}

command=${1:-}
shift || true
case "$command" in
  observe) observe "$@" ;;
  decide) decide "$@" ;;
  adopt) adopt "$@" ;;
  commit) commit "$@" ;;
  push) push "$@" ;;
  notify) notify "$@" ;;
  *)
    sed -n '2,30p' "$0" >&2
    exit 2
    ;;
esac
