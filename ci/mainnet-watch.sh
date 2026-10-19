#!/usr/bin/env bash
# Compare Finney's finalized runtime with the parity pin in upstream.toml.
#
#   ci/mainnet-watch.sh [--out <dir>] [--issue <owner/repo>]
#
# Exits 0 whether or not the runtimes match; a failed RPC, a wrong genesis or
# an unreadable pin exits nonzero. When they differ, writes the issue body to
# <dir>/issue.md. With --issue (needs gh and GH_TOKEN), opens one issue in
# <owner/repo>, or refreshes the open one, and closes it once the pin matches.
# Reads only public chain RPC and the public upstream repository.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
out=${RTAO_TEMP_ROOT:-${TMPDIR:-/tmp}}/mainnet-watch
issue_repo=
while (($#)); do
  case "$1" in
    --out)
      out=${2:?--out needs a directory}
      shift 2
      ;;
    --issue)
      issue_repo=${2:?--issue needs owner/repo}
      shift 2
      ;;
    *)
      echo "unknown argument $1" >&2
      exit 2
      ;;
  esac
done
mkdir -p "$out"

upstream_repo=RaoFoundation/subtensor
marker='<!-- rtao-mainnet-watch -->'
endpoints=${MAINNET_ENDPOINTS:-}

# Read one scalar from a [section] of a flat, generated TOML file.
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

