The recovered CLI machinery is now in canonical source. Its entrypoint dispatches
`call`, `doctor`, `config`, `tools` and `explain`; other command families still
need integration. See
[m4-cli-machinery.md](m4-cli-machinery.md) for the adopted behavior, safety fixes
and validation limits. This adoption adds no intent/read coverage and does not
complete M4. Owner orchestration is also present in canonical source; its current
status belongs in [state.md](state.md).

The CLI adoption composes five hash-verified recovery members: the M4-09 CLI
patch, client continuation supplement, `--json-password` replay fix, PSF
attribution gate and root NOTICE addition. The original archive and manifest
remain under `.local/recovery/2026-10-14/`; the bounded adoption record is
`.local/recovery/2026-10-14/cli-adoption.json`. No disposable source checkout or
build cache was restored.

D-041 preserves the original multisig opening across automatic later rounds.
The client composer refuses absent, replaced and already-approved pending state
and encodes `Some(expected)`. Saved semantic approvals also disable owner-rate
replanning, because a final approval can consume pending state when its inner
owner call fails. The new-opening receipt inference assumes the runtime uses
System block heights and extrinsic indices. The separate runtime source guard
has not been adopted, so future runtime pins need source review. D-041 and D-042 are registered
with regression sources; hosted validation is pending. No new upstream filing
is claimed.

Config/catalog handlers and their binary routing are adopted; see
[m4-config-catalog.md](m4-config-catalog.md). Generated transaction/query and
wallet/weights/axon candidates remain in the recovery archive. Their command builders and dispatch handlers must
be reviewed against the adopted machinery before integration. Prepared source
and historical isolated results do not establish acceptance of the combined code.
The separate M5 offline candidate also remains provisional.

Q30 supersedes the old local validation campaign. Applicable client tests and
builds run on free hosted `ubuntu-latest` CI. Do not restore the retired
`target/execute-plan.6Ro0PC` workspace, its build caches, queued gates or native
Keychain harness. Keychain/process behavior is checked through injected runners;
real Keychain, device and browser acceptance remain deferred and unverified.
Python oracle, cross-language interoperability and node scenarios are optional
targeted hosted checks. No local build or test ran during CLI adoption.
