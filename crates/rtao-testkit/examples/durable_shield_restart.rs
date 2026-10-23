//! Finite synthetic-chain process crash/restart. No external RPC, native
//! decryption, real funds, consensus verification or production dispatch.
//! Run with journal-shield enabled and RTAO_TEMP_ROOT beneath local scratch.
use rtao_client::{
    Client, ClientConfig, JournalShieldReadiness, Network, ShieldOptions, ShieldPeriod, Wait,
    journal::{JournalControl, OperationIntent, OperationJournal},
};
use rtao_primitives::CryptoType;
use rtao_signer::{Coldkey, Signer};
use rtao_testkit::{
    FakeState,
    journal_shield::{Inner, JournalLab, LabKeys, LabResult, LabSigner, SyntheticProtocol},
};
use rtao_tx::{ExecuteOptions, Executor};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    time::Duration,
};

// Every child is reaped, including when a diagnostic assertion fails.
struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
async fn connect(endpoint: &str) -> LabResult<Client> {
    let url = url::Url::parse(endpoint)?;
    if url.scheme() != "ws" || !matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "::1")) {
        return Err("example accepts only its owned loopback peer".into());
    }
    let session = rtao_rpc::Session::connect(rtao_rpc::SessionConfig::new(url)).await?;
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_session(session, config).await?;
    client.enable_synthetic_journal_dispatch().await?;
    Ok(client)
}
async fn child(phase: &str, path: &Path, endpoint: &str) -> LabResult<()> {
    let client = connect(endpoint).await?;
    let journal = OperationJournal::open(path)?;
    let protocol = Arc::new(SyntheticProtocol::default());
    if phase == "dispatch" {
        let signer = Arc::new(LabSigner::new(CryptoType::Ed25519)?);
        let binding = journal
            .begin(OperationIntent {
                id: "restart-example".into(),
                correlation_id: Some("application:stable-operation".into()),
                genesis: client.genesis(),
                account: signer.account(),
                scope: "restart-example".into(),
                effects: json!({"System.remark":[1,2,3]}),
                obligation: false,
                semantic_policy: None,
                extensions: Default::default(),
            })?
            .with_shield_protocol(protocol)
            .material(json!({"intent":"retain-until-finalized"}))?;
        let options = ExecuteOptions {
            journal: Some(binding),
            shielded: true,
            period: Some(4),
            ..Default::default()
        };
        let call = rtao_runtime::calls::system::remark(vec![1, 2, 3])
            .compose(&client.refresh_runtime().await?)?;
        // Parent kills this process only after the actual author handler has
        // observed exact signed bytes, durable authorization and native inclusion.
        let _ = Executor::new(client.clone())
            .submit_call::<Coldkey, _>(call, &LabKeys::new(signer), &options)
            .await?;
        return Err("dispatch child unexpectedly received an acknowledgment".into());
    }
    if phase != "recover" {
        return Err("unknown example child phase".into());
    }
    journal.control(JournalControl {
        submissions_paused: true,
        allow_obligations: false,
        extensions: Default::default(),
    })?;
    let binding = journal
        .bind("restart-example")?
        .with_shield_protocol(protocol);
    let options = ShieldOptions {
        period: ShieldPeriod::P4,
        wait: Wait::Finalization,
        submit: rtao_client::SubmitOptions {
            journal: Some(binding.clone()),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut effect = None;
    for _ in 0..2 {
        match client.prepare_journal_shield(&options).await? {
            JournalShieldReadiness::Finalized(report) if report.success => {
                if effect.is_some_and(|hash| hash != report.hash) {
                    return Err("recovery changed effect identity".into());
                }
                effect = Some(report.hash);
                let record = binding.record()?;
                if record.finalized_effect()?.map(|a| a.hash) != effect || report.shield.is_none() {
                    return Err("recovery lost inner/carrier attribution".into());
                }
            }
            _ => return Err("recovery did not return the exact finalized inner".into()),
        }
    }
    client.close().await;
    println!("Recovery process: finalized inner restored twice while paused; no signer opened.");
    Ok(())
}
#[tokio::main]
async fn main() -> LabResult<()> {
    let args = std::env::args().collect::<Vec<_>>();
    if args.len() == 4 {
        return child(&args[1], Path::new(&args[2]), &args[3]).await;
    }
    if args.len() != 1 {
        return Err("usage: durable_shield_restart (owned child phases are internal)".into());
    }
    let root = PathBuf::from(
        std::env::var_os("RTAO_TEMP_ROOT")
            .ok_or("set RTAO_TEMP_ROOT to local disposable scratch")?,
    )
    .join(format!("paired-process-{}", std::process::id()));
    let path = root.join("journal.json");
    let mut lab = JournalLab::new(FakeState::default(), &path, CryptoType::Ed25519).await?;
    lab.script([Inner::LostAcknowledgment]);
    lab.release_for_process().await?;
    let exe = std::env::current_exe()?;
    let mut dispatch = Running(
        Command::new(&exe)
            .arg("dispatch")
            .arg(&path)
            .arg(lab.node.url())
            .stdout(Stdio::null())
            .spawn()?,
    );
    tokio::time::timeout(Duration::from_secs(10), lab.lost_acknowledgment()).await?;
    dispatch.0.kill()?;
    let status = dispatch.0.wait()?;
    if status.success() {
        return Err("dispatch process was not interrupted".into());
    }
    println!(
        "Dispatch process killed after one actual author request and inclusion, before acknowledgment."
    );
    // Reopen the original disk image and a fresh native Session, then release
    // the journal again so an independent recovery process owns its file lock.
    lab.restart().await?;
    let record = lab.journal()?.bind("restart-example")?.record()?;
    if record.attempts.len() != 2 {
        return Err("signed pair was not durably published".into());
    }
    lab.release_for_process().await?;
    let mut recovery = Running(
        Command::new(&exe)
            .arg("recover")
            .arg(&path)
            .arg(lab.node.url())
            .spawn()?,
    );
    let status = tokio::task::spawn_blocking(move || recovery.0.wait()).await??;
    if !status.success() {
        return Err("recovery child failed".into());
    }
    if lab.authors() != 1 {
        return Err("recovery transmitted another carrier".into());
    }
    lab.close().await;
    println!(
        "PASS: one logical operation, two signed components, one author request across process restart. Synthetic execution only."
    );
    Ok(())
}