pins=$root/upstream.toml
pin_spec=$(toml_get "$pins" parity spec_version)
pin_tag=$(toml_get "$pins" parity tag)
pin_commit=$(toml_get "$pins" parity commit)
pin_network=$(toml_get "$pins" parity network || echo "")
genesis=$(toml_get "$pins" networks.finney genesis)
metadata_spec=$(toml_get "$pins" networks.finney.metadata spec)
metadata_code=$(toml_get "$pins" networks.finney.metadata code_hash)
endpoints=${endpoints:-$(toml_get "$pins" networks.finney genesis_endpoint) https://archive.chain.opentensor.ai:443}
[[ "$pin_spec" =~ ^[0-9]+$ && "$metadata_spec" =~ ^[0-9]+$ ]]
[[ "$genesis" =~ ^0x[0-9a-f]{64}$ && "$metadata_code" =~ ^0x[0-9a-f]{64}$ ]]

# One JSON-RPC call against the selected endpoint.
rpc() {
  curl -sS --fail --max-time 20 --retry 2 --retry-delay 3 \
    -H 'Content-Type: application/json' \
    --data "$(jq -cn --arg method "$2" --argjson params "$3" \
      '{id: 1, jsonrpc: "2.0", method: $method, params: $params}')" "$1" |
    jq -ce '.result // empty'
}

# Use the first endpoint that answers with Finney's genesis.
endpoint=
for candidate in $endpoints; do
  if observed=$(rpc "$candidate" chain_getBlockHash '[0]' | jq -r .); then
    if [[ "$observed" == "$genesis" ]]; then
      endpoint=$candidate
      break
    fi
    echo "$candidate has genesis $observed, not Finney's $genesis" >&2
  fi
done
if [[ -z "$endpoint" ]]; then
  echo "no Finney endpoint answered with the pinned genesis" >&2
  exit 1
fi
finalized=$(rpc "$endpoint" chain_getFinalizedHead '[]' | jq -r .)
[[ "$finalized" =~ ^0x[0-9a-f]{64}$ ]]
number=$(($(rpc "$endpoint" chain_getHeader "[\"$finalized\"]" | jq -r .number)))
version=$(rpc "$endpoint" state_getRuntimeVersion "[\"$finalized\"]")
spec=$(jq -r .specVersion <<<"$version")
spec_name=$(jq -r .specName <<<"$version")
transaction_version=$(jq -r .transactionVersion <<<"$version")
code=$(rpc "$endpoint" state_getStorageHash "[\"0x3a636f6465\", \"$finalized\"]" | jq -r .)
[[ "$spec" =~ ^[0-9]+$ && "$code" =~ ^0x[0-9a-f]{64}$ ]]

echo "Finney finalized #$number ($finalized): spec $spec, :code $code"
echo "Pinned: parity $pin_tag (spec $pin_spec, $pin_network), finney metadata spec $metadata_spec, :code $metadata_code"

open_issue() {
  gh issue list --repo "$issue_repo" --state open --limit 100 \
    --json number,body --jq "map(select(.body | contains(\"$marker\"))) | .[0].number // empty"
}

if [[ "$spec" == "$pin_spec" && "$spec" == "$metadata_spec" && "$code" == "$metadata_code" ]]; then
  echo "match"
  rm -f "$out/issue.md"
  if [[ -n "$issue_repo" ]]; then
    number_open=$(open_issue)
    if [[ -n "$number_open" ]]; then
      gh issue close "$number_open" --repo "$issue_repo" \
        --comment "The parity pin now matches Finney: spec $spec, \`:code\` \`$code\` at finalized block $number."
    fi
  fi
  exit 0
fi

if [[ "$spec" == "$pin_spec" ]]; then
  title="Mainnet runtime code changed at spec $spec"
else
  title="Mainnet runtime upgraded to spec $spec"
fi
echo "$title"

# Upstream view: published release, tag, branch heads and the commits on main
# whose runtime source declares the observed spec. All reads are public.
gh_get() { gh api -H 'Accept: application/vnd.github.raw+json' "$@"; }
declared_spec() {
  local source
  source=$(gh_get "repos/$upstream_repo/contents/runtime/src/lib.rs?ref=$1" 2>/dev/null) || return 1
  awk -F'spec_version:' 'NF > 1 && !found { sub(/^[ \t]*/, "", $2); sub(/[^0-9].*/, "", $2); print $2; found = 1 }' <<<"$source"
}
upstream_url=https://github.com/$upstream_repo.git
release=no
if command -v gh >/dev/null && gh api "repos/$upstream_repo/releases/tags/v$spec" >/dev/null 2>&1; then
  release=yes
fi
tag_commit=$(git ls-remote "$upstream_url" "refs/tags/v$spec^{}" "refs/tags/v$spec" |
  awk 'NR == 1 || /\^\{\}$/ { commit = $1 } END { print commit }') || tag_commit=
heads=$(git ls-remote --heads "$upstream_url" main testnet devnet) || heads=
branch_rows=
candidates=
suggested=
if command -v gh >/dev/null; then
  while read -r sha ref; do
    branch_rows+="| \`${ref#refs/heads/}\` | \`$sha\` | $(declared_spec "$sha" || echo '?') |"$'\n'
  done <<<"$heads"
  # Release merges among the 100 newest commits on main, then the 30 newest
  # commits that touch the runtime version; keep those declaring the spec.
  seen=" "
  while IFS=$'\t' read -r sha parents subject; do
    [[ "$seen" == *" $sha "* ]] && continue
    seen+="$sha "
    if [[ "$(declared_spec "$sha" || true)" == "$spec" ]]; then
      kind=commit
      if ((parents > 1)); then
        kind=merge
        if [[ -z "$suggested" && "$subject" =~ release[-/_]?[^0-9]*$spec([^0-9]|$) ]]; then
          suggested=$sha
        fi
      fi
      candidates+="- \`$sha\` ($kind) $subject"$'\n'
    fi
  done < <(
    gh api "repos/$upstream_repo/commits?sha=main&per_page=100" --jq \
      ".[] | select(.commit.message | test(\"release[-/_]?[^0-9]*$spec([^0-9]|$)\"; \"i\")) | [.sha, (.parents | length), (.commit.message | split(\"\\n\")[0])] | @tsv"
    gh api "repos/$upstream_repo/commits?sha=main&path=runtime/src/lib.rs&per_page=30" --jq \
      '.[] | [.sha, (.parents | length), (.commit.message | split("\n")[0])] | @tsv'
  )
fi

if [[ "$release" == yes ]]; then
  suggestion="the published release \`v$spec\`${tag_commit:+ (\`$tag_commit\`)}"
elif [[ -n "$suggested" ]]; then
  suggestion="\`$suggested\`, the release-$spec merge on \`main\`"
else
  suggestion="none found; pick the commit on \`main\` that merged the release-$spec runtime"
fi
if [[ "$release" == yes ]]; then
  repin="RTAO_PIN_CANDIDATE=1 cargo xtask pin --parity v$spec --prepare-candidate"
  repin_note=
else
  repin="RTAO_PIN_CANDIDATE=1 cargo xtask pin --parity branch:main@${suggested:-<commit>} --prepare-candidate"
  repin_note=$'\n'"   (\`--parity v$spec\` once upstream publishes a v$spec release)."
fi
tag_line=none
[[ -n "$tag_commit" ]] && tag_line="\`$tag_commit\`"

cat >"$out/issue.md" <<EOF
$marker
<!-- rtao-mainnet-watch spec=$spec code=$code -->
Finney finalized a runtime that differs from rtao's parity pin.

| | Observed on Finney | Pinned in \`upstream.toml\` |
|---|---|---|
| Spec version | \`$spec\` (\`$spec_name\`, transaction version \`$transaction_version\`) | parity \`$pin_tag\` spec \`$pin_spec\`; finney metadata spec \`$metadata_spec\` |
| \`:code\` hash | \`$code\` | \`$metadata_code\` |
| Finalized block | \`#$number\` \`$finalized\` | parity commit \`$pin_commit\` |

Observed through \`$endpoint\`.

## Upstream

- Published release \`v$spec\`: $release
- Tag \`v$spec\`: $tag_line

| Branch | Head | Declared spec |
|---|---|---|
${branch_rows:-| ? | gh unavailable | ? |
}
Commits on \`main\` whose \`runtime/src/lib.rs\` declares spec $spec, newest first (release merges among the 100 newest
commits, and the 30 newest commits that touch that file):

${candidates:-- none found; check the release branch or a newer main.
}
Suggested pin: $suggestion.
\`pin\` proves that the commit is on the branch and declares the spec, and
records Finney's finalized metadata for it.

## Re-pin

1. \`$repin\`$repin_note
2. \`cargo xtask import-upstream\`, \`cargo xtask extract\`, \`cargo xtask regen\`,
   then port the source changes and reconcile \`parity/map.toml\`, exports and
   the e2e map until \`cargo xtask parity check --strict\` passes. If rtao
   already ported this spec and later moved away from it, reverting those
   pin-move commits restores the port and its generated surfaces.
3. \`cargo xtask compat --measure-build-weight --build-target aarch64-apple-darwin\`
   and \`cargo xtask compat\`; run the hosted \`validate_compat\` lanes and adopt
   their reports.
4. Add an entry to \`book/src/upstream-log.md\`.

This issue is refreshed while the runtimes differ and closed once the pin matches.
EOF

if [[ -z "$issue_repo" ]]; then
  cat "$out/issue.md"
  exit 0
fi
number_open=$(open_issue)
if [[ -z "$number_open" ]]; then
  gh issue create --repo "$issue_repo" --title "$title" --body-file "$out/issue.md"
elif ! gh issue view "$number_open" --repo "$issue_repo" --json body --jq .body |
  grep -qF "<!-- rtao-mainnet-watch spec=$spec code=$code -->"; then
  gh issue edit "$number_open" --repo "$issue_repo" --title "$title" --body-file "$out/issue.md"
else
  echo "issue #$number_open already records spec $spec"
fi
