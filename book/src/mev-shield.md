# MEV shield

MEV shielding signs an inner transaction, encrypts it with the chain's next
shield key, then submits a signed `MevShield.submit_encrypted` carrier.
Only the carrier is sent to the author RPC. The runtime decrypts the inner
transaction for execution. Shielding hides the inner call while it is in the
mempool; its execution becomes visible on chain.

## Select shielding explicitly in Rust

Use `Executor::submit_shielded` for a typed intent or
`submit_shielded_dyn` for a dynamically parsed intent. They retain semantic
policy, origin and preflight checks before resolving the private signer.
The facade's `owner` feature enables the registration intent used below:

```rust
use std::time::Duration;
use rtao::{
    intents::BurnedRegister,
    primitives::{Balance, NetUid, Tao},
    signer::{Coldkey, KeySource},
    tx::{ExecuteOptions, ExtrinsicResult, ShieldOptions, ShieldPeriod},
    Executor,
};

async fn register_shielded<K: KeySource<Coldkey>>(
    executor: &Executor,
    keys: &K,
    netuid: NetUid,
) -> Result<ExtrinsicResult<()>, rtao::tx::TxError> {
    let intent = BurnedRegister {
        netuid,
        hotkey_ss58: None,
    };
    executor.submit_shielded(&intent, keys, &ShieldOptions {
        period: ShieldPeriod::P8,
        max_fee: Some(Balance::<Tao>::from_rao(1_000_000)),
        execute: ExecuteOptions {
            deadline: Some(Duration::from_secs(90)),
            wait_for_finalization: true,
            ..Default::default()
        },
    }).await
}
```

The caller supplies a policy-bearing executor, an existing key source and a
fee cap appropriate to the operation. A missing hotkey argument uses the key
source's public hotkey. Inspect the returned result and call
`raise_for_failure()` before treating registration as successful. Returning
`Ok(result)` can still represent an on-chain or pool refusal.

The ordinary Rust `execute` path automatically shields intents whose catalog
marks it required, including burned registration and adding collateral.
It does not select shielding merely because an intent's CLI default enables
it. For an optional shield, choose `submit_shielded` explicitly.

For already composed, runtime-stamped raw calls, use `submit_call` or
`submit_call_dyn` with `ExecuteOptions::shielded = true`. Raw calls require
`Policy::allow_raw_calls` when a policy is active; their bytes do not acquire
an intent's semantic authorization. The lower-level
`Client::submit_shielded` accepts an already available `Signer` and client
`ShieldOptions`. Prefer the executor when you need lazy private-key access
and semantic checks. The client and transaction-layer option types are
different types.

## Short eras, paired nonces and fees

`ShieldPeriod` accepts only `P4` and `P8`; the default is eight blocks. The
carrier uses nonce `n`, the inner uses `n + 1`, and the next cached lease is
`n + 2`. Both signatures share a finalized era anchor. Do not allocate or
override those paired nonces yourself.

If `B` is the best height, `F` the finalized height and `p` the period, the
client refuses when `B + 2 >= F + p`. Four blocks therefore tolerate at most
one block of finality lag, and eight tolerate at most five. This guard runs
before signing and again during preparation. Excess lag produces
`TxError::ShieldEraExpired` through the executor. A slow signer can exhaust
the short era even when an ordinary transaction would remain valid.

The signing account needs free TAO for the outer carrier fee. Alpha-fee
support for an unshielded inner call does not pay that fee. `max_fee` on the
transaction-layer shield options caps the inner fee; it is not a total
carrier-plus-inner cap. The executor also checks an intent's required free
reserve with carrier overhead where its preflight supplies that reserve.
`Client::estimate_shielded_carrier_fee` uses a conservative maximum-size
carrier probe with public identity and does not reserve a nonce.

## CLI routing

The CLI resolves required shielding, then an explicit `--mev-shield` or
`--no-mev-shield`, then persistent configuration, then the intent default.
A required-shield operation refuses an explicit disable. Forced or required
shielding also refuses an unavailable next key or insufficient carrier fees.
A default-only shield may fall back to direct submission in those cases and
reports that choice. An RPC error reading the key is an error, not an absent
key. Inspect dry-run effects and warnings when reviewing the route.

For shielded CLI transactions, `--period` must be four or eight blocks when
supplied. The raw `call` command can opt in with `--mev-shield`. Sequential
`hparams trim-null` uses direct finalized receipts and refuses shielding;
pass `--no-mev-shield` when persistent configuration enables it.

## Receipts and uncertain outcomes

Pool acceptance proves only that the carrier was accepted. For inclusion or
finalization waits, the client scans for the exact signed inner hash and
resolves its dispatch outcome. Finalization checks canonical history and
handles a reorganization while waiting. A successful carrier with no inner
before expiry is reported as failure. Retain the carrier and inner hashes;
executor results expose shield metadata including `inner_extrinsic_hash`.

Shield carrier rejection or unknown fate never causes automatic
retransmission. Reconciliation examines history rather than sending another
carrier. Keep an unresolved hash until its fate is known before deciding
whether to prepare a new transaction. The executor can rebuild after a
pre-broadcast runtime mismatch, and its separate owner rate-limit rule can
permit one new execution after a known included refusal. Those cases do not
authorize replaying a carrier whose fate is unknown.

Hosted Rust fixtures in `crates/rtao-client/tests/shield.rs` exercise the
envelope, nonce order, era boundaries, fee refusal, missing keys, receipt
scanning, expiry, reorganization and uncertain fate. Executor and CLI tests
cover policy, private-key ordering and routing. Their presence does not
establish the latest run's result or live-chain coverage. Localnet and Python
interop remain optional targeted hosted checks; this guide requires no local
node, container or testnet deployment.
