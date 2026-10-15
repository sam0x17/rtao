A failed `Sudo.sudo` can report success through the blocking SDK. `Sudid` decodes to `{sudo_result: Err(...)}`, but the current client looks for `result` and then tests the entire attribute map for `Err`. Outer `System.ExtrinsicSuccess` therefore hides the inner failure.

Add ungated `protocol::receipt` and use it in the blocking client. It resolves all six Python-supported nested-result events, including `SudoAsDone`, `Utility.BatchInterrupted` and `Utility.ItemFailed`; retains the five MevShield failure forms; and uses the last `TransactionFeePaid.actual_fee`, falling back to checked Treasury/Balances deposit sums. Native fee amounts must fit `u64` rao. Existing `TxOutcome` field types and semantic-code conversion remain intact.

The regression encodes `Sudid(Err(BadOrigin))` and outer success through actual Finney v478 V15 `System.Events` metadata, then decodes it with the public storage helper. The resulting attributes contain `sudo_result` and no `result`; the new resolver returns failure. The resolver rejects missing required fee, weight and outer failure attributes; nested wrapper discovery follows Python and skips absent result fields. Live Sudo-wrapper evidence remains pending.

The inherited Python-style diagnostic formatter now uses a generated Unicode 15 printable table matching CPython 3.12.11. U+1C89 is escaped as `\u1c89`, while Unicode 15 U+1E4F1 remains printable. An independent native comparison checked every Rust scalar against the pinned CPython generated table, with zero differences for the shipping printable table. The initial Unicode 16 dependency is removed. The complete Unicode-DFS-2016 notice is embedded in the table; the core package declares `Apache-2.0 AND Unicode-DFS-2016`. The final lockfile is unchanged from the v478 base.

This depends on #3246's portable protocol helpers and #3243's value navigation. The diff includes their original commits until they merge. Original receipt commit `aadeb06bb517b24428f4d7f64a351ccc8765e502` is preserved; Unicode correction `849c192e9702fa245a6f16941359290786554d85` follows it. Both protocol and receipt branches received ordinary corrective commits without rewriting their published ancestry.

Clean committed HEAD validation on macOS ARM64:

- Rust 1.89.0 and 1.99.0: four receipt regressions and 27 inherited portable tests, including one public formatter test covering 21 Unicode, quote, control and escape cases, pass on each compiler.
- Each compiler passes 71 all-feature library tests; one existing test requiring physical OpenCL GPUs remains ignored. Binding compilation passes with all features/targets; the binding defines no Rust unit tests.
- Rust 1.89.0 core and binding Clippy passes with all features/targets and warnings denied, using a separate build target for this checkout.
- Portable `wasm32-unknown-unknown` checks pass on both compilers; package formatting, exact source/lock/scope guards and clean-tree checks pass.

The ordinary push used the protected pre-push hook. PyO3 checks set `PYO3_NO_PYTHON=1`; no local Python interpreter was used for this correction. Functional checks use a unique workspace compiler wrapper. Strict Clippy on Rust 1.99 is not claimed because existing base-file lints remain.

Hosted Python behavior/generator tests and live integration remain pending. Earlier remote workflows ended in startup failure without Rust jobs; local validation does not establish hosted success.
