# M1-21 metadata drift

The implementation compares semantic runtime contracts from canonical V15/V16
SCALE inputs. It reports all pallets; call indices, argument names and exact
shapes; V16 call deprecations; event indices and fields; error indices; storage
prefixes, hashers, keys, values, modifiers and defaults; constant shapes and
bytes; runtime APIs and versions; configured model projections; signed
extension order and explicit/implied payloads; V16 extension mappings;
extrinsic versions; and `System.Version`.

Structural hashing and descriptor generation reuse the production runtime and
regeneration code. Outer runtime call/event/error enums stay opaque, so an
added inner call is additive. Reads and model projection changes are
behavioural. Changes to signing indices/arguments, extension payloads or order,
transaction version, spec name, V15 availability or v4 support are breaking.
An appended by-name model field preserves its configured projection.

`cargo xtask drift --network <network>` reads genesis and one finalized
runtime's spec, `:code` and metadata at immutable block hashes. Every committed
index blob is verified offline before an indexed `(spec, :code)` returns early.
`--check-open-prs` also inspects immutable open bot pin/drift PR heads, preventing
duplicate proposals. Unknown identities produce semantic JSON/Markdown and
verbatim independent `subxt diff` output, plus formatted descriptor previews.
A surface-unchanged preview must match the previous descriptors byte for byte.

`--apply` records immutable metadata and runs regeneration and offline tests.
A newer Finney runtime requires its normal source/parity pin bump before
recording or promoting descriptors; a preview never relabels the Python source
or substitutes localnet metadata for the Finney codegen input. Reusing a spec
with a different runtime code hash fails rather than overwriting pinned bytes.
Removed metadata or model incompatibilities leave a report before promotion
fails.

`.github/workflows/drift.yml` schedules the four networks every six hours on
free `ubuntu-latest`. Scheduled runs stay off until `RTAO_UPKEEP=on`.
A `workflow_dispatch` defaults to `dry-run=true`, performing acquisition,
regeneration and checks without a publishing token, push or PR. Publishing
creates the bot App token only after checks pass. Surface-unchanged upkeep PRs
request automatic merging after CI. The workflow never applies the `CI` label.
The `localnet-main` job starts the digest-pinned real main image; the tool
checks the running container's exact image identity before acquiring metadata.

Validation on 2026-10-14:

- Eleven drift tests passed, including the committed real 477->478 inputs, all
  required synthetic classes, spec 478 field append, fixed fraction changes,
  V16 extension mappings, all existing network indexes and identical descriptor
  regeneration.
- The same eleven tests passed on Rust 1.89. Strict xtask Clippy passed for all
  targets after integration with the other M1 commands.
- The real semantic diff classified 477->478 as `surface-unchanged`; independent
  `subxt diff` reported no metadata difference.
- Live Finney, testnet and devnet checks returned 0 against their indexed 478
  identities.
- Workflow actionlint 1.7.7, shell syntax and YAML parsing passed. Downloaded
  actionlint archive matched the published SHA256 checksum.

[Fixture acquisition evidence](m1-drift-fixtures.md) and its retained JSON
record document finalized blocks, checksums and source provenance.
[Comparison results](m1-drift-results.json) retain the real semantic report,
independent comparison and all three live identity checks. The actual
`localnet-main` baseline and hosted dry-run exit remain pending: this Mac has
no Docker daemon, so the code and workflow do not claim a local image run.
M1-21 and its hosted exit are not marked complete by this record.

Scratch and builds remain under `target/execute-plan.6Ro0PC`. The fixture
acquisition helper audited and removed its own `tmp/drift-pins` directory.
The descriptor-preview regression reaps its rustfmt processes and removes its
exact disposable output files/directories. After retaining comparison results,
the real/live output directories were checked for open files, processes,
launchd units, configuration references, symlinks and virtual-environment
dependencies, then removed. Shared build/source caches and the checksum-verified
`tmp/drift-tools/actionlint` remain live for other workflow owners in the parent
goal; that exact tool directory needs cleanup when their checks finish.
