# Understanding parity

rtao tracks which pinned upstream sources have been reviewed, which native
behaviors have tests, and which comparisons have independent expected outputs.
Use these records together when deciding whether an API covers your use case.
A source mapping alone does not establish behavioral equivalence.

## Read the evidence

| Evidence | What it establishes | Where to inspect it |
|---|---|---|
| Source reconciliation | A reviewed disposition of an exact upstream file blob and its Rust homes | `parity/map.toml` and the generated [API index](api-index.md) |
| Native test execution | The named assertions passed for the tested commit, features and runner | The exact commit's hosted `pr-gates` results |
| Immutable vector | An expected result was recorded and retained with provenance | `vendor/upstream/golden.json`, `vendor/upstream/corpus.json`, or an authenticated oracle manifest |
| Vector replay | Native output matched the recorded cases during that execution | The corresponding hosted replay result |
| Live runtime comparison | Selected operations were observed against the specified running chain | Separate integration evidence with chain identity and observed results |

The source inventory covers Python, Rust, catalogs, tests and tooling. Its
`kind` explains the disposition: `ported`, `generated`, `extracted`, `vendored`,
`oracle-only` or `n/a`. A reconciled tooling row can have no corresponding
public Rust API. Read its notes before interpreting the Rust home as an
available operation.

`synced_blob = "pending"` means source reconciliation is unfinished. A blob SHA
records the reviewed version; the gate checks it against the authenticated
watched tree. `synced_tag` identifies the pin. A change to the upstream file
requires a new disposition even when its public name stays the same.

The generated `parity/matrix.json` keeps families separate:

- File rows report `pending` or `reconciled` source status.
- Intent and read rows report declared implementation status and separately
  report whether a named oracle vector is `recorded` or `missing`.
- Export rows record aliases or explicit mapping decisions. Those decisions
  help locate APIs and do not certify their behavior.
- Divergence, environment, E2E and chain-assertion rows retain their own
  decisions and coverage limits.

The matrix is an inventory. Its `recorded` value means a vector exists; it does
not assert that a replay passed. Counts from different families cannot be
combined into a percentage of Python compatibility.

## Follow one operation

Start with the generated transaction or query reference to find the native
type, constructor fields, feature and upstream source. Follow the file row in
the [API index](api-index.md), then read the row's notes and the relevant Rust
tests. Consult the [migration guide](migration.md) and rendered
[divergence register](divergences.md) for intentional differences, including
checked native `u64` balances. The [alias index](aliases.md) locates mirrored
item spellings and explicit export decisions.

For a transaction, distinguish a successful plan from a submission and a
decoded receipt. `parity/e2e_map.toml` maps upstream test names to native
counterparts, but those counterparts have different scopes. The matrix's
`evidence.scope`, `runtime` and `runtime_limits` fields preserve the distinction.
A `client-contract` fixture can check call bytes, policy, receipt handling or
follow-up reads while runtime dispatch, storage mutation, rollback and actual
fees remain unverified. A mapped hosted scenario still needs a recorded
execution result.

For a read, inspect both returned values and the state identities it uses.
`RecordingView` records descriptor touches and block hashes, including empty
map scans. Fixtures can catch reads that silently switch blocks. They cannot
establish that a live runtime supplies the same values.

## Recorded fixtures and the Python oracle

The upstream golden and shape corpus are immutable inputs imported from the
pinned source. Their embedded runtime is spec 461. Replaying them checks those
recorded encodings and shapes; it does not establish current spec 478 runtime
execution. Current metadata inputs are described separately in
[compatibility](compat.md).

The independent Python oracle has a separate lifecycle. `vendor/oracle` currently
contains reviewed lock and sample inputs, with no committed output manifest or
intent/read vectors. Independent Python differential parity remains unverified.
Native tests of the input recipes or adapters do not produce Python expected
outputs.

Under PLAN.md Q30, a first oracle refresh is optional hosted work. The source
gate can report `optional-unverified` while retaining that gap. If oracle
outputs exist now or in repository history, the gate requires verification;
the initial-absence policy cannot hide removed evidence. The oracle's source,
interpreter, image, lock, seed and output checks are implemented in
`xtask/src/oracle.rs` and `xtask/src/oracle_readiness.rs`; the procedure is
documented in `ci/oracle/README.md`.

## Keeping a claim current

For a client change, add a bounded regression against the production path and
run it through hosted CI. Record the exact commit and check result. A green
source check establishes that the declared mappings satisfy its rules; retain
the behavior test result as separate evidence. See
[testing your subnet](testing.md) for fixture patterns.

For an upstream pin change, compare authenticated source and extracted
contracts, review affected consumers and divergences, and retain the porting
report. `parity report` can propose cosmetic reconciliation only when its
symbol, extraction and authenticated oracle comparisons support it. Missing
oracle evidence cannot become an unchanged-oracle result. Record unavailable
comparisons explicitly in the [upstream log](upstream-log.md).

No live runtime range is certified by the current compatibility record.
Platform, hardware and live-chain coverage need their own evidence; ordinary
free Ubuntu CI does not supply those results.
