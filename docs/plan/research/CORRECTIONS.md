# Corrections to the research reports (verified by the orchestrator on 2026-10-13)

1. **C1 §0 finding 2 is wrong about a wire-visible identifier change at spec 478.** The 478 runtime does wrap
   the fee extension (`runtime/src/transaction_payment_wrapper.rs`, `IDENTIFIER = "ChargeTransactionPaymentWrapper"`),
   but the wrapper overrides `fn metadata()` to return `ChargeTransactionPayment::<T>::metadata()`
   (`transaction_payment_wrapper.rs:324-326`). Devnet runs spec 478 today and its metadata still lists
   `ChargeTransactionPayment` with the same type id 409. Clients that match extensions by identifier
   (bittensor-core's `PAYLOAD_FIELDS`, subxt's default params) are unaffected.
2. **Spec 478 changes no client-visible metadata.** `subxt diff meta/finney.scale meta/devnet.scale` reports no
   difference; the two 404,727-byte blobs differ in exactly one byte (the `System.Version` constant's
   spec_version, 477 -> 478). Many runtime releases are behavior-only; rtao's drift tooling must treat
   "spec bumped, surface unchanged" as the common case (the Python codegen header deliberately omits
   spec_version for the same reason, A2 finding 3).
3. **Publishability-driven recommendations are superseded by the user.** Reports B (§9 option d), E (summary
   table), F (§0 item 6, 9) and G (§8 item 10) recommend not depending on `bittensor-core` mainly because it
   is `publish = false` with git/patch dependencies that crates.io rejects. The user has decided crates.io does
   not matter, git dependencies are fine, and rtao should be a veneer over bittensor-core. The *technical*
   points in those reports still stand and must be addressed: blocking reqwest HTTP client (not tokio), polling
   block streams, no subscriptions, dynamic `Value` codec in Python/cyscale shapes, the 64-block MEV-shield era
   in `client.rs` vs the chain's 8-block maximum (A1 finding 3), the epoch-schedule predicate difference
   (C1 finding 11), V16 metadata rejection, no nonce cache, ~20 hand-written IntentCall constructors.
4. **Coexistence verified**: see `feasibility-veneer-coexistence.md` — bittensor-core and subxt 0.51 share one
   SCALE stack and run together against finney.
