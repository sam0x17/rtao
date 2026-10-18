# CI-only Python oracle

No script here may execute locally, including in local containers. Rust parses
the Python source AST and validates plans/outputs without an interpreter.
Execution requires a designated free Ubuntu GitHub oracle or record lane.

The image checks out `parity.commit` through its exact tag, builds
`bittensor-core-py` as the locked SDK path dependency with maturin, and uses
upstream's frozen `uv.lock`. There is no PyPI core fallback. Rust 1.89/bookworm
and UV 0.8.24 base manifests are pinned by digest. CPython is exactly **3.12.11**;
the build asserts implementation and version and records the compiled binding,
source, toolchain and lock hashes. `vendor/oracle/runtime-lock.json` records these
runtime pins separately from the pin generator's byte-defining Cargo package
lock. Build backends and all their dependencies use exact versions and verified
wheel hashes from `build-requirements.txt`; isolated PEP517 resolution is disabled.
Image identity asserts those versions and records the requirements hash.
Python 3.12's float `sum` uses compensation; `accumulate` remains a left
fold. The conform corpus covers that distinction, ties, clipping and a valid
raw-u16 row of 2,500 UIDs.

`cargo xtask oracle inputs` parses every script with Rust and emits a context
hash and image tag. `oracle image --plan` prints the Docker invocation; actual
image builds/pushes are rejected outside hosted CI. `oracle dump` requires a GHCR
digest or the immutable image ID from that hosted build, checks source/context
labels, disables the container network, and writes to fresh task scratch. Image
tags include both parity tag and context hash. A failed image/dump leaves the
previous committed vectors authoritative.

The default `M2` scope dumps all nine intents and fifteen ported reads (the
thirteen scoped reads plus `metagraph` and `token_symbols`). The `all` scope requires
registry/sample equality and dumps the full registry. Intent dumps contain call
trees, schemas, preflights, plans, fee/spend/extras and the complete seeded state.
Timelock ciphertext/reveal-round fields carry explicit exclusions. Read dumps
use upstream's `_jsonable` at head and block 90 and record every storage,
runtime-API and constant touch. Each sample has a seed file. Reviewed branch
samples extend `samples_extra.json`. The reviewed inputs include eighteen intent
branches and fifteen read branches. The `M2` scope selects twelve intent
extras for 21 intent cases and 60 head/pinned read cases. The six root-origin amount extras require
`all` scope, bringing that scope to 101 intent cases. Ordinary Rust input checks
build and plan them without recording Python results; expected Python outputs
are produced only by optional hosted upstream execution.

Each extra has `id`, `review`, and `args` (intent) or `params` (read).
Optional `storage` entries contain `container`, `name`, `params`, and `value`;
`storage_defaults`, `storage_maps`, `constants`, and `runtime` entries contain
`container`, `name`, and `value`. A map value is a list of key/value pairs.
The same seed application code handles both kinds, including constant overrides.
Every vector records its complete pre-execution FakeSubstrate state.

`oracle --check` is Rust-only. It checks context/source/interpreter pins,
manifest hashes, scope coverage, native-u64 balance behavior, error
classification, committed extra-sample identity and exact vector/seed inventory.
It runs the native intent/read adapters on every case. Read hits must be subsets
of H and their sample union must equal H on both implementations. H comes from
the authenticated helper-aware `extracted/read_hits.json` plus reviewed entries
in `parity/read_hits_extra.toml` with this form:

```toml
[read.block_time]
hits = [{kind = "constant", container = "Aura", name = "SlotDuration"}]
```

Missing helper hits or branch samples fail this check. Source extraction is
rechecked before using H. Native intent replay compares schema, constructor
JSON, SCALE call bytes, build extras, preflight, plan, fee, spend and touches.
Only the documented ciphertext/reveal-round fields are masked. Native reads
compare exact result JSON, touch sets and the selected hash on state requests.
Pure checks cover Balance, error messages, production RPC classification,
chain-error JSON, conform and Quicknet arithmetic/UTC times. Comparisons use
`rtao_primitives::json::compare`, which preserves integer tokens exactly,
compares non-integers as f64, and rejects integer/noninteger equivalence.

The pure dump also writes `pure/hyperparams.json` in both scopes. Its schema-1,
recipe-1 native-codec corpus runs all ten hyperparameter methods across all 48
table entries, an unknown name, and seven controlled precedence fixtures.
The fixtures use upstream methods with temporary input table entries to force
metadata units over hand units, explicit codecs over metadata, signed codecs,
signed-without-bits behavior, unknown identifiers, missing storage and missing
hand semantics. The original upstream tables are restored after the fixture
dump. Source hashes bind the result to the pinned hyperparameter/settings code;
the recorded 42 storage identities bind the catalog input table as well.

Every name has 129 input cases: 32 integer anchors, six semantic bound neighbors,
two booleans, 22 binary64 anchors, 27 half-integer division cases with adjacent
floats, six human bound neighbors and 34 text inputs. Bounds include native u64
TAO amounts and u128 fixed-point settings. Oversized raw inputs remain exact
integer JSON tokens for rejection diagnostics; they never widen balances.
Text cases cover raw/human distinctions, malformed input, Unicode whitespace
and digits, boolean spelling and exponent overflow. `normalized` receives
numeric inputs; `annotate` and `to_raw` also receive text. Float descriptors and
normalized outputs carry u64 IEEE-754 bits, including signed zero, NaN and
infinity, so the corpus never writes nonfinite JSON numbers.

Rust parses the literal input recipe from `dump_pure.py`, rebuilds every bound
and neighboring float input, and requires exact one-to-one coverage and order.
It replays all results, including exact error type/message JSON. Missing,
duplicated, reordered or changed inputs fail before replay. These native recipe
tests produce no Python expected outputs. Hosted differential acceptance stays
pending until a real designated Ubuntu image build, dump and `oracle --check`
pass and their authenticated artifacts are adopted. No local Python or container
run establishes that acceptance.

`oracle.yml` builds and checks in the designated lane. Its dry-run builds a
local image on the disposable hosted runner and performs the same dumps/checks,
without pushing GHCR or creating a PR. Outputs and build metadata remain Actions
artifacts for review/adoption. Hosted execution and actual vectors have not yet
been produced. The ordinary Rust tests validate reviewed input encodability,
intent builds/plans and read dispatch at two heights; they supply no expected
Python outputs and do not establish hosted differential evidence.
The optional `record` dispatch starts a separate owned localnet only after the
GHCR image has been pushed. It checks drand egress, waits for initialized
metadata and a drand round, and retries once with a fresh container on readiness
failure. Only the exact labeled container created by that job is removed.

`cargo xtask record --plan --image <GHCR-digest> --localnet <container-id>` prints
the upstream recorder wrapper invocation. Actual recording requires the
designated Ubuntu record lane and a running container labeled
`io.rtao.localnet=recording`. It joins that exact container's network namespace,
preserving upstream's localhost guard for its real `//Alice` transfer. It filters
absent `*_CASES` entries against live metadata, lists every omission in
`dropped.json`, runs both upstream recorders, and retains the upstream fixture
formats in fresh scratch. It never overwrites committed recordings. Localnet
provisioning and current-spec replay still need hosted execution.
Both execution commands require `RUNNER_ENVIRONMENT=github-hosted`; the shell
provisioner also checks the real Linux host before invoking any container.
