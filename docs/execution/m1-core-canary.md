# M1-22 disposable core canary

Q30 makes a fresh core-canary run optional targeted hosted work. Current M1
requires source/fixture review and ordinary free `ubuntu-latest` checks, with
no local canary, rehearsal workspace or additional toolchain installation.
The implementation description and validation below record the historical run.

`cargo xtask pin --core-branch main --no-commit` resolves the authenticated
monorepo's `main` branch to a full Git commit. It rewrites every monorepo Git
dependency in the copied root manifest, including the downstream BLS patch, to
that immutable `rev`. It removes both core URL patch tables and `[core.patch]`.
It creates no commit.

The command requires `RTAO_CORE_CANARY_DISPOSABLE=1`. A local workspace must be
strictly inside the canonical `RTAO_TEMP_ROOT`; the original workspace and the
temporary root itself are refused. GitHub Actions clean checkouts satisfy the
alternate hosted scope. The core build script independently enforces the same
scope whenever a canary pin is present.

The released `[core]` and `[parity]` pins remain intact. A separate
`[core.canary]` row records `branch`, immutable `commit`, source `toolchain`, and
source-input `fingerprint`. Generated core constants report the effective
commit, `BITTENSOR_CORE_TAG = "canary:main"`, and
`BITTENSOR_CORE_BRANCH = "main"`. Release builds retain their released tag and
commit and have an empty branch constant. Root's core-surface generator uses
the effective commit for a canary.

After `cargo update --workspace` resolves the disposable lockfile, preparation
requires one monorepo Git BLS entry and immutable revision consistency for all
monorepo lock sources. It also rejects the core-patch fork. Preparation audits
`cargo tree --locked --workspace --all-features --target all --edges normal,build`
against full lock identities, requiring exactly one active BLS implementation at
the canary revision. The parser handles duplicated nodes and procedural macros
and refuses unsupported source forms or ambiguous identities. Cargo metadata's
resolve graph includes a dormant optional registry BLS edge in this workspace;
the activated dependency tree excludes it.

At that historical checkpoint, the core lock integration test still asserted
that the complete raw lockfile contained exactly one BLS package. Canary
preparation did not waive it, and `--no-fail-fast` kept that failure from
suppressing semantic cases. Current pin/BLS checks follow the approved Q26
built-dependency policy in [bls-lock.md](bls-lock.md); this record does not
restore the former literal-lock criterion.

Historical validation used a recorded local task root, with `build` as the
Cargo target and `tmp` for disposable files. The rehearsal copied sources to
a disposable directory there; the original manifest, lockfile, and pin file were
not rewritten by that run. The actual resolved main commit was
`850f2755bb05a3851f948489f0ab27b0645cebce`. The command completed its source and
graph audits successfully.

All five focused tests pass. They cover scope rejection, required CLI mode combinations,
preserved release/parity pins, removal of both patch tables, future monorepo
dependencies, inconsistent revision selectors, dormant versus activated BLS
entries, wrong/fork sources, duplicate tree nodes and procedural-macro markers.
The core provenance regression confirms that a canary cannot claim the retained
release tag. The original core semantic tests ran with failure continuation and
passed; its unchanged raw BLS assertion reports two packages.

Strict xtask all-target Clippy, owned formatting, and Rust 1.89 xtask/core
compilation pass. In the disposable tree, `pin --check`, `extract --check`, and
the effective core-surface generation/compile pass. The retained release
identity check and the effective source fingerprint both use the canonical
production fingerprint. Root's compatibility input reader now checks the
immutable canary selector and records both the effective commit and retained
release provenance.

The final offline CI profile ran with `--no-fail-fast --test-threads 4`:
626 tests ran, 624 passed, two failed, and two were skipped by the existing
profile. All RPC cases passed. The failures were the unchanged raw BLS count
and the new compatibility canary regression's incomplete Markdown fixture
(`missing evidence string tag`). The actual compatibility input test passes
against the canary selector. The root owner has the remaining fixture
diagnostic. No failing case was filtered out.

The rehearsal directory and temporary metadata/tree dumps were removed after
checking running processes, launchd/configuration references, symlinks and virtual
environment dependencies. No live dependency referenced the rehearsal. Bounded
logs were recorded under the parent's former temporary root. That root is
retired and absent; its build, rehearsal and log paths are historical. Preserve
shared global Cargo/source caches and unrelated work. No wallet or private
state was changed.
