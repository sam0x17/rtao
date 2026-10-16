# M2 oracle preparation

## Current source and scope

M2-05 is optional targeted hosted work under Q30. `vendor/oracle` contains
only the three reviewed generation inputs, with no first manifest, vectors or
current-spec recording corpus. Ordinary and strict client-source parity report
`optional-unverified`; neither mode establishes independent Python behavior.

`xtask/src/oracle_readiness.rs` permits the exact checked M2-14
`Strict client-source parity machinery` row under the explicit Q30 policy.
It still rejects duplicate/malformed status rows, partial or corrupt outputs,
incomplete history and removal of previously committed evidence. Existing
outputs use full authentication and native replay; standalone `oracle --check`
still requires actual artifacts. Source-machinery completion does not complete
strict M8, source/e2e reconciliation or oracle behavioral parity.

The oracle workflow is an inactive [draft](../../.github/deferred-workflows/oracle.yml).
Any future refresh needs a bounded reviewed hosted entry point; no local
container, GHCR deployment or automatic upkeep is a development prerequisite.
M2-S02 remains open/unaccepted and the M2 milestone exit remains open.

## Historical preparation and validation

The image, dumpers, Rust verifier and current-spec recorder wrapper are authored.
Under Q30, a fresh M2-05 oracle refresh is optional targeted hosted work. The
committed `vendor/oracle` directory contains generation inputs only: no first
manifest, vectors or current-spec recordings have been produced. The initial staging policy
reported the oracle as pending and rejected strict mode without a corpus.
The current Q30 optional-unverified policy above supersedes that restriction.

The original staging guard required M2-14 to remain unchecked. It rejected
checked or duplicate milestone rows,
missing plan status, incomplete Git history and any previously committed oracle
artifact. Existing partial or corrupt outputs always invoke the real verifier
and fail rather than becoming an initial-pending state. A regression reads the
actual current plan, so this contract is checked alongside synthetic fixtures.

The implementation and validation notes below are historical. Q30 retired the
local rehearsal campaign and its temporary directories. They do not authorize
new local builds, containers, Python execution or infrastructure setup.

The interpreter is exactly CPython **3.12.11**, asserted during image construction
and execution. `vendor/oracle/runtime-lock.json` records the interpreter, UV
0.8.24, Rust 1.89/bookworm base, UV base and BuildKit digests. It is separate from
the pin generator's byte-defining package `lock.json`, so pin regeneration cannot
silently discard the interpreter pin. Image identity also records source tag/
commit, context hash, compiler, binding hash and upstream Cargo/UV lock hashes.
The SDK core is built from the pinned path dependency with maturin and upstream
Cargo.lock, without a PyPI core fallback. Its six-package build-backend environment
uses exact versions and verified PyPI wheel hashes. PEP517 build isolation is
disabled so the frozen upstream lock cannot conceal floating Maturin/Hatchling
resolution. Image identity checks the installed versions and requirements hash.

M2 scope covers nine intents and all fifteen ported reads (the thirteen M2 reads,
`metagraph` and `token_symbols`). The all scope checks registry/
sample equality and covers the complete registries. Intent results include call
trees, schema, intent JSON, preflight, plan, fee/spend/extras and touched items.
Every sample records the full FakeSubstrate state before execution. Reads use
upstream's `_jsonable` at head and block 90, with storage/runtime/constant touches.
Extra branch samples have a documented seed schema. Twelve reviewed intent
inputs and fifteen reviewed read inputs cover encrypted/raw/clipped weights,
IPv6 serving, transfer call choices, epoch and timestamp boundaries, LastUpdate
absence/negative indices, sparse/duplicate/zero weight rows, and UTF-8 symbols.
There are 21 intent and 60 head/pinned read cases. These are inputs only;
Python expected outputs must come from hosted execution. Ciphertext/reveal-round
exclusions are explicit.

Pure dumps include exactly 10,000 unique native-u64 Balance displays, error and
RPC classification, timelock arithmetic, CLI metadata and conform. The conform
cases include `.5` ties, clipping, invalid values, `[1e16, 1, 1]`, mixed-scale
rows, raw-u16 validation and a valid 2,500-UID raw row. Python 3.12's compensated
`sum` is distinct from `accumulate`'s left fold. Inputs preserve f64 bits; the
Rust verifier replays the parent's weights APIs and compares token kinds and
values with the exact JSON comparator. Mismatches print expected/actual values,
the Python source path and its parity-map row.

