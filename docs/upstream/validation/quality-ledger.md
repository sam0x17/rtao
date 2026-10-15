# Independent review of the Ledger filings

This audit examines [subtensor PR #3242](https://github.com/RaoFoundation/subtensor/pull/3242)
at `eafe84e8a97376e7902118ebb204b9c53e3df55e`, and [Zondax issue #295](https://github.com/Zondax/ledger-polkadot/issues/295)
and [PR #296](https://github.com/Zondax/ledger-polkadot/pull/296) at its original
`9edd5fa5f9e609046e6bdd0f5e77239a7cf6f20d` and corrected
`8645ebdc31d188a722e22396d26fb52144246da4` heads. The posted bodies and exact
diffs were read independently on 2026-10-14. Results below describe completed
checks.

The original `TaoBalance` display problem is accurate. The corrected Zondax PR
formats native amounts and preserves price fields as raw integers, fixing the
wrong ratio unit introduced by its original commit. The correction is published
and its committed head passes all 5604 native tests. Its version bump and device
snapshots remain unfinished and are disclosed in the updated PR body.

## Subtensor transport injection

The exact diff changes only `sdk/bittensor-core/src/signers/{mod,hid,ledger}.rs`.
`ApduTransport: Send + Sync` is public and ungated, while `LedgerDevice` stays
behind `ledger`. `open()` still calls the same HID opener and `exchange()` still
locks the same `HidDevice`, writes the same bytes and reads the same response.
Only the trait dispatch and allocation of the transport box are added. Request
building, 250-byte chunking and status handling share the same implementation
for injected transports and HID. The short-response diagnostic changes from
"HID response was too short" to "APDU response was too short", as stated.

The three submitted tests assert automatic `Send + Sync`, exact INIT/ADD/LAST
requests for 300 payload bytes plus 200 proof bytes, and the existing refusal
message. The PR explicitly distinguishes its Rust tests from the older v477
prototype emulator run and explicitly says it has no physical-device run or full
monorepo preflight. No finding contradicts those scope statements.

Fresh Rust **1.89.0 and 1.99.0** runs each pass **74 library tests with 1
existing ignore**, plus **3 independent public-API/generator tests**. They use
a unique `RUSTC_WORKSPACE_WRAPPER` at
`upstream-quality/wrappers/pr-3242.sh`, `--locked --offline`, the shared recorded
task target, and `PYO3_NO_PYTHON=1`. These final runs supersede the initial
unwrapped counts after shared-target foreign test-binary reuse was discovered
elsewhere. The verbose compiler logs identify the exact PR checkout and wrapper;
the MSRV library binary is `bittensor_core-8ea32f600bb7592a`, and the current
library binary is `bittensor_core-2d98ea2d24d43ac8`. Both compilers regenerate
byte-identical relative-price fixtures with the SHA256 recorded below.

The additional tests exercise the public API: short APDU/status responses and
invalid addresses; sign chunk
boundaries at payload sizes 1, 249, 250, 251, 499, 500, 501 and 65535; rejection
of a 65536-byte payload before any transport exchange; and the ratio fixture
generator. The submitted five signer tests are included in the library run.

The HID exchange body is byte-identical before and after, with SHA256
`06cbe151a8fff8b9fdbd7d441ddb832de0ab600839111ca08e0a2f194ca9b58a`;
the unchanged framing helpers hash to
`bdff5757e2615091c719bf2937bb663457b83e1660a1ce68328e89e1f93dc8dd`.

## Native C++ reproduction of the Zondax fix

The exact PR head was built on macOS arm64 using Apple Clang 21.0.0, CMake 4.4.4,
the repository's fmt 7.1.2 and GTest 1.14.0 pins, and nlohmann_json 3.12.0. The
build used Unix Makefiles and Debug mode, with `CMAKE_POLICY_VERSION_MINIMUM=3.5`
to accommodate the old pinned dependencies on current CMake. No app production
source or fixture was changed for this run. No container, emulator or SDK/test
Python was executed. However, a later log audit found that Hunter's dependency
installer automatically discovered local Python and ran its `link-all.py`
helper; GTest configuration also discovered the interpreter. This violated the
session's prohibition on local Python. The audit did not inspect this downloaded
helper before configuration. Before cleanup, its task-local discovery script
was changed to fail before any interpreter probe and select Hunter's shell
fallback. The subsequent direct native builds execute no Hunter helper. This is
recorded as a workflow violation; the test results are still the actual native
C/C++ results, and must not be described as a Python-free build.

| Source under test | Fresh result |
|---|---|
| Exact submitted PR head | **5603 passed, 0 failed** |
| Same tree and fixtures, with only the added `TaoBalance` allowlist entry removed | **5597 passed, 6 failed** |

The six failures are exactly vectors 1 (`add_stake`), 2 (`add_stake_limit`) and 4
(`Utility.batch`), each in normal and expert mode. Only the four `TaoBalance`
display lines differ in each mode. The existing `Balances.transfer_keep_alive`
and raw `AlphaBalance` vectors remain passing. These fresh results reproduce the
counts and concrete before/after claim in the submitted PR body.

## Relative-price defect and conservative correction

Subtensor's dispatches use `TaoBalance` for both native amounts and
`limit_price`. For `move_stake_limit` and `swap_stake_limit`,
`pallets/subtensor/src/staking/move_stake.rs:get_max_amount_move` divides the
input by `1_000_000_000` and compares it to
`current_alpha_price(origin) / current_alpha_price(destination)`. It is a
relative price between subnet tokens. The TAO symbol is incorrect there. The
issue and PR both disclose this accurately; no claim of correctness should
omit this effect.

The original PR body stated that `checkBalanceEncoding` "only has the field's
type name ... to go on", but `Field_t` also supplies the field name. The
function cannot distinguish the five `limit_price` calls from their shared
field/type pair and existing pallet context, but it can distinguish price
fields from amount fields. The published PR body now describes both inputs and
the conservative guard accurately.

The app's `checkBalanceEncoding` can inspect the current field name and type
name. Its `PrintItem_t` has a pallet name but no call name. Consequently a
conservative fix can keep every field named `limit_price` and typed
`TaoBalance` raw, while formatting native `TaoBalance` amount fields normally.
That avoids introducing a new call-context mechanism and also keeps
add/remove/collateral prices raw. Those latter values represent a price per
alpha rather than a native amount, so raw display avoids implying a TAO-only
unit. The tradeoff is losing decimal formatting on those price fields.

Real signature-payload metadata-proof vectors for direct swap, direct move and a
nested batch were generated using the core API and recorded finney v478 V15
metadata. They use mock account/genesis bytes, immortal era, nonce 7 and no tip;
they are deterministic parser inputs rather than signed extrinsics or evidence
of network acceptance. Metadata SHA256 is
`17cdbed812cb59a833cc64d2764d734aee5d08174385868167a1cf284d6ff502`;
the generated fixture hashes to
`fb2b9845157e0cea1d8c3344dced9146b718ad4b3cdd9399d25bdd74c82854dd`.

The independent C++ regression first **fails** against the exact submitted
head: direct swap, direct move and both calls inside the batch show
`limit_price : TAO 0.95` in normal and expert mode. All three metadata digests
match the Rust-generated digests. The local correction adds the exact
`Field_t.name == limit_price` and `Field_t.typeName == TaoBalance` guard,
documents the exception, and changes only the two price display expectations in
the old spec 477 fixture. The transaction, extension, proof and digest fields
of every original fixture remain byte-identical.

The corrected full C++ suite passes **5604 of 5604 tests**. The final
self-contained relative-price test, which loads its committed fixture path
rather than an environment variable, also passes. All eight ratio screens now
show `limit_price : 950000000`; amount fields still format as TAO and
`AlphaBalance` remains raw. A second independent source review verifies the
exact field/type match, nested call unwinding and unchanged Option behavior.
No runtime metadata or transaction semantics were changed.

After restoring the checkout for root review, a second corrected build also
passes **5604 of 5604 tests** without invoking CMake, Hunter or Python. A direct
native build script reads the CMake app-source inventory and compiles the same
20 existing C sources and 10 C++ app/test/dependency sources with Clang. The
absent `zbuffer.c` entry is skipped exactly as CMake's `GLOB` skips it. It uses
the existing debug/BLAKE3/version definitions and the same fmt 7.1.2, GTest
1.14.0 and nlohmann_json 3.12.0 archives, verified against the original pinned
archive digests. Compiler/link argv and archive hashes are retained in
`quality-ledger/native-command-provenance.json`; the explicit compiler process
list avoids the interpreter discovery/helper behavior of the earlier build.

The corrective follow-up is **published** as
[`8645ebd`](https://github.com/sam0x17/ledger-polkadot/commit/8645ebdc31d188a722e22396d26fb52144246da4)
on the existing `fix/tao-balance` branch. An ordinary fast-forward push advanced
the branch from `9edd5fa`; no force push or hook bypass occurred. Task-owned
pre-commit and pre-push hooks checked the exact six-file scope, formatting,
clean committed checkout, expected parent/fork/branch and `sam0x17` actor.
The pre-push hook rebuilt the committed head directly with native compilers and
passed **5604 of 5604 tests**. Native clang-format 21.1.8 and
`git diff --check` also pass. The formatter's Mach-O binary was extracted from a
checksum-verified distribution archive; no Python wrapper was installed or run.

The actual remote branch and PR head both match the commit above. Both posted
body texts match their retained drafts. PR #296 now explains the field/type
guard, all five price semantics, mock fixture provenance and the scope of fresh
validation; issue #295 narrows its metadata statement to the relevant field
types and describes the corrected price behavior. No label or review-request
mutation was performed. The seven upstream workflow runs for the published head
report `action_required` without job results, including
[Verify PRs to main](https://github.com/Zondax/ledger-polkadot/actions/runs/38592915868).
They have not validated the commit. No workflow approval, rerun or setting
change was attempted; this audit claims the local committed-head checks above.

## Limits and outstanding upstream work

The existing five submitted vectors do not exercise move/swap relative prices
or optional `TaoBalance` fields. The new regression closes the observed ratio
gap on the published branch. Option behavior was checked by source review; no new Option
display fixture was executed.

`app/Makefile.version` remains 100.0.29, matching an existing release tag; the
version-check workflow uses the shared Zondax checker. The posted PR openly
states that the patch version and seven main-menu snapshots are unfinished.
This audit does not fabricate screenshots or claim that a native C++ test
covers the physical Ledger screen, emulator integration, HID hardware or
production installers. The older emulator and historical finney validation
claims are prior reported evidence, not fresh reproduction by this audit.

The issue originally said spec 477 and 478 have identical V15 metadata apart
from the spec version. This audit confirms the relevant amount/price types at both
versions through the executed proofs, but did not compare the two complete
metadata blobs. The upstream tag comparison contains nine commits and leaves
the dispatch/type declarations unchanged; that alone does not prove whole-blob
identity. The posted issue now limits its statement to those relevant field
types.

## Evidence and workspace hygiene

Bounded logs, posted-body snapshots, hashes, fixture provenance, test sources
and the corrective patch are retained under the recorded task root at
`upstream-quality/ledger-audit/` and in [the bounded artifact directory](quality-ledger/).
Initial disposable C++ checkout, build and Hunter directories were cleaned after
process/dependency checks. Root then requested the corrected source for review;
the exact Zondax head was fetched again and the preserved corrective patch
applied. Those restored directories were retained for root review,
committed-head checks and authorized follow-up publication. After publication
and body/head verification, checks found no process/open-file, launchd,
configuration, external symlink, virtual-environment or external Git-worktree
dependency on the checkout, native build, dependency sources or formatter.
The native test binary links only system libc++ and libSystem. The exact owned
`zondax/`, `cpp-pr/`, `native-deps/` and `native-tools/` directories were then
removed. Both sets of isolated C++ proofs and the outgoing commit are retained
in the bounded artifact directory. The shared Rust/Git caches and root-owned
Subtensor checkout remain live and were not removed. No CMake reconfiguration
ran after restoration.

A reconfigure initially omitted `HUNTER_ROOT` and created a 32 MiB
`~/.hunter` cache. Its birth time established that this task created it.
After restoring the recorded root and rebuilding stale dependency files, checks
found no running/open-file, launchd, configuration, external symlink, virtual
environment or live build dependency on that cache. The exact directory was
removed; no unrelated home-directory files were removed. The Python-helper
execution noted above remains recorded as a workflow violation.
