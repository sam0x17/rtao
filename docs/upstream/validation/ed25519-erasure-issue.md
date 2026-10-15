`bittensor-core::keys::Keypair` leaves its owned Ed25519 signing object without a wiping destructor. Dropping the final owner wipes optional retained mnemonic/seed buffers, but the underlying Ed25519 pair still contains the secret seed, expanded scalar and nonce prefix.

The source chain at v478 is:

- [Core KeypairInner/Keypair](https://github.com/RaoFoundation/subtensor/blob/850f2755bb05a3851f948489f0ab27b0645cebce/sdk/bittensor-core/src/keys/mod.rs#L127) owns `sp_core::ed25519::Pair` directly and has no `Drop` implementation. Its optional mnemonic/seed fields do use `Zeroizing`.
- [URI construction](https://github.com/RaoFoundation/subtensor/blob/850f2755bb05a3851f948489f0ab27b0645cebce/sdk/bittensor-core/src/keys/mod.rs#L263) sets both optional buffers to `None`, so this affects a normal URI-derived Ed25519 keypair too.
- [The pinned sp-core pair](https://github.com/RaoFoundation/polkadot-sdk/blob/cacb4310f20c7cac83eb3ccd8ed5a5ad4212608a/substrate/primitives/core/src/ed25519.rs#L55) derives `Copy` and owns an Ed25519-Zebra `SigningKey`.
- [Ed25519-Zebra 4.1.0 SigningKey](https://docs.rs/ed25519-zebra/4.1.0/src/ed25519_zebra/signing_key.rs.html#59-68) derives `Copy`, `Clone` and `Zeroize`, but does not wipe on drop. Deriving `Zeroize` does not invoke it automatically.

Current subtensor `main` was also checked on 2026-10-14 and still owns the pair without an enclosing wiping destructor. The Sr25519 branch differs: its Schnorrkel keypair/secret implementations call zeroization on drop.

A bounded Rust probe against the locked `sp-core` build confirms the destructor distinction:

```rust
fn main() {
    assert!(!std::mem::needs_drop::<sp_core::ed25519::Pair>());
    assert!(std::mem::needs_drop::<sp_core::sr25519::Pair>());
    println!("ed25519_pair_needs_drop=false");
    println!("sr25519_pair_needs_drop=true");
}
```

The assertions passed again on rustc 1.99.0 and Rust 1.89.0/aarch64 macOS in an actual `bittensor-core` integration test at v478 with the upstream lockfile unchanged: `sp-core` 37.0.0 at `cacb4310f20c7cac83eb3ccd8ed5a5ad4212608a`, Ed25519-Zebra 4.1.0, and Schnorrkel 0.11.5. A compile-time `Copy` constraint on the actual Ed25519 pair also passed. The original downstream probe used the same sp-core revision with compatible Zebra 4.2.0, whose relevant fields and lack of a wiping destructor agree with 4.1.0. This checks the type's destructor behavior; it does not read freed memory or claim that a particular optimizer keeps every dropped byte intact.

The concrete risk is residual signing material after the final owner drops, increasing exposure to a later process-memory disclosure or dump. No network exploit or user secret was involved in this check.

Please provide erasure at the core's owning boundary for the actual Ed25519 signing object, including the expanded scalar and nonce prefix, and add regression coverage for final-drop behavior. Wiping a `to_raw_vec()` copy alone cannot erase the separate pair. This may require support from the underlying `sp-core` pair rather than a change to the retained mnemonic/seed fields.
