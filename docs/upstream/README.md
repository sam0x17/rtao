# Filed upstream work

Live GitHub inventory checked 2026-10-14: eight PRs and five issues, all open.
These are actual filings; the remaining PLAN.md contribution rows are planned.

| Type | Repository | Filing |
| --- | --- | --- |
| PR | RaoFoundation/subtensor | [#3242: Injectable Ledger APDU transport](https://github.com/RaoFoundation/subtensor/pull/3242) |
| PR | RaoFoundation/subtensor | [#3243: Portable decoded-value navigation](https://github.com/RaoFoundation/subtensor/pull/3243) |
| PR | RaoFoundation/subtensor | [#3246: Portable protocol helpers and runtime fee queries](https://github.com/RaoFoundation/subtensor/pull/3246) |
| PR | RaoFoundation/subtensor | [#3247: Sudo and nested receipt failures](https://github.com/RaoFoundation/subtensor/pull/3247) |
| PR | RaoFoundation/subtensor | [#3248: Epoch scheduling and commit inclusion phase](https://github.com/RaoFoundation/subtensor/pull/3248) |
| PR | RaoFoundation/subtensor | [#3249: Shared portable/Python metadata IR](https://github.com/RaoFoundation/subtensor/pull/3249) |
| PR | RaoFoundation/subtensor | [#3250: Pure password encoding and strict signature verification](https://github.com/RaoFoundation/subtensor/pull/3250) |
| PR | Zondax/ledger-polkadot | [#296: TaoBalance amount formatting](https://github.com/Zondax/ledger-polkadot/pull/296) |
| Issue | RaoFoundation/subtensor | [#3244: Metadata-visible selective metagraph selectors](https://github.com/RaoFoundation/subtensor/issues/3244) |
| Issue | RaoFoundation/subtensor | [#3245: V15/V16 metadata release assets](https://github.com/RaoFoundation/subtensor/issues/3245) |
| Issue | RaoFoundation/subtensor | [#3251: Ed25519 signing-object erasure](https://github.com/RaoFoundation/subtensor/issues/3251) |
| Issue | RaoFoundation/subtensor | [#3252: Secret URI diagnostics](https://github.com/RaoFoundation/subtensor/issues/3252) |
| Issue | Zondax/ledger-polkadot | [#295: Raw staking-amount display](https://github.com/Zondax/ledger-polkadot/issues/295) |

The independent audit is recorded in
[protocol](validation/quality-protocol.md), [epochs](validation/quality-epoch.md),
[metadata/key helpers](validation/quality-ir-keys.md),
[issues](validation/quality-issues.md), [Ledger](validation/quality-ledger.md)
and [Unicode](validation/quality-unicode.md).
Those records identify exact commits, test commands, artifact isolation,
failures and validation limits. Protocol, epoch and issue checks have final
isolated current/MSRV evidence. The corrected key-helper commit passed Rust
1.89/1.99 tests and portable wasm checks, plus strict Clippy on upstream's pinned
Rust 1.89 toolchain. Rust 1.99 Clippy reports two style lints in unchanged base files.

The audit found three required code corrections: partial password-decoder output
was not wipe-guarded on failure in #3250, #296 applied TAO to relative price
ratios, and #3246's Unicode 16 diagnostics differ from the pinned Python 3.12
Unicode 15 data (also inherited by #3247). The Ledger correction is published
at `8645ebdc31d188a722e22396d26fb52144246da4`; its committed-head native suite
passes all 5,604 tests. Key-helper and Unicode corrections have independent
local proofs. The password correction is published at
`64f152725dda64c040e8f3c6435c08f450093a59`; its remote and PR SHAs matched, and its
updated body was verified. The Unicode follow-ups are published at
`77480991b2b3b39ec828d2da9d7c6ecf31113a81` (#3246) and
`849c192e9702fa245a6f16941359290786554d85` (#3247). Remote and PR SHAs and
the corrected bodies were verified. Their complete outgoing gates pass
current/MSRV native tests, binding compilation, portable wasm and upstream's
pinned strict Clippy, using separate functional/wasm/lint targets.
The Ledger version bump and device snapshots remain pending. Issue #3251's actual upstream
Zebra 4.1.0 provenance and #3252's owned-boundary mitigation scope/reproduction
were corrected on GitHub and the resulting bodies were verified. The receipt
PR's malformed-input claim and both Ledger filing bodies were also narrowed
and verified against their actual evidence.

A subsequent [wheel release audit](validation/release-u10-compatibility.md)
records #3250's unresolved dependency-version coordination and required
installed-wheel checks. Its existing PR body now discloses that release
prerequisite; the native implementation tests do not establish compatibility
with the minimum published wheel. The audit found no currently installable
broken stable pair and selected no new version floor.

Subtensor's hosted workflows failed at startup without Rust jobs. Python binding
runtime tests, live Sudo integration and current physical-device/emulator runs
must be distinguished from passed native tests. #3246 and #3247 disclose their
stacked dependency commits; they should be rebased after those dependencies merge.
