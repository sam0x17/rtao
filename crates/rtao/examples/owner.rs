//! Preview owner intents, then configure a subnet through the bounded owner API.

use anyhow::{Context, ensure};
use clap::Parser;
use rtao::{
    client::{Client, ClientConfig},
    intents::{SetHyperparameter, SetMechanismCount},
    neuron::{HparamArg, OwnerConfig, OwnerOps, WindowWait},
    prelude::{Balance, MechId, NetUid, PlanOptions, Policy, Tao},
    wallet::{PasswordSources, Wallet},
};
use std::{collections::BTreeSet, path::PathBuf, sync::Arc, time::Duration};

#[derive(Parser)]
#[command(about = "Preview owner settings; add --execute to submit them.")]
struct Args {
    /// Named network or an explicit WebSocket endpoint.
    #[arg(long, default_value = "local")]
    network: String,
    /// Existing subnet owned by this wallet's coldkey.
    #[arg(long)]
    netuid: u16,
    #[arg(long, default_value = "default")]
    wallet: String,
    #[arg(long, default_value = "default")]
    hotkey: String,
    #[arg(long, default_value = "~/.bittensor/wallets")]
    wallet_path: PathBuf,
    /// Read the coldkey password from a file, or use its standard BT_PW_ variable.
    #[arg(long)]
    password_file: Option<PathBuf>,
    /// Immunity period in raw chain blocks.
    #[arg(long, default_value_t = 5000)]
    immunity_period: u64,
    /// Optionally set the mechanism count after the hyperparameter succeeds.
    #[arg(long)]
    mechanism_count: Option<u8>,
    /// Maximum estimated transaction fee in native rao.
    #[arg(long)]
    max_fee_rao: Option<u64>,
    /// Bound each owner operation, including reads, windows and finality waits.
    #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..))]
    operation_timeout_secs: u64,
    /// Submit the reviewed settings through the owner API.
    #[arg(long)]
    execute: bool,
}

async fn configure(args: &Args, client: Client) -> anyhow::Result<()> {
    let netuid = NetUid(args.netuid);
    let timeout = Duration::from_secs(args.operation_timeout_secs);
    let wallet = Arc::new(
        Wallet::new(&args.wallet, &args.hotkey, &args.wallet_path)?.with_password_sources(
            PasswordSources {
                password_file: args.password_file.clone(),
                ..Default::default()
            },
        ),
    );
    // The policy applies to the semantic intent. Owner configuration moves no TAO.
    let executor = rtao::intents::executor(client).with_policy(Policy {
        max_fee: args.max_fee_rao.map(Balance::<Tao>::from_rao),
        max_spend: Some(Balance::<Tao>::from_rao(0)),
        allowed_netuids: Some(BTreeSet::from([netuid])),
        ..Default::default()
    });
    let options = PlanOptions {
        deadline: Some(timeout),
        ..Default::default()
    };
    let parameter = SetHyperparameter {
        netuid,
        name: "immunity_period".into(),
        value: serde_json::json!(args.immunity_period),
    };
    // Planning uses the public wallet view and never unlocks the coldkey.
    let mut previews = vec![
        executor
            .plan(&parameter, wallet.as_ref(), &options)
            .await?
            .to_json(),
    ];
    if let Some(count) = args.mechanism_count {
        let intent = SetMechanismCount {
            netuid,
            mechanism_count: MechId(count),
        };
        previews.push(
            executor
                .plan(&intent, wallet.as_ref(), &options)
                .await?
                .to_json(),
        );
    }
    println!("{}", serde_json::to_string_pretty(&previews)?);
    if !args.execute {
        return Ok(());
    }

    let owner = OwnerOps::new(executor, wallet).with_config(OwnerConfig {
        operation_timeout: Some(timeout),
        ..Default::default()
    })?;
    let deadline = tokio::time::Instant::now()
        .checked_add(timeout)
        .context("operation timeout is too large")?;
    // Each wake replans at a new head. An admitted execution drains to its fate.
    let result = owner
        .set_hparam(
            netuid,
            "immunity_period",
            HparamArg::from(args.immunity_period),
            WindowWait::UntilOpen { deadline },
        )
        .await?;
    println!("{}", serde_json::to_string(&result.to_json())?);
    result.raise_for_failure()?;
    if let Some(count) = args.mechanism_count {
        let result = owner.set_mechanism_count(netuid, count).await?;
        println!("{}", serde_json::to_string(&result.to_json())?);
        result.raise_for_failure()?;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    ensure!(
        args.mechanism_count != Some(0),
        "mechanism count must be positive"
    );
    let client = Client::connect(ClientConfig::for_network(&args.network)?).await?;
    let result = configure(&args, client.clone()).await;
    client.close().await;
    result
}