`xtask oracle --check` validates context/interpreter/source identity, complete
manifest hashes/scopes and exact sample/seed inventory, then runs the native
adapters for every intent and read. H comes from the helper-aware authenticated
`read_hits.json` plus reviewed TOML extras; Rust recomputes source extraction
before trusting it. Python and Rust touches must agree per case, remain subsets
of H and have sample unions equal to H. Native state requests use the selected
hash at head and block 90. Full recorded seed fields are checked and applied;
constants patch the served metadata, null point entries remain misses, map seeds
retain rows, and the Python LastUpdate zero sentinel becomes an empty SCALE
sequence. Nonempty event/display-symbol seeds require an explicit adapter.
Historical runtime API seeds absent from the recorded metadata remain in the
fake's exact source tables; no bytes are invented for them, and attempting
to invoke such an API still fails metadata lookup. A Rust AST test reads all
24 runtime cases directly from upstream's `seeded_substrate` loop and checks
their native seed conversion without running Python.

Intent replay checks the input schema, constructor/default JSON, call bytes,
preflight, executor plan, fee/spend/extras and touches. The direct hosted test
shares these adapters with the checker. Pure Balance/error/conform comparisons
remain exact. RPC vectors now call the production response decoder and compare
the exception variant, message, policy metadata, discarded block and chain-error
JSON. Quicknet round/time arithmetic uses native constants and native UTC
timestamp conversion. The root task corrected the discarded-state diagnostic
to match the pinned Python archive-node guidance.

The archived `oracle.yml` design targets free Ubuntu. Dry-run builds and checks without GHCR or
PR writes. A live image push uses context and parity tags plus inline registry
cache. Dumps use the immutable hosted image ID or a GHCR digest, check labels and
disable the container network. Outputs remain reviewable Actions artifacts;
committed vectors are never overwritten by these commands. The future upkeep
PR adoption path is not implemented by this preparation.

The optional record job creates an exact labeled pinned localnet, checks drand
egress/readiness, retries once with a fresh container, and removes only that
owned container. The wrapper preserves upstream's localhost guard for its real
`//Alice` transfer, filters absent metadata cases into `dropped.json`, invokes
both upstream recorders and keeps their formats. Rust validates output hashes
and spec/source identity. Current-spec wire/corpus replay needs real hosted
outputs. No existing recordings are replaced.
The static review fixed stale spec selection when compatibility lists both 477
and 478, early container-exit handling in the fresh-node retry, private GHCR
authentication on the separate recording runner, hosted-runner enforcement,
committed extra-sample comparison and record image-manifest equality.

Validation used an isolated Rust rehearsal crate under the recorded task root,
with the production modules copied and only the compile-time root helper pointed
at the real workspace. Strict Clippy and four tests pass on Rust 1.99 and 1.89.
The tests parse all nine Python scripts with Rust, reject unpinned images/unsafe
paths and local execution, and require committed vectors once M2-05 is checked.
No absent vector set was presented as a successful behavioral replay. Read-only
Docker command plans parse without execution. actionlint 1.7.7 passes the oracle
and both release workflows, including ShellCheck 0.11.0 over embedded shell and
the recorder shell script. Those results describe the historical tool snapshot;
no local validation-tool installation or retained task-tool dependency is required.

Root integrated `oracle` and `record` command arms/help. The native replay
follow-up added the xtask TX/signer dependencies and enabled the weights,
serving and transfer intent features, leaving main and the workspace manifest
to the root task. Seven focused integrated oracle tests pass on Rust 1.99 and 1.89,
including the source-derived runtime seeds and reviewed input branches. The
shared direct adapter test also passed two local input checks; its actual
hosted replay remains explicitly ignored until verified artifacts exist.
Strict Clippy passes for all xtask targets on Rust 1.99.
Standalone Null commits now preserve Python's descriptor reads while the
validator's `TimelockCommitter` retains its consensus ciphertext caps. The
dedicated Null read-set regression passes, as do all seven oracle tests after
the production change. The raw Null sample and exact touch checks remain intact.
Any optional adoption of generated vectors still requires authenticated hosted
Python output and its native replay, plus applicable hosted client checks.
Neither producing that first batch nor configuring GHCR is an M2 prerequisite. No signing key, trust anchor or Keychain changed. Both prior release
review findings were fixed: the pinned SemVer checker precedes the dry-run
rehearsal, and live release PRs regenerate compatibility and run gates before
the App-token push.

The rehearsal and downloaded ShellCheck directories were audited for processes,
launchd, configuration, symlink and virtualenv dependencies and removed.
`m2-oracle-inputs.json` retains the bounded historical context identity. The
retired task root and its build/source/tool paths are not current dependencies.
Shared global Cargo/toolchain caches remain preserved; no wallets or private
state were created or removed.

The native replay follow-up created no separate disposable directories. Its
historical shared `target/execute-plan.6Ro0PC` build and temporary directories
were deleted during Q30 cleanup and must not be recreated. No Python interpreter
or local container ran, and no expected vectors were fabricated.

The original Q30 staging correction changed source, documentation and
regressions, with local formatting/source inspection only. Its then-pending
execution status is historical; applicable current results must come from the
exact hosted checkpoint. No temporary directory was needed.
