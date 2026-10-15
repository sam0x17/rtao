//! Preview explicit owner settings, then submit them through the public owner API.

use anyhow::Context;
use clap::{ArgGroup, Parser};
use rtao::{
    HparamArg, OwnerConfig, OwnerOps, WindowWait,
    client::{Client, ClientConfig},
    intents::{SetHyperparameter, SetMechanismCount},
    prelude::{Balance, MechId, NetUid, PlanOptions, Policy, Tao},
    wallet::{PasswordSources, Wallet},
};
use std::{collections::BTreeSet, path::PathBuf, sync::Arc, time::Duration};

#[derive(Debug, Parser)]
#[command(about = "Preview subnet owner settings; add --execute to submit them.")]
#[command(group(ArgGroup::new("settings").required(true).multiple(true)))]
struct Args {
    /// Named network or an explicit WebSocket endpoint.
    #[arg(long, env = "BT_NETWORK", default_value = "finney")]
    network: String,
    /// Existing subnet owned by this wallet's coldkey.
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..4096))]
    netuid: u16,
    #[arg(long, env = "BT_WALLET", default_value = "default")]
    wallet: String,
    #[arg(long, env = "BT_WALLET_HOTKEY", default_value = "default")]
    hotkey: String,
    #[arg(long, env = "BT_WALLET_PATH", default_value = "~/.bittensor/wallets")]
    wallet_path: PathBuf,
    /// Read the coldkey password from a file, or use its standard BT_PW_ variable.
    #[arg(long)]
    password_file: Option<PathBuf>,
    /// Set the immunity period in raw chain blocks (u16 at the current SDK pin).
    #[arg(long, group = "settings")]
    immunity_period: Option<u16>,
    /// Set the mechanism count after any requested hyperparameter succeeds.
    #[arg(long, group = "settings", value_parser = clap::value_parser!(u8).range(1..=16))]
    mechanism_count: Option<u8>,
    /// Maximum estimated fee per transaction, in native rao.
    #[arg(long)]
    max_fee_rao: Option<u64>,
    /// Bound each operation's reads, owner window and finality wait.
    #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..))]
    operation_timeout_secs: u64,
    /// Submit the requested settings; the default only prints public plans.
    #[arg(long)]
    execute: bool,
}

async fn configure(args: &Args, client: Client) -> anyhow::Result<()> {
    let netuid = NetUid::new(args.netuid)?;
    let timeout = Duration::from_secs(args.operation_timeout_secs);
    let wallet = Arc::new(
        Wallet::new(&args.wallet, &args.hotkey, &args.wallet_path)?.with_password_sources(
            PasswordSources {
                password_file: args.password_file.clone(),
                ..Default::default()
            },
        ),
    );
    // These settings move no TAO; scope both planning and submission to this subnet.
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
    let mut previews = Vec::new();
    if let Some(blocks) = args.immunity_period {
        let intent = SetHyperparameter {
            netuid,
            name: "immunity_period".into(),
            value: serde_json::json!(blocks),
        };
        previews.push(
            executor
                .plan(&intent, wallet.as_ref(), &options)
                .await?
                .to_json(),
        );
    }
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
    // Public planning never unlocks the coldkey. An executing owner operation
    // reviews a fresh head again, including after a freeze or rate-limit window.
    println!("{}", serde_json::to_string_pretty(&previews)?);
    if !args.execute {
        return Ok(());
    }

    let owner = OwnerOps::new(executor, wallet).with_config(OwnerConfig {
        operation_timeout: Some(timeout),
        ..Default::default()
    })?;
    if let Some(blocks) = args.immunity_period {
        let deadline = tokio::time::Instant::now()
            .checked_add(timeout)
            .context("operation timeout is too large")?;
        let result = owner
            .set_hparam(
                netuid,
                "immunity_period",
                HparamArg::from(blocks),
                WindowWait::UntilOpen { deadline },
            )
            .await?;
        println!("{}", serde_json::to_string(&result.to_json())?);
        result.raise_for_failure()?;
    }
    if let Some(count) = args.mechanism_count {
        let result = owner.set_mechanism_count(netuid, count).await?;
        println!("{}", serde_json::to_string(&result.to_json())?);
        result.raise_for_failure()?;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("TLS crypto provider already installed"))?;
    let args = Args::parse();
    let client = Client::connect(ClientConfig::for_network(&args.network)?).await?;
    let result = configure(&args, client.clone()).await;
    client.close().await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_setting_and_execute_are_separate_explicit_choices() -> anyhow::Result<()> {
        assert!(Args::try_parse_from(["owner-tool", "--netuid", "7"]).is_err());
        let args =
            Args::try_parse_from(["owner-tool", "--netuid", "7", "--immunity-period", "5000"])?;
        assert_eq!(args.immunity_period, Some(5000));
        assert_eq!(args.mechanism_count, None);
        assert!(!args.execute);
        Ok(())
    }

    #[test]
    fn invalid_scope_counts_and_timeout_are_rejected_before_io() {
        for (flag, value) in [
            ("--netuid", "0"),
            ("--netuid", "4096"),
            ("--mechanism-count", "0"),
            ("--mechanism-count", "17"),
            ("--immunity-period", "65536"),
            ("--operation-timeout-secs", "0"),
        ] {
            let mut command = vec!["owner-tool", "--netuid", "7", "--immunity-period", "5000"];
            let position = command.iter().position(|part| *part == flag);
            if let Some(position) = position {
                command[position + 1] = value;
            } else {
                command.extend([flag, value]);
            }
            assert!(Args::try_parse_from(command).is_err(), "{flag}={value}");
        }
    }

    #[test]
    fn maximum_native_settings_and_fee_are_retained_exactly() -> anyhow::Result<()> {
        let args = Args::try_parse_from([
            "owner-tool",
            "--netuid",
            "4095",
            "--immunity-period",
            "65535",
            "--mechanism-count",
            "16",
            "--max-fee-rao",
            "18446744073709551615",
            "--execute",
        ])?;
        assert_eq!(args.immunity_period, Some(u16::MAX));
        assert_eq!(args.mechanism_count, Some(MechId::LIMIT));
        assert_eq!(args.max_fee_rao, Some(u64::MAX));
        assert!(args.execute);
        Ok(())
    }
}
