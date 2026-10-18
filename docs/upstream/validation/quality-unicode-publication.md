# Unicode 15 corrective publication

Published 2026-10-14 to the two existing PRs after review of the
[Unicode audit](quality-unicode.md) and exact prototype patch. Ordinary
follow-up commits preserve every published ancestor.

| PR | Branch in `sam0x17/subtensor` | Corrected HEAD |
| --- | --- | --- |
| [#3246](https://github.com/RaoFoundation/subtensor/pull/3246) | `sdk-portable-protocol` | `77480991b2b3b39ec828d2da9d7c6ecf31113a81` |
| [#3247](https://github.com/RaoFoundation/subtensor/pull/3247) | `sdk-receipt-nested-failures` | `849c192e9702fa245a6f16941359290786554d85` |

The maintainer is author and committer. GitHub API and SSH both authenticated
`sam0x17`. No force push, rebase, hook bypass, new filing,
maintainer comment, label or review-request change occurred. Only the two
authorized existing PR bodies were updated. Fetching each body again matched
its retained text ([#3246](quality-unicode-publication/pr-3246-body.md),
[#3247](quality-unicode-publication/pr-3247-body.md)); title, fork, branch, base, open state, empty labels and empty
review requests matched the before snapshot.

## Source and dependency boundary

Each correction changes exactly five paths: core Cargo.toml, the formatter,
the generated printable table, the existing portable fee/dispatch test, and
Cargo.lock. The formatter/table/test hashes match the independently reviewed
prototype. The generated table embeds the complete Unicode-DFS-2016 notice;
core declares `Apache-2.0 AND Unicode-DFS-2016`. Scoped offline Cargo checks
canonically removed only the Unicode dependency edge and 1.1.0 package,
seven lines. Both final locks are byte-identical to v478.

The original [U1 patch](../pr-u1.patch) and [U2 patch](../pr-u2.patch) retain
their historical provenance. Separate [U1 correction](../pr-u1-unicode15.patch)
and [U2 correction](../pr-u2-unicode15.patch) patches contain only each new
commit. U2 preserves the original U1 commit rather than rebasing onto the new
U1 head; its equivalent correction follows the original receipt commit.

## Actual outgoing gates

The actual task-local pre-push hook (not retained in the repository) checked origin,
SSH destination, GitHub actor, SSH actor, exact original remote SHA, one
authorized outgoing HEAD/ref, clean tree and expected committed SHA. It invoked
the full scoped outgoing gate, including the exact
reviewed-source, license, lock and stacked-scope guards. The upstream AGENTS.md
full-monorepo/`unarbos` rules were superseded for these SDK follow-ups by the
maintainer's explicit decision to use the `sam0x17` account and Rust-only local
checks.
The full monorepo preflight was not run.

| Check at each corrected HEAD | Rust 1.89.0 | Rust 1.99.0 |
| --- | --- | --- |
| U1 portable: fee/dispatch 5, protocol 13, navigation 9 | 27 pass | 27 pass |
| U2 portable: same suites plus receipt 4 | 31 pass | 31 pass |
| New public formatter test inside those counts | 21 cases pass | 21 cases pass |
| All-feature core library, per checkout | 71 pass, one GPU ignore | 71 pass, one GPU ignore |
| Binding all-feature/all-target compilation | pass | pass |
| Strict core/binding all-feature/all-target Clippy | pass | not claimed |
| No-default portable wasm | pass | pass |

The ignored test is `pow::tests::gpu_mines_and_revalidates_chain_compatible_work`,
which requires physical OpenCL GPUs. The binding defines no Rust unit tests;
its runtime tests remain in CI. Every correction Cargo command set
`PYO3_NO_PYTHON=1`. No Python interpreter, Docker or host-setting change was
used. Pinned strict Clippy retains upstream workspace policy and `-D warnings`;
an empty local Clippy configuration prevents parent consumer-only rules from
being discovered. Existing current-compiler base lints are not reported as passed.

An initial U1 push waited on the shared target until its idle SSH transport
closed. After the portable suite passed, the next owned Cargo process was
audited as idle, with no children and the exact checkout/command, then terminated
under the task's isolation authorization. Its failed push and partial gate logs
were excluded from accepted results and are not retained in the repository. No shared
job was stopped. Both final pushes used temporary per-command SSH keepalives.

Final functional/wasm gates used unique no-op workspace compiler wrappers and
separate disposable targets per checkout. Clippy had a further separate
target because Cargo Clippy overrides the wrapper. There were no target symlinks
or copied artifact overlays. Eight U1 and ten U2 executed artifacts were hashed;
dependency files, resolved workspace-relative source hashes and build logs tie
them to the exclusive checkout. Library dependencies contain the shipping Unicode
table in both branches and receipt code only in U2. The four library executable
identities differ across checkout and compiler.

## Publication and hosted limits

The exact posted body texts are retained under `quality-unicode-publication/`.
The accepted per-PR pre-push, push, publication, results and artifact records,
before/after PR snapshots, fork remote SHA output, source/scope records, stage
logs and startup hook hash snapshots are not retained in the repository. The result recorder's log-hash selection was narrowed during the
final builds to accepted stage logs; it excludes growing outer push logs and
interrupted attempts. The final helper/hook hashes and completed outer-log hashes
were captured separately after execution.

Each new SHA has ten workflows with `startup_failure`; all twenty job endpoints
return zero jobs. Both commits have zero check-runs. Vercel's status is
`Authorization required to deploy.` The run, job, status and check snapshots
establish no hosted Rust or Python result. Hosted Python behavior/generator
tests, runtime oracle replay and live integration remain pending. No forbidden
CI label or subscriber review request was used to retry them.

## Cleanup

The archived independent probes were byte-compared with the existing bounded
archive, audited and removed before committing. The probe cleanup covered
84,172 bytes and recorded the archive hash.

After publication, exact worktrees `upstream-quality/pr-3246`,
`upstream-quality/pr-3247` and scratch root `upstream-quality/unicode-publication`
were audited and removed. The finished cleanup covered 7,054,340,580 bytes,
both final clean HEADs, 2,013 dependency entries, service-manager and
configuration scans. There were zero live dependencies
and zero audit errors. Checks covered processes, open files, inbound symlinks,
virtual-environment files and configuration references before removal.

Shared task build/dependency caches, the bare repository, other review worktrees,
original wrappers and authenticated Unicode sources/proof remain live and were
preserved. No wallets, private data, model assets, miner/validator state, recovery
journals or unrelated work were removed. No rtao integration, manifest, lock,
PLAN, state or global README file was edited by this work.
