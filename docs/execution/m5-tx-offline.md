# M5 executor offline signing

The executor now exposes `prepare_call` and `submit_signature` over the client's
authenticated offline transaction API. Public preparation accepts a composed
call, an account and `PrepareOptions`. It keeps proxy, Sudo and multisig call
bytes intact. Automatic nonce selection reads a fresh chain nonce without
reserving cached state; an explicit nonce also reserves no lease. Fees use
native u64 rao.

Submission applies the current executor policy, or the explicit per-call policy
replacement, through the same `allow_raw_calls` check as `submit_call`. Preparing
or importing a payload cannot authorize its submission. New proxy or shield
options refuse because they would change signed bytes. The prepared nonce and
era remain authoritative; waiting, deadlines and reconciliation come from
`ExecuteOptions`. The executor never replans or retries an externally signed
call.

The client authenticates imported payloads, verifies the external signature and
algorithm, checks both runtime versions and chain/era identity, and owns the
explicit nonce lease during submission. Its existing deadline handling retains
the attempted transaction hash after an unacknowledged author RPC. The executor
uses its existing receipt conversion without adding an enclosing timeout that
could lose that hash. Source-level API compatibility was checked with the agent
maintaining the client implementation; hosted compilation remains required.

The recovery source was read in memory from
`m5-offline/proof/m5-source-candidate.patch` inside
`.local/recovery/2026-10-14/unfinished-source.tar.gz`. Its SHA-256 was verified as
`0afc4b1f4010c7031c2d8683935fd3c28da38f9e14a96775b8802240aea63031`.
Only the executor, tx exports and tx offline files were adapted from that
candidate. The existing owner-retry, intent review and receipt behavior was
preserved. No recovery build output was extracted.

Eight executor tests are prepared in `crates/rtao-tx/tests/offline.rs`: current
policy after JSON import and per-call replacement; exact proxy/Sudo/multisig
bytes and resolved receipt identity; refusal of post-signature wrapper options;
bad signature/account/algorithm refusal before a nonce lease; independent spec
and transaction version changes; fresh and explicit preparation nonces with
immutable signed era/tip; preparation and submission deadline refusal; and exact
unknown-fate hash retention without retry. Client tests separately cover the
wire schema, import authentication and malformed payloads.

Only source inspection, installed formatting and diff checks ran locally under
Q30. The new tests and compilation are pending free Ubuntu CI. No Python,
build/test command, scratch directory, wallet access or network node was used.
This source checkpoint does not establish M5 acceptance or cross-language
interop execution.
