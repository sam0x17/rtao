# M3-04 metagraph persistence

Validation below is historical preparation. Current acceptance uses applicable
free `ubuntu-latest` client checks under Q30; former builds, fixture roots and
raw logs are retired. Do not restore them for another local run.

The `rtao-metagraph` `persist` feature provides asynchronous
`persist::save(&Metagraph, path)` and `persist::load(path, &RuntimeCache)`.
Fetched graphs retain their chain and API provenance. Synthetic graphs,
responses naming another subnet and graphs whose public chain identity was
changed after fetching cannot be saved.
The response bytes are preserved exactly, including selective fields that were
not requested; persistence does not serialize the current row model or JSON.
The separately fetched registered and unregistered commitment maps are absent
after loading. Commitment fields present in the raw API response remain intact.

The version-1 contract is `RTMG`, `u8(1)`, a SCALE header, then the unframed
original runtime-API response. The header is ordered as genesis `[u8;32]`,
block hash `[u8;32]`, block number `u64`, runtime spec `u32`, subnet `u16`,
mechanism `u8`, and preset. All numbers use ordinary SCALE fixed-width encoding.
Preset is a SCALE enum: index 0 is `get_metagraph`, index 1 is
`get_mechagraph`, and index 2 is `get_selective_mechagraph` followed by
`Vec<u8>` containing the original field indices in request order. Retaining
this distinction avoids decoding a selective response with the full API's
output type and preserves the mechanism identity omitted by selective outputs.

Save creates an exclusively owned sibling temporary file, writes the header and
response, syncs the file, renames it over the destination and syncs the parent
directory. A failed write or rename removes that exact temporary file. File
operations and response decoding run on blocking workers. Concurrent writers
publish complete files, with the final successful rename choosing the result.

Load validates the magic, version, header, native subnet/mechanism bounds,
known field indices and chain genesis. It resolves metadata with the shared
cache's offline `cached_runtime(spec)` method, then decodes through the original
runtime API and checks response identity and complete byte consumption. It
does not call RPC and never changes its graph input, including corrupt input.
Metadata missing from both memory and the shared disk directory causes an
explicit runtime-cache error. v10 `.pt` pickles are not read.

Validation on 2026-10-14:

- Rust 1.89 passed all seven persistence integration tests. They cover exact
  full response bytes, nonzero full and selective mechanisms, deterministic
  re-save, memory and fresh disk-only cache loads, unselected `None` fields,
  bad magic/version/header/field indices, oversized vector length claims,
  wrong identities, trailing or incomplete raw values, missing metadata,
  input preservation, save rejection, rename-failure cleanup and concurrent
  atomic replacement. Each offline-load test checks that RPC request counts
  remain unchanged after closing the client.
- Rust 1.89 also passed `cargo check -p rtao-metagraph --no-default-features`;
  the persistence module and provenance are entirely feature-gated.
- Rust 1.89 formatting passed for all owned sources and the integration test.
- The final current-toolchain all-feature suite passed all 48 metagraph tests,
  including the seven persistence tests. Strict Clippy passed all metagraph
  targets and features after the final identity checks.

The focused historical MSRV log was recorded at
`target/execute-plan.6Ro0PC/tmp/m3-metagraph-persist-msrv.log`; consolidated
current-toolchain logs were `m3-metagraph-all-features-current.log` and
`m3-metagraph-all-features-clippy.log` in the same `tmp` directory. The first test
run found a selective fixture missing mandatory `netuid`; the file loader
correctly rejected its encoded subnet 0. The fixture now names subnet 1, and
save also rejects responses whose subnet differs from the fetched identity.

Historical fixtures used
`target/execute-plan.6Ro0PC/metagraph-persist-tests` and removed their precisely
owned directories when each test ended. After validation, the empty fixture
container was audited for live processes, launchd/configuration references and
symlinks, then removed with its exact path. It contained no virtual environment
or other live dependency. Q30 cleanup later removed the shared build and tmp
directories and raw logs. No current hosted verification, PLAN checkbox, commit
or milestone exit is claimed by this preparation record.
