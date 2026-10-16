# M1-22 probe source and historical evidence

Q30 keeps offline pin and fixture checks in ordinary hosted `pr.yml`. Live
probes are optional targeted hosted evidence. No local run, service, retired
build root, scheduled upkeep or issue publishing is required for M1.

Historically, `cargo xtask probe --networks finney,testnet` passed on 2026-10-14, from
08:25:09 to 08:28:43 UTC. The complete observations are in
[m1-probe.json](m1-probe.json); [m1-probe-run.md](m1-probe-run.md) is the generated
failure-issue report format. This evidence covers the live probe in M1-22 and
M1 exit 1; the other milestone exits remain separate checks.

| Observation | Finney | Testnet |
| --- | --- | --- |
| Finalized block | 9,280,307 | 8,218,680 |
| Finalized wall-clock age | 44.169 s | 39.226 s |
| Best-to-finalized timestamp lag | 36 s | 24 s |
| Runtime spec | 478 | 478 |
| Netuid 1 | 256 UIDs; all three paths equal | 256 UIDs; all three paths equal |
| Largest active subnet | Netuid 120, 295 UIDs; all three paths equal | Netuid 301, 407 UIDs; all three paths equal |
| Last ten finalized blocks | 9,280,298–9,280,307; 2,234 events decoded | 8,218,671–8,218,680; 809 events decoded |
| Enabled metadata-hash validation | `0x010001`: `Err(Invalid(Payment))` | `0x010001`: `Err(Invalid(Payment))` |

Both live `:code` hashes matched
`0x7e722b3aa5c9192c98665a5d65fa91364bf9055e21fc90efce6d59bf4ef414e1`.
Both endpoints served V15 without the missing-method needle; the bytes matched
the product runtime cache. Its recomputed RFC-0078 digest matched the immutable
pin, `0xd1f8b86bfb8851c364cebee7cf9d7c1cfcc511ce559b57ea98afaf68f3d86987`.

`cargo xtask probe pins` generates [codegen/probe-pins.toml](../../codegen/probe-pins.toml)
offline from each network's SHA256-verified metadata index. Each digest row binds
its spec, genesis, `:code` hash, upstream commit, V15 SHA256 and chain constants.
`probe pins --check` recomputes every row and rejects stale output. The expected
digest uses immutable inputs rather than current node properties. The runtime
build uses `enable_metadata_hash("TAO", 9)`; its metadata records the spec name
and SS58 prefix. Testnet's node display symbol is `testTAO`, and is recorded
independently of the runtime build's `TAO` digest constant.

The population scan enumerates active `NetworksAdded` entries and reads their
`SubnetworkN` values at the same finalized hash. Each metagraph decoder is
explicitly required, so an automatic fallback cannot pass the differential
check. All three calls use the same immutable snapshot, and their complete
`to_json()` values must compare equal. The log records the population scan and
each path's JSON SHA256. The public `subnets().metagraph(NetUid(1))` accessor also
ran with its default commitment fetching enabled. Events select the parent
runtime separately for each immutable block.

The core creates the transfer, signature payload, Enabled signed extensions,
ed25519 signature and complete signed extrinsic. The request uses only
`state_call` with `TaggedTransactionQueue_validate_transaction`, source
`External` and the pinned state hash. The unfunded key is generated in memory;
its secret is never saved. The log retains the public account, signed extrinsic,
validation arguments, response and decoded result. Strict core decoding rejects
malformed or trailing response bytes. `UnknownTransaction::CannotLookup` and
`InvalidTransaction::BadProof` fail the check; the observed payment failure is
allowed by architecture §12.6. No transaction was submitted.

The probe observed all five extracted Finney ordinary/archive endpoints and
testnet's endpoint using health and method-discovery reads. All accepted those
reads, and no policy refusal was observed. Sessions use one request per second,
one burst token, one request in flight and bounded retries. These are passive
observations, not measurements of a provider's maximum quota. A check has a
180-second deadline and a network run has a ten-minute deadline. Completed
networks are written immediately to the report; failed prerequisites are
reported explicitly as unavailable.

All three probe tests passed on the current toolchain and Rust 1.89. They reject
metadata bound to the wrong spec and decode
the real validity-error wire variants, reject both prohibited results and
truncated/trailing responses, and verify an Enabled signed extrinsic, its local
signature, mode and exact runtime-API argument boundaries. The offline pin
check passed on Rust 1.89, and `cargo clippy --locked -p xtask --all-targets -- -D warnings`
passed. The workflow YAML parsed successfully.

Ordinary hosted gates run `probe pins --check` offline. A future targeted hosted
live check may use the recorded network/output flags with bounded timeouts and
retain its own source/result identity; the historical result is not a current
endpoint observation.

The [probe workflow draft](../../.github/deferred-workflows/probe.yml) is inactive
outside `.github/workflows`. No six-hour schedule or automatic issue update is
installed for current development. The former `target/execute-plan.6Ro0PC`
root is retired and absent on the laptop; its historical build/tool references
do not authorize recreating or resuming it. No wallet was created for that run.
