//! Offline executable using the maintained synthetic chain; no node or funding.
use rtao_client::{
    Client, ClientConfig, Network, SubmitOptions,
    journal::{OperationIntent, OperationJournal, RecoveryStatus},
};
use rtao_signer::{KeypairSigner, Signer};
use rtao_testkit::{FakeChain, FakeState, Reply};
use std::{path::PathBuf, time::Duration};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("pass a new disposable journal path")?,
    );
    if path.exists() {
        return Err("use a new path; existing evidence is never reset by this example".into());
    }
    let fake = FakeChain::from_state(FakeState::try_default()?);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(subxt_rpcs::RpcClient::new(fake.clone()), config).await?;
    let signer = KeypairSigner::from_seed(&[7; 32], rtao_primitives::CryptoType::Ed25519)?;
    let journal = OperationJournal::open(&path)?;
    let operation = journal.begin(OperationIntent {
        id: "sample-operation".into(),
        correlation_id: Some("sample-revision".into()),
        genesis: client.genesis(),
        account: signer.account(),
        scope: "offline-example".into(),
        effects: serde_json::json!({"action":"remark","value":"synthetic"}),
        obligation: false,
        semantic_policy: None,
        extensions: Default::default(),
    })?;
    fake.mutate_state(|state| {
        state.set_handler("author_submitAndWatchExtrinsic", |state, request| {
            state.dispatch(request);
            Reply::silent()
        })
    });
    let call = rtao_runtime::calls::system::remark(b"synthetic".to_vec())
        .compose(&client.refresh_runtime().await?)?;
    let result = client
        .submit(
            call,
            &signer,
            &SubmitOptions {
                journal: Some(operation.clone()),
                deadline: Some(Duration::from_millis(100)),
                ..Default::default()
            },
        )
        .await;
    if result.is_ok() {
        return Err("fault injection failed".into());
    }
    drop(operation);
    drop(journal);
    // Simulate process state loss: recreate storage handles. The synthetic chain
    // remains outside the restarted caller, just as a real chain would.
    let reopened = OperationJournal::open(path)?;
    let operation = reopened.bind("sample-operation")?;
    for _ in 0..2 {
        match operation.recover(&client, 16).await? {
            RecoveryStatus::Finalized(report) if report.success => {}
            _ => return Err("expected finalized historical receipt".into()),
        }
    }
    if fake.mutate_state(|state| state.submissions.len()) != 1 {
        return Err("duplicate dispatch".into());
    }
    println!(
        "Recovered the same logical operation twice from one finalized submission; replacement remains blocked."
    );
    client.close().await;
    Ok(())
}
