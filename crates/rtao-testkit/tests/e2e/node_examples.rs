// Derived from RaoFoundation/subtensor sdk/bittensor-core/tests/e2e.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Node-dependent documentation items called by the existing e2e scenarios.

use rtao_client::{BlockRef, Client, ViewExt};
use rtao_core::codec::Value;
use rtao_primitives::{AccountId, Balance, NetUid, Tao};
use rtao_runtime::storage::subtensor_module as storage;
use rtao_signer::{Coldkey, KeySource};
use rtao_tx::{ExecuteOptions, Executor, ExtrinsicResult, TxError};
use std::{error::Error, time::Duration};

#[docify::export(live_neurons_descriptor)]
pub(super) fn neurons_api()
-> rtao_runtime::RuntimeApiDescriptor<(rtao_primitives::NetUid,), rtao_core::DynValue> {
    // Match upstream's generic accessor, which returns the core Value carrier.
    // Reuse the generated signature and projection rather than assuming a type ID.
    rtao_runtime::RuntimeApiDescriptor::new(
        rtao_runtime::runtime_apis::neuron_info_runtime_api::GET_NEURONS_LITE.surface(),
    )
}

/// Read storage, a constant and a runtime API through one connected snapshot.
#[docify::export(live_typed_reads)]
pub(super) async fn typed_reads(client: &Client, netuid: NetUid) -> Result<(), Box<dyn Error>> {
    let snapshot = client.at(BlockRef::Best).await?;
    let tempo = snapshot.storage(&storage::TEMPO, (netuid,)).await?;
    assert!(tempo > 0);
    let ed = snapshot
        .constant(&rtao_runtime::constants::balances::EXISTENTIAL_DEPOSIT)
        .await?;
    assert!(ed.0 > 0);
    let neurons = snapshot.call(&neurons_api(), (netuid,)).await?;
    assert!(matches!(neurons.as_core(), Value::List(_)));
    Ok(())
}

/// Submit a typed transfer through the caller's shared executor and coldkey.
#[docify::export(live_typed_transfer)]
pub(super) async fn execute_transfer<K: KeySource<Coldkey>>(
    executor: &Executor,
    coldkey: &K,
    destination: AccountId,
    amount: Balance<Tao>,
) -> Result<ExtrinsicResult, TxError> {
    let transfer = rtao_intents::transfer::Transfer {
        dest_ss58: destination.into(),
        amount_tao: amount.into(),
        keep_alive: true,
    };
    let options = ExecuteOptions {
        deadline: Some(Duration::from_secs(240)),
        ..Default::default()
    };
    executor.execute(&transfer, coldkey, &options).await
}
