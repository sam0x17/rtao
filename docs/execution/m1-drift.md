# M1-21 metadata drift

Current scope is client source and hosted checks under Q30. Ordinary `pr.yml`
checks the retained drift fixtures, pins and generated output. Live acquisition
is optional targeted hosted work; schedules, bot publishing, localnet images and
deployment are deferred. No local node, Docker service, tool installation or
retired build root is required.

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

In a targeted hosted job, `cargo xtask drift --network <network>` reads genesis and one finalized
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

The historical workflow is retained at
[.github/deferred-workflows/drift.yml](../../.github/deferred-workflows/drift.yml).
Its triggers cannot run from that directory. Its former schedule, publishing
and localnet-main branches are deferred reference code, with no requirement to
enable them for M1. A future targeted check needs a reviewed bounded hosted
entry point. Do not apply the `CI` label to trigger or retry upstream work.

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
[Comparison results](m1-drift-results.json) retain the historical semantic report,
independent comparison and three live identity checks. They establish no current
localnet-main or live check. Q30 removes those infrastructure runs as M1
prerequisites; current source/fixture and hosted acceptance remain separate.

The former `target/execute-plan.6Ro0PC` root is retired and absent on the laptop.
Do not recreate it or resume its tools/queued work. Historical helper/preview
cleanup retained bounded results and removed disposable output after dependency
checks. That history creates no current live-cache requirement. Preserve shared
global caches, wallets, recovery material and unrelated work.
