# M2-22 pin pipeline preparation

## Current source and scope

M2-22 covers reviewed source/pin/extraction/regeneration/parity maintenance and
applicable hosted guards. Q30 defers forced full-pipeline replay, bot deployment
and scheduled upkeep. Oracle/recording stages are optional targeted hosted work.
The historical complete-pipeline design below is not a development checklist
or a request to provision a candidate, node, image, toolchain or private cache.

The [pin-bump](../../.github/deferred-workflows/pin-bump.yml),
[upstream-next](../../.github/deferred-workflows/upstream-next.yml) and
[nightly](../../.github/deferred-workflows/nightly.yml) workflows are inactive
drafts outside `.github/workflows`; only ordinary `pr.yml` is active. No poll,
schedule, issue publication or PR creation is required. Preserve adoption
recovery journals and durable state. M2-S02 remains open/unaccepted and M2 exit
remains open.

## Historical preparation and validation

`cargo xtask pin --parity <vN|auto>` prepares a candidate under
`RTAO_TEMP_ROOT`. The original workspace supplies the parity-report baseline.
The candidate includes current source edits and its own build directory.

The pipeline verifies finalized release and image provenance, re-vendors the
watched tree, extracts and regenerates outputs, builds and executes the oracle,
records the pinned localnet, and replays recordings through Rust. A changed
core fingerprint rewrites dependency declarations and snippets, updates the
shared lock, and regenerates the core surface. An active core patch holds the
candidate with `core-patch-conflict` until its rebase is available. From M7,
the presence of Ledger pins also requires successful transcript refresh.

The parity report runs against the original workspace with auto-bless enabled.
All native gates, pinned fast/slow localnet e2e, and core-oracle tests must pass
before adoption. Python execution requires the existing designated hosted
Ubuntu oracle or recording lane. Local execution refuses before creating a
candidate or invoking Docker. No local Python or Python container was run.

Historical command forms (execution is not required under Q30):

```sh
cargo xtask pin --force --plan
cargo xtask pin --parity v478 --force --dry-run
cargo xtask pin --parity auto --check --resolve-only --github-output "$GITHUB_OUTPUT"
cargo xtask upstream-next --branch main --plan
cargo xtask upstream-next --branch testnet --out "$RTAO_TEMP_ROOT/next.json"
```

`--force` uses the committed tag, metadata, finalized head, and image digests.
It reruns the candidate pipeline and requires identical source-file bytes and
inventory plus an explicitly empty porting report. `--plan` prints the stages
without network access or Python execution. `--resolve-only` checks release
provenance and produces polling outputs without rewriting source pins.

Fresh oracle payload hashes must match the committed hashes during replay.
The authenticated execution image is retained as separate build evidence.
Fresh localnet recordings carry new block hashes and era anchors; Rust replays
both the fresh recordings and the original immutable snapshot. Replay retains
the snapshot's committed bytes and saves fresh authenticated outputs separately.

Adoption checks that the original source inventory and hashes still match its
snapshot. Every changed file is journaled before replacement. A failed write
restores old bytes; a recovery journal is retained on an adoption error.
Successful runs retain a porting report, status JSON, and an archive of verified
oracle and current-spec recording outputs. Candidate processes and Docker
mounts are checked before exact scratch removal.

Verified oracle output is archived before later gates run. Fresh recordings
are archived after their Rust replay. A later gate failure therefore preserves
useful authenticated evidence while holding adoption.

The archived `pin-bump.yml` design targets free `ubuntu-latest`, serializes a
30-minute poll, and
skips releases already under review. Scheduled upkeep is off until
`RTAO_UPKEEP=on`; manual dispatch defaults to dry-run. PR publication requires
validated outputs, upkeep enabled, and a live dispatch. It never applies the
`CI` label or requests the restricted review team.

The archived `upstream-next.yml` design compares immutable `main` and `testnet`
revisions nightly. No schedule is active.
Its Rust helper joins changed blobs to the parity map, normalizes catalog
source locations, and compares the core-input fingerprint. It preserves
release pins and produces an informational issue when changes are present.
The archived core-canary lane design refreshes its disposable main image digest, requires
the published image's revision and source labels to match resolved upstream
main, and runs the Rust core-oracle smoke. The localnet owner wired it into
`nightly.yml`; the helper removes its exact owned container and retains bounded
logs under the task root.

Validation and remaining evidence:

- Both new workflows parse as YAML and use `ubuntu-latest` exclusively. Both
  shell scripts pass `bash -n`.
- Nine existing golden/corpus/typed-call tests pass with their original
  vendored inputs. New recordings use `RTAO_RECORDED_ROOT`; corpus replay
  requires complete registry coverage and exact decoded values. Dropped
  call/storage vectors must account for their original recorder inventory.
- The eighteen pin/metadata/network tests, pipeline safety tests, and catalog
  normalization test pass on current Rust and Rust 1.89. Strict Clippy passes
  for xtask and the three recorder replay test binaries. The added filesystem
  regression verifies symlink ancestors cannot overwrite unrelated data.
- Native CLI plan output parses as JSON. A local `pin --force` refuses before
  any candidate/container execution, and the upstream pin file stays identical.
- Read-only previews of both main and testnet resolved to
  `850f2755bb05a3851f948489f0ab27b0645cebce`, matching v478 and its core
  fingerprint. They reported no watched, catalog, or extrinsic-codec changes;
  production pins stayed identical.
- Fresh hosted oracle/recording outputs, their current-spec native replay,
  fast/slow e2e, Python interop, and successful `--force` byte reproducibility
  were not executed by that preparation. They are optional/deferred under Q30.
  The historical literal BLS failure is superseded by approved Q26
  [built-dependency checks](bls-lock.md). M2-22 source completion requires
  applicable hosted source checks; the M2 security exit remains open.
