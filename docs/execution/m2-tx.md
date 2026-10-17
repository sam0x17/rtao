# M2 transaction executor

## Current source and scope

Current work follows [PLAN.md Q30](../../PLAN.md#current-execution-scope-q30-2026-10-14): client
source and bounded free `ubuntu-latest` checks. M2-S02 is fixed by the
combined core patch at fork rev `626f2a4f`, and M2-26 is complete. The record
below preserves earlier source snapshots and their measured results.

## Historical preparation and validation

`rtao-tx` implements the M2 intent contracts, dynamic dispatch, plans, policies,
execution, typed outputs and chain errors. Balances and caps use checked native
`u64` rao (Q25 / D-035).

Plans pin read queries to one snapshot and compose calls with the current signing
runtime. Planning and preflight use public key views. Execution enforces policy
and hard preflight blocks before resolving a private signer, then submits the
reviewed call bytes. A runtime stamp mismatch causes one rebuild and another
policy check. A second mismatch fails without broadcasting.

The full execution deadline starts before planning and key resolution. Admission
has a separate limit: key resolution, nonce fetch, preparation, signing,
signature attachment and author acknowledgment share one budget. After author
acknowledgment, inclusion and finality use the full deadline. Synchronous work
that overruns a deadline is checked before the next RPC or broadcast. A timeout
after attempting an author call preserves unknown fate; the nonce lease drops
before optional reconciliation.

Transient pool retries wait for another best-chain head, reserve a fresh nonce
and reuse the reviewed call and resolved signer. Dispatch failures return an
outcome without retrying. Rejected or unknown outcomes use `Inclusion::Unobserved`.
Successful outcomes retain build extras; failed outcomes omit them. Typed output
views read the original Python keys, including `registration_mode` and
`pure_proxy`, while retaining the full data map. TAO JSON uses correctly rounded
integer division, including fixed IEEE-bit regressions beyond `2^53` rao and
at `u64::MAX`.

`IntentRegistry` accepts the generated parser from `rtao-intents` without a
production dependency cycle. `Executor::tools` projects all 83 authenticated
schemas into Python's tool manifest fields and absolute documentation URLs.
Generated input errors return a typed error. The executor's role bound has a
compile-fail test using a hotkey-only source for a coldkey transfer.

The production Client and encoded FakeChain tests cover public-only previews,
native policy limits, unavailable fee estimates, exact signed call bytes,
proxy dispatch account versus fee payer, coldkey fee warning order and nested
call leaves, definite rejection, unknown fate, transient retry, dispatch failure,
runtime replanning and synchronous deadline overruns. Output tests cover Python
field aliases and failure JSON. The actual core-constructor policy harness and
its 73,728 comparisons are recorded in [m2-tx-policy.md](m2-tx-policy.md).

The authenticated error gates cover all 390 module names, 52 dispatch names,
18 semantic codes, 16 standard invalid-transaction mappings and 256 custom
bytes. They check descriptions, remediation, explanation, documentation URLs,
classification precedence and complete JSON. Short module messages use the
generated Python docs table in declaration order, with the last nonempty name
winning. Unmapped custom values preserve the original RPC diagnostic and
continue through Python's watch/standard matching rules.

Validation on 2026-10-14 used `--locked` throughout:

| Check | Rust | Result |
| --- | --- | --- |
| `cargo test -p rtao-client -p rtao-signer -p rtao-tx --all-features` | 1.99 and 1.89 | 193 checks passed on each |
| `cargo test -p rtao-client -p rtao-tx --no-default-features` | 1.99 | 179 checks passed |
| `cargo clippy -p rtao-client -p rtao-signer -p rtao-tx --all-features --all-targets -- -D warnings` | 1.99 and 1.89 | passed on each |
| `cargo check -p rtao-tx --no-default-features` | 1.99 | passed |
| Owned Rust formatting and architecture whitespace | 1.99 | passed |

TX accounts for 37 checks: five unit tests, eight error gates, 16 executor
tests, seven core-policy tests and one compile-fail role check. The client
accounts for 142 checks, including 28 submission tests and three structured
pool-error tests. The signer accounts for 14 checks.

Final logs are `target/execute-plan.6Ro0PC/tmp/m2-tx-final-{current,msrv,
no-default,clippy,msrv-clippy}.log`. No separate scratch directories were
created by the TX owner. That task root and its build/log paths are retired
and absent; shared global Cargo/toolchain caches remain preserved.

Raw submission and shield execution are assigned to M4. Subnet-registration
completion lands with its M4 intent. Offline executor methods and complete
multisig/pure-proxy behaviors land in M5. Mandatory shield intents fail closed
until that execution path is present. This evidence does not claim those later
behaviors at that historical checkpoint. Live localnet and oracle execution
are optional targeted hosted evidence under Q30, not development exit gates.
