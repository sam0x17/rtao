# Independent review of submitted metadata and key helper changes

Review targets are [subtensor#3249](https://github.com/RaoFoundation/subtensor/pull/3249),
commit `0c4ed7585ae9d232577e53d45813f383e0ec2eec`, and
[subtensor#3250](https://github.com/RaoFoundation/subtensor/pull/3250), commit
`c0208a82ab2a17a91c87654f3d6ebc0dc887cd20`. Both are based on
`850f2755bb05a3851f948489f0ab27b0645cebce`. Their submitted lockfiles have SHA256
`c2008ec9d5e290a65ad4357e4ccdcf7caf2b0b74568a81efddfa9760ded2ca3a` and neither
submission changes a dependency or lockfile. This is an independent review of
the submitted files and posted claims, followed by additional Rust regressions.

## Accuracy of the existing evidence

The posted bodies describe scoped Rust checks and explicitly reserve Python
runtime regressions for hosted CI. Recorded PR-state snapshots identify the
reviewed SHAs; the retained hooks require the clean checkout HEAD to equal the
outgoing commit. Their logs record Rust 1.89 portable tests, 71 passing library tests with
one pre-existing physical-GPU ignore, strict core and binding Clippy and portable
wasm checks. The full monorepo preflight was not run. Python materialization and
binding runtime behavior have not been executed locally. Compilation of PyO3
source with `PYO3_NO_PYTHON=1` establishes its Rust typing, not interpreter runtime
correctness. The recorded hosted workflows failed at startup without test jobs;
they do not establish passing Python tests or full upstream CI.

## Metadata IR review

The original PyO3 `metadata_ir` source and the new portable extraction agree on
legacy fields: specification version, ordered pallets, call names and ordered
argument names/identities, trimmed joined documentation, indexed error names and
documentation, real storage names/value identities, constant names, and runtime
API names/method names. New fields are additive. The existing Python transport
explicitly selects the legacy fields when constructing its dataclasses, so it
does not pass new keys into their constructors.

The materializer creates dictionaries and lists, explicitly using integer lists
for storage defaults and constant SCALE bytes. It delegates metadata traversal
to core. Source inspection confirms identical legacy documentation joining,
empty unnamed-field names and pseudo-storage filtering. It does not establish
Python runtime execution; the submitted Python regression remains required.

The core validates type references before rendering them. The iterative visited
set checks composite/variant fields, sequence/array elements, tuple members,
compact payloads, bit store/order types and generic parameters. This preserves
valid cycles and catches dangling references that the identity renderer would
otherwise replace with a fallback. Call/event/error root types must be enums;
storage hasher and extracted key-component counts must agree. Single-hasher
tuple keys remain one encoded key; several hashers use separate tuple components.
These are the checked malformed shapes; the extractor is not a general validator
for every possible semantic inconsistency in metadata.

V15 versions start as unknown. The V16 helper checks trait names, duplicate
traits, the entire trait set and each method-name multiset, then checks all version
conflicts before applying any version. Method order is irrelevant to this version
guard. It does not compare V15/V16 type signatures or establish block pinning;
the public contract requires the caller to obtain both at the same block/spec.

Independent tests prepared for this review compare every legacy field across the
entire pinned golden fixture, check storage and extension identity preservation,
exercise every graph-edge category including both bit-sequence references and
generic parameters, cover valid recursion plus a second malformed edge, cover
atomic V16 rejection and method reordering, and parse a one-hasher tuple key and
plain storage through the actual metadata parser.

## Key helpers review and correction required

The strict verifier calls the same private `verify_with_crypto` used by the
existing public verifier. It decodes the same SS58 public key, preserves the
existing scheme/length errors and omits only the second verification attempt on
`<Bytes>message</Bytes>`. The binding delegates to it and uses the existing
CoreError-to-Python conversion. The added Python exports are registered in the
native module and forwarded through `bittensor.sp_core`.

The pure password codec repeats the old UTF-8 bytewise XOR and standard base64
format, preserves trimming and exact empty-key/base64/UTF-8 error messages, and
contains no environment reads or writes. The host get/save wrappers retain their
compatibility API, including the environment mutation in save. Independent tests
compare the verifier directly with pinned `sp_core` algorithms, compare encoding
against the original index-based XOR loop over varied Unicode/password lengths,
and use a Rust child process to establish preservation of a pre-existing
environment value without mutating the parent process environment.

**The submitted U10 decoder does not guard partial base64 output on failure.**
At `sdk/bittensor-core/src/keyfile_env.rs:30`, it constructs `Zeroizing` only after
`STANDARD.decode(...)` succeeds. Base64 0.22.1's `engine/mod.rs:244` creates a
regular Vec, writes decoded bytes into it, then propagates a decoding error before
returning the Vec. Malformed data after several valid decoding blocks can leave
reversible XOR-encoded password bytes in that allocation when it is freed.
These bytes are not direct plaintext; the public environment-variable name is
enough to reverse them. The base64 dependency makes no zeroization guarantee.
The equivalent staged rtao helper has the same source flaw.

The correction installs a `Zeroizing<Vec<u8>>` before decoding and reserves
`base64::decoded_len_estimate(trimmed.len())` up front, then uses `decode_vec`.
The public sizing function and STANDARD decoder use the same conservative bound,
so decoding does not reallocate after writing sensitive bytes. The guard owns the
allocation on success and failure. Zeroize's Vec implementation wipes initialized
elements and spare capacity on drop. Corrected-source compatibility tests passed
on both toolchains as recorded below; publication requires its own committed-HEAD
gate and remote confirmation.

The host wrappers also own an unguarded environment-read String and encoded String
in the submitted version. The local correction guards those temporary copies and preserves their API and
the intentionally stored environment value. Returned plaintext Strings, caller
inputs and the persistent process environment remain caller-owned; this change
does not promise to wipe all dependency or interpreter allocations.

## Additional execution evidence

One interim run reused a core-library test executable from another worktree in
the shared target directory: it reported 74 passed instead of this source's 71,
including three Ledger transport tests absent from U9. That result is excluded
from the reviewed-HEAD proof. Cargo's native build lock prevents simultaneous
writes but does not distinguish these workspace artifacts across checkouts.
Final reruns use a separate no-op `RUSTC_WORKSPACE_WRAPPER` path for each review
worktree. Cargo's documented wrapper mechanism changes the workspace artifact
filename hash while the script simply executes the supplied compiler command.
Registry dependency caches and the shared target root remain in use. The new
artifact filenames differ from the interim ones; each toolchain again reports
the expected 71 library passes and one existing ignore for U9.

An earlier Clippy attempt found the ancestor rtao policy because the temporary
override directory contained no Clippy file. Upstream checks now use an explicit
empty `clippy.toml` in the task directory; the staged rtao checks restore the
repository's actual policy. No global configuration changed. The failed and
interrupted runs are retained as diagnostic evidence, not passing validation.

The isolated U9 and corrected U10 MSRV strict Clippy checks pass. Stable Rust 1.99 strict Clippy
fails with its new `chunks_exact_to_as_chunks` lint at
`sdk/bittensor-core/src/keys/base58.rs:44` and
`sdk/bittensor-core/src/signers/ledger.rs:190`. Both files are byte-identical to
the submitted base. Their SHA256 values are respectively
`62efdbc2fdb0f2687d12bf9181c35236f1027513df4ed5b3ba069af64962cba3` and
`3d04824dd663f6b79985f2af1c22b047b4ca1ca80b6c1ee000624157f29d18cc`.
The posted bodies claim strict Clippy on the pinned Rust 1.89 toolchain; that
claim remains accurate. No source change or lint suppression is included in these
PRs for the unrelated current-toolchain findings.

The unchanged submitted production sources pass the following independent rerun
on Rust 1.89.0 and stable 1.99.0, with locked offline dependencies:

| Target | Submitted regressions | Independent regressions | Result on each toolchain |
| --- | ---: | ---: | --- |
| U9 metadata IR | 8 | 5 | 13 passed |
| U10 password/strict verifier | 6 | 5 | 11 passed |

The U10 independent suite demonstrates partial reversible XOR bytes after a late
base64 error and establishes that guarded preallocation avoids buffer growth for
both successful and failed decoding. This proves the dependency behavior behind
the identified ownership flaw; it does not inspect freed allocator memory.
The direct pinned-algorithm, full-fixture legacy projection and child-environment
checks all pass. Original production sources and lockfiles were unchanged during
this phase. Sources and complete output are retained in
`quality-ir-independent.rs`, `quality-keys-independent.rs`,
`quality-ir-keys-submitted.sh` and `quality-ir-keys-submitted.log`.

The U10 correction is retained in `quality-u10-error-path.patch`. The same
decoder correction and two compatibility/late-error tests are applied to
`crates/rtao-core-ext/src/keyfile_env.rs` and
`crates/rtao-core-ext/tests/password_environment.rs`.

| Final isolated check | Rust 1.89 | Rust 1.99 |
| --- | --- | --- |
| Corrected U10: eight submitted and five independent portable regressions | 13 passed | 13 passed |
| U9 and corrected U10: all-feature core library | 71 passed, one existing GPU ignore | 71 passed, one existing GPU ignore |
| U9 and corrected U10: no-default portable wasm | passed | passed |
| U9 and corrected U10: strict all-feature/all-target core and binding Clippy in separate checkout targets | passed | two unchanged-base style lints |
| Staged rtao password correction | two passed; strict Clippy passed | two passed; strict Clippy passed |

`quality-ir-keys-isolated-first.log`, `quality-ir-keys-isolated-final.log`,
`quality-ir-clippy-msrv.log` and `quality-keys-clippy-msrv.log` retain the execution
output. The separate-target Clippy runs replace the earlier shared-target Clippy
result. Formatting, diff and unchanged-lock checks passed.

The corrected upstream source is published as
`64f152725dda64c040e8f3c6435c08f450093a59` on `sdk-pure-key-helpers`. Its ordinary
push to `sam0x17/subtensor` passed `quality-u10-pre-push-hook.sh` and
`quality-u10-outgoing-gate.sh` with exit zero. Remote branch and PR HEAD were confirmed
at that exact SHA. The independent injected audit source was archived and removed
before committing. The outgoing gate required clean HEAD, submitted-base ancestry,
exactly the three correction paths and an unchanged lockfile. Committed-HEAD proof
passed eight portable tests and 71 all-feature library tests with one existing
GPU ignore on each toolchain, pinned-MSRV strict isolated Clippy, and both portable
wasm checks. Formatting and final clean-tree checks passed. Functional checks used
a commit-specific compiler wrapper and Clippy used its separate checkout target.

The existing PR body was updated from `quality-u10-correction-body.md`; fetching
it again confirmed byte-for-byte equality. The title, branch, owner, open state,
empty labels and empty review-request list matched the before snapshot. No force,
hook bypass, persistent Git configuration or additional PR was used. Bounded
publication evidence is retained in `quality-u10-correction-pre-push.log`,
`quality-u10-correction-push.log`, `quality-u10-correction-state.json`,
`quality-u10-correction-remote.txt` and `quality-u10-correction-runs.json`.

All ten hosted workflows for the corrected SHA report `startup_failure`; each
jobs endpoint reports zero jobs. The Vercel status reports an authorization
failure. These establish no hosted Rust or Python test result; no CI-label or
review-request action was taken to retry them.

No Python interpreter, label or review-request action has been used for this
independent audit.

The exact U9/U10 review worktrees and their two Clippy target directories were
audited and removed after publication. No process, open file, GUI/system launchd,
configuration, inbound symlink or virtual-environment dependency was found;
30 symlinks and 51 configuration/environment/Git files were inspected without a
scan error. An unrelated Time Machine SMB mount was unavailable to `lsof`; the
exact local-path queries returned no open file. Removal covered 16,163 regular
files and 1,437,066,258 bytes. `quality-ir-keys-cleanup.json` retains the exact
paths, SHAs, checks and counts. The shared bare repository/build/source caches
and the unique compiler wrappers remain live by root request; other owners'
directories and user state were preserved.

The remaining disposable proof files, task-local hook and cleanup rehearsal were
also removed after archiving their bounded output. That follow-up inspected 12
symlinks and 49 configuration/environment files, with no process, open-file,
launchd or inbound dependency. `quality-ir-keys-scratch-cleanup.json` records
the exact 606,461-byte removal and the retained wrapper/empty-Clippy paths.
The archived `quality-ir-keys-diagnostic-*` runs are preliminary or interrupted
execution evidence and are excluded from accepted validation.
