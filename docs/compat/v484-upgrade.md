# Finalized v484 runtime and Python parity upgrade

The parity pin advances from v482 (`8cf229f8ef969389d054a4c161b53ffd164a041f`)
to v484 (`5749dbfbe4a78ccd3938d46ffae45e059488feb7`). The core remains v478
(`850f2755bb05a3851f948489f0ab27b0645cebce`) with patch
`5f2477f5290f5f4cb467524754cc025626b52974`: its core-input fingerprint remains
`b3:edc9f51ac0953af2b9b60665398e0146567ae4ca9988d7f018d3788f01a73b86`.
No recovery or journal facility is removed or replaced.

## Public provenance

The independent finalized Finney observation was block 9,364,327,
`0x95589f228de6c1e32a6b355605b39aa4792f8f1952a571456d23ebb1f2b146c9`.
Candidate preparation pinned the later finalized block **9,364,341**,
`0x0c856f3c0a470a23703858e652949a726369549288f5b1b1917897084d77e7a7`.
Both report spec 484 and code hash
`0x23041f50d3529b359e9be3b90f3ca40474678e834e97f49fb401d0c691097760`.
The published [v484 release](https://github.com/RaoFoundation/subtensor/releases/tag/v484)
is final, published 2026-10-23, and its tag resolves to the new parity commit.
The downloaded `subtensor.wasm` has that exact BLAKE2b-256 hash; its SHA256 is
`8ba7928018328ba9e08d6089b0d92c841967801733ff4cd3c4965f105881e681`.
The upgrade manifest and srtool digest agree with the release/source identity.

The anonymously fetched GHCR manifest for
`ghcr.io/raofoundation/subtensor-localnet:v484` hashes to
`sha256:6cdbb3daa48256895d5b9c2486a9d7b85c3d42f662d6adcd242a10ba149d3b35`.
Its authenticated config identifies linux/amd64 and OCI revision
`5749dbfbe4a78ccd3938d46ffae45e059488feb7`. This proves the registry artifact
identity and its declared revision; it is not an image execution or a
reproducible-build attestation. The old pinned image was
`sha256:6fa0bccdd855b5abd81aa3d08f425b3c4593590104adc7f180959d1657ccebc8`.
Exact metadata, asset and config hashes are in
[v484-provenance.json](v484-provenance.json).

## Implemented behavior and source reconciliation

- Import all 390 authenticated artifacts and retain historical metadata while
  adding finalized V15/V16 metadata. Normal Rust extraction/regeneration
  produces 86 intents and 105 reads, including typed dispatch and namespaces.
- Add `SubnetLifecycleState`, single and map reads, async and blocking aliases,
  and facade exports. New storage takes precedence; absent/invalid new storage
  falls back to legacy cleanup status, queue, registration and first-emission
  state at the same immutable block. Transport/protocol failures propagate.
- Include pending/dissolving subnets in reporting, separately expose pool
  emission policy, and join lifecycle into all four stake/valuation readers.
  CLI JSON includes the new fields; stake list, wallet inspection and overview explain automatic
  payout and unavailable manual unstaking while preserving lock/free notes.
- Metagraph balances use the queried subnet and response's symbol, including empty
  symbol fallback and root TAO, without consulting stale display symbols.
- Raise the single-hotkey root-claim admission envelope from 129 to 130 while
  preserving the charged four-unit allowance and coldkey-wide 256 limit.
  Update guarded quote extraction and inclusive-boundary regressions. The
  imported quotes are adjusted historical spec-467 fixtures, not fresh v484
  runtime fee measurements.
- Import liquid-alpha consensus getter/setter ABI entries and verify argument
  widths. Confirm factory-backed CLI defaults remain fresh per invocation.

[`parity/v484-sources.json`](../../parity/v484-sources.json) authenticates every
watched source against public Git trees and every pinned artifact SHA256.
There are 364 watched sources: 334 unchanged blobs, 28 changed existing files
and two new Python test modules. Each changed path has an explicit native
mapping decision and exact diff hash. Unchanged identity carries only the
previous declared source scope; it does not assert unchanged extraction or
native behavior. Whole generated-output provenance differences remain visible.
The normal parity blessing and producer guards remain required.

The normal porting report finds 30 changed watched paths and both added reads,
with no auto-blessed rows. Extraction differs, beginning with finalized-head
provenance. Its attempted oracle comparison is unavailable: the current finite
root-claim extractor rejects the old fixture function inventory, and no fresh
independent Python batch exists. Explicit source reconciliation does not treat
that report failure as proof of oracle equality.

Runtime RPC changes make dissolving subnets reportable and hide settled
hotkeys behind cleanup cursors. Native consumers preserve those runtime API
rows; the client does not reimplement payout/settlement or filter them using
`NetworksAdded`. Runtime wire layouts remain guarded by regenerated metadata.
Older v482 reconciliation limitations remain historical evidence, not newly
validated behavior.

## Maintenance decision

Use the existing `pin --prepare-candidate` scratch-source path for local pin
preparation. It now verifies core-patch tables, lock entries, source ancestry,
reviewed tree/diff and security fixes without starting hosted API probes.
An explicit `pin --check --source-only` exposes the same bounded check and
prints that the patched/unpatched build probes were not executed. It cannot
be combined with release resolution, network refresh or other pipeline modes.
Ordinary `pin --check` and the full pipeline still require the hosted guard
and both build probes. A changed core base still requires its existing rebase
and review path. This separates source preparation from full validation and
does not create release or compatibility evidence.

## Validation and remaining gaps

Local checks and their exact result summary are recorded in
[`parity/v484-validation.json`](../../parity/v484-validation.json). These are
scoped development checks. Runtime tests (70), read tests (137), the affected CLI
and claim suites, EVM routing/ABI tests and blocking-facade checks pass, along
with strict parity, source-only pins, import/extraction/regeneration, compatibility
and scoped Clippy. Client units pass 193 tests; one existing Unix socket fixture
cannot bind because the required task temporary path exceeds `SUN_LEN`. The
hosted-only core-patch fixture correctly refuses local execution; its three
local tests pass. These two local limitations remain recorded, with no guard
bypass. Free GitHub `ubuntu-latest` remains authoritative
for full validation and was not invoked. No image/container, Python oracle,
localnet e2e, device, soak or fuzz execution is claimed. There is no fresh
independent Python output comparison, no live dissolution/payout evidence and
no hosted patched/unpatched API probe result. Lifecycle migration and netuid reuse,
cleanup-cursor suppression of settled stake, actual admission/fee/refund behavior,
and EVM gas/proxy/owner authorization enforcement remain runtime-owned and
unverified by these client fixtures. The build-weight refresh measures only `cargo tree` for the existing target,
records precommit working-tree input hashes, and does not execute a target binary.
Compatibility must retain a
null validated spec range and empty runtime attestations/reports. No new
upstream provenance gap prevents source implementation; execution evidence
remains the concrete outstanding gap. No upstream message or filing was sent.
