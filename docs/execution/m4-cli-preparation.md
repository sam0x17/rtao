The recovered CLI machinery is in canonical source. Its committed entrypoint
registers `call`, `doctor`, `config`, `tools` and `explain`. Transaction/query
and context integration now includes verified hosted generated interfaces;
weights, axon and implemented hyperparameter routes are added for hosted checks.
Other command families remain incomplete. See
[m4-cli-machinery.md](m4-cli-machinery.md) for the adopted behavior, safety fixes
and validation limits. This adoption adds no intent/read coverage and does not
complete M4. Owner orchestration is also present in canonical source; its current
status belongs in [state.md](state.md).

The CLI adoption composes five hash-verified recovery members: the M4-09 CLI
patch, client continuation supplement, `--json-password` replay fix, PSF
attribution gate and root NOTICE addition. The recovery archive, manifest and
adoption record are not retained in the repository. No disposable source
checkout or build cache was restored.

D-041 preserves the original multisig opening across automatic later rounds.
The client composer refuses absent, replaced and already-approved pending state
and encodes `Some(expected)`. Saved semantic approvals also disable owner-rate
replanning, because a final approval can consume pending state when its inner
owner call fails. The new-opening receipt inference assumes the runtime uses
System block heights and extrinsic indices. The separate runtime source guard
has not been adopted, so future runtime pins need source review. D-041 and D-042
are registered with regression sources and included in the checked-in parity
matrix. Combined hosted validation is pending. No new upstream filing is claimed.

Config/catalog handlers and their binary routing are adopted; see
[m4-config-catalog.md](m4-config-catalog.md). Corrected transaction/query source
now resolves plural account arrays, separates pending behavior from feature
availability and preserves the effective origin during late PoW refresh.
Plural name resolution is registered as additive D-043 and its declared matrix
row is adopted from hosted generation. The prepared
human review includes preflight facts from the same snapshot and call as its plan.
External backend factories and rich staking pickers remain incomplete; see
[m4-generated-review.md](m4-generated-review.md). Wallet and subnet commands
are committed in `a56d518`, with strict M5 call import in `0812681` and the
two M5 reads in `2331a32`/`e79b6ac`. Read dispatch needs hosted regeneration;
the broader offline API and sequential trim-null session remain in progress.
Prepared source and historical isolated results do not establish acceptance
of the combined code.

Q30 supersedes the old local validation campaign. Applicable client tests and
builds run on free hosted `ubuntu-latest` CI. Do not restore the retired local
validation workspace, its build caches, queued gates or native Keychain harness. Keychain/process behavior is checked through injected runners;
real Keychain, device and browser acceptance remain deferred and unverified.
Python oracle, cross-language interoperability and node scenarios are optional
targeted hosted checks. No local build or test ran during CLI adoption.
