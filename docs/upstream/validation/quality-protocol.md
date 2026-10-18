# Independent audit of submitted portable protocol PRs

Audit date: 2026-10-14. Review covers the actual submitted source and posted
GitHub bodies, independently selected behavioral probes, and their limits.
The new audit tests are disposable probes; they are not submitted PR changes.
The later [Unicode audit](quality-unicode.md) found a required correction in
U1's Python-style formatting, inherited by U2. Its exhaustive native proof
supersedes any inference that the passing tests below establish full Unicode
parity. The reviewed correction is now published, as recorded in
[Unicode publication](quality-unicode-publication.md). This original audit used
no Python interpreter, GitHub write, label or review request.

## Exact submitted commits and dependency scope

| Filing | Submitted head | Scope against v478 |
|---|---|---|
| [#3243](https://github.com/RaoFoundation/subtensor/pull/3243), U15 | `1ac10f67ff3dcf8306d18183471158b8a648cdb4` | 4 files; 474 additions, 90 deletions |
| [#3246](https://github.com/RaoFoundation/subtensor/pull/3246), U1 | `9e926d1f2f4943f11a23ef13ff517e7857e9cc27` | 18 files including U15; 2,299 additions, 292 deletions |
| [#3247](https://github.com/RaoFoundation/subtensor/pull/3247), U2 | `aadeb06bb517b24428f4d7f64a351ccc8765e502` | 21 files including U15/U1; 2,792 additions, 354 deletions |

GitHub's live API and fetched PR refs agree. Their base is exactly
`850f2755bb05a3851f948489f0ab27b0645cebce` (v478). U15's cherry-pick in U1,
`fb8274680b5287d8c4e82e77bfe9506d4946a362`, has an identical core tree to
#3243. U2 contains that commit and the submitted U1 commit unchanged. The
posted U1/U2 dependency descriptions are accurate. All three currently have
`mergeable: true` and `mergeable_state: blocked`; this does not establish a
passing CI result. The visible Vercel status requires authorization, and the
recorded Rust workflow evidence showed startup failures without Rust jobs.

## Source review and selected independent proofs

U15 is an API addition and bounded traversal change. It does not claim a
historical decoding defect. The original seven public client helper exports
remain public re-exports. The portable helpers preserve the original matching,
borrowing, integer and byte conversion semantics, except the explicitly
documented rejection after 256 singleton wrappers. The audit compares an
independently generated carrier corpus against the original helper source,
including duplicate/non-string keys, all carrier types, malformed hex,
integer boundaries, pair combinations and alternating nested wrappers. A
separate 256/257-level probe identifies the intentional compatibility boundary.

U1 replaces the actual blocking-client fee path: the old implementation signs
a mortal extrinsic and calls `payment_queryInfo`; the submitted implementation
calls the shared immortal/zero-signature fee helpers and
`TransactionPaymentApi_query_info`. The audit constructs the signed v4
`System.remark([])` wire bytes manually, including framing, MultiAddress,
signature variant, zero signature, era, compact nonce, zero tip and disabled
metadata mode. RuntimeDispatchInfo responses are manually constructed instead
of round-tripping the implementation's encoder. Amount/class boundaries,
every truncation, trailing bytes and JSON/hex errors are checked. An explicit
read-only Finney probe is prepared to verify acceptance and decoding against
the actual runtime at an immutable finalized block.

U2's central bug statement is correct by source inspection: the original
client checks `attributes.result` or the entire map, whereas the actual Sudo
event field is `sudo_result`. The audit runs the intact original client
beside the submitted public client, through `submit_encoded` and a bounded
local HTTP JSON-RPC server. Both consume the same manually assembled
System.Events SCALE bytes. This verifies original false success, submitted
failure, correct dispatch classification and preserved successful variants
through actual request, inclusion lookup, event decoding and receipt paths.
The exact original client file has SHA256
`6f424c3b05f74ae53543e556b448b6d316350a0b9c38a970d9f836b07f2152a1`;
the probe copy is byte-identical to the v478 Git object. SCALE encoding,
decoding, runtime parsing and key primitives used by the original-client
adapter are unchanged by these PRs.

## Accuracy limits requiring explicit wording

The U2 body's phrase about rejecting malformed required attributes applied
to required event fields and fee/weight/outer failure attributes. The GitHub
body was corrected and its exact resulting text verified in
[receipt-pr-body.md](receipt-pr-body.md), SHA-256
`a029b41f32f84e5eeaab131179a3cbeac40acbaf85699e54cd68664696f16a72`.
Nested
wrapper discovery deliberately mirrors Python and skips absent result
fields. For example, a synthetic `Sudid` attributes map with no
`sudo_result`, followed by outer success, is not rejected. Actual valid
metadata-decoded `Sudid` events contain that field. A probe records this
limitation; the posted wording should be narrowed instead of describing a
general fail-closed validator for arbitrary event carriers.

U1's metadata-defined fee decoding requires V15 runtime API descriptions.
The existing V14 fallback still supports storage/call functionality, but is
not a fee-query fallback for a runtime lacking that API description. The
review does not claim compatibility with every older runtime.

The mock RPC receipt proof is not a live Sudo transaction. No account funding,
submission to a public chain, Ledger action, Python parity execution or hosted
Rust success is inferred from it. The actual Finney fee proof, when run, is
read-only and cannot establish live receipt behavior.

## Actual Finney fee response

The read-only proof passed at the v478 finalized block
`0x02f6debc90eb2ef7623d0a42865708ae707454ffa4048014c270a3f777ee3e7e`,
using `https://entrypoint-finney.opentensor.ai:443`. It used only
`chain_getFinalizedHead`, `state_getRuntimeVersion`, and `state_call`.
The nonce and public key are fixed public test values; no wallet was opened
and no extrinsic was submitted.

The hand-built signed-v4 extrinsic is exactly 109 bytes. Its query arguments
are those bytes followed by `6d000000`, the little-endian u32 length.
`TransactionPaymentApi_query_info` returned:

```text
0x42e9ae0e994100f9a5000000000000
```

An independent SCALE decoder obtained `Weight { ref_time: 61586000,
proof_size: 4198 }`, `Normal`, and `partial_fee: 42489` rao. The raw request,
response, immutable block and runtime version were captured locally; that
capture is not retained in the repository. The Rust audit includes
this captured response directly; it does not encode an output value and then
decode its own bytes.

## Execution status

Source and posted-body review complete. Final Cargo test runs use the task's
unique no-op `RUSTC_WORKSPACE_WRAPPER` scripts under
`upstream-quality/wrappers/`, one for each PR. This prevents the earlier
shared-target artifact reuse observed by another audit. The initial
non-isolated results are excluded from the following certification.

| Check | Rust 1.89.0 | Rust 1.99.0 |
|---|---|---|
| U15 original nine portable tests | pass | pass |
| U15 independent 3,193-carrier comparisons, host exports, 256/257 limit | pass | pass |
| U1 original 17 portable protocol/error/fee tests and nine inherited navigation tests | pass | pass |
| U1 independent wire, raw dispatch-info boundaries and captured actual Finney response | pass | pass |
| U2 original four receipt tests and 26 inherited portable tests | pass | pass |
| U2 independent public-client original/submitted comparison across six wrappers, four valid success forms, native fee fallback/paid precedence/overflow, malformed nested input scope | pass | pass |
| Original all-feature library suite at each of the three heads | 71 pass, one existing GPU test ignored per head | 71 pass, one existing GPU test ignored per head |

Portable `wasm32-unknown-unknown` checks pass at all three heads using the
installed `+1.89` toolchain, compiler `29483883e`. The separate `+1.89.0`
installation has the same compiler but lacks that target; no installation was
performed. Current compiler identity is `b940084d7` (1.99.0).

The final test logs are not retained in the repository. U2's MSRV full script initially exited 101 because the added audit
probe incorrectly assumed current Treasury metadata and its mock startup
deadline was too short under concurrent native certificate discovery. The
final independent receipt log passes both toolchains after using actual
Balances metadata and a bounded idle watchdog. These were probe corrections;
submitted code did not change.

The six library-test artifact identities are distinct. Their dependency
files contain `protocol/receipt.rs` exactly for U2, agreeing with each logged
checkout; 36 artifact/log records and dependency-file hashes were recorded but
are not retained in the repository.

| Filing | MSRV library artifact suffix | Current library artifact suffix |
|---|---|---|
| U15 | `30c4f06976961205` | `fcd41433db2c3ba4` |
| U1 | `412f448c10c98343` | `05e5e6547b1477c9` |
| U2 | `7bd6583483d5a39b` | `2c26463f0911b4ac` |

The complete disposable probes were archived as a 24 KiB archive,
`quality-protocol-probes.tar.gz`, not retained in the repository (SHA256
`7b7c6b446a7496b9243924d66f783ac48865baa53f14959d2348d0b031ebe3f6`).

Strict Clippy recertification passed at all three exact heads. Cargo Clippy overrides
`RUSTC_WORKSPACE_WRAPPER` with its driver, so its shared-target metadata still
collides: the PR U2 integration tests temporarily saw a foreign core lacking
`protocol::receipt` despite the actual U2 source exporting it. The error log
is not retained in the repository. No submitted-source patch
was created to hide this build artifact problem. Real disposable lint targets
were approved at `upstream-quality/lint/pr-{3243,3246,3247}`,
superseding the earlier shared target requirement for these final lint
checks. The pinned 1.89.0 core and binding checks ran in those three targets
with all features/targets and warnings denied, exiting zero for all three
(71 s, 52 s, 52 s). Their log, which recorded the exact source paths,
submitted heads and compiler, is not retained in the repository.
`PYO3_NO_PYTHON=1` was set throughout. An empty task-local `clippy.toml`
stopped discovery of rtao's consumer-only method bans; upstream's workspace
lint policy and `-D warnings` stayed active.

The dependency-aware cleanup audit passed for the three worktrees and three
lint caches: no open files, processes, launchd/configuration references or
virtual environment dependencies. Symlinks remained within their owned trees.
The three source worktrees were then retained until publication/acceptance;
they remained live dependencies for that work. The common repository, wrappers
and main build cache also remain shared dependencies. The three dedicated lint
caches (about 1.7 GiB) and the temporary lint-configuration directory were
audited and removed. The later Unicode publication completed
acceptance for U1/U2 and removed those exact finished worktrees in its separate
dependency-aware cleanup. U15, shared caches and original wrappers remain live.
