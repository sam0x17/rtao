//! Hand-written command families and catalog discovery.

/// Local address-book management and signer account import.
pub mod addresses;
/// Operation discovery and exact semantic/chain error explanations.
pub mod catalog;
/// Miner registration collateral inspection and transactions.
pub mod collateral;
/// Shared persistent config commands.
pub mod config;
/// Stake locking, conviction and locked-alpha acceptance.
pub mod conviction;
/// Crowdloan campaign and contributor queries.
pub mod crowd;
/// EVM keys, native funding, HTTP transactions and precompile commands.
#[cfg(feature = "evm")]
pub mod evm;
/// Browser-extension bridge lifecycle and public account discovery.
#[cfg(feature = "extension")]
pub mod extension;

/// On-chain axon endpoint commands.
pub mod axon;
/// Catalog-driven transaction and read commands.
pub mod generated;
/// Subnet owner settings and read commands.
pub mod hparams;
/// Denomination conversion and existing-endpoint latency probes.
pub mod misc;
/// Saved multisig signer sets, public pending operations and proposal discovery.
pub mod multisig;
/// Public preparation and late proof refresh for registration.
pub mod pow;
/// Proxy delegation, pure-account lifecycle and pending proxy execution.
pub mod proxy;
/// Root validator funds, allocation and registration.
pub mod root;
/// Schema-derived argument coercion and defaults.
pub mod schema;
/// Stake aliases of ordinary generated transaction operations.
pub mod stake;
/// Subnet emission weight vector views and owner changes.
pub mod subnet_emission;
/// Subnet creation, registration and metagraph views.
pub mod subnets;
/// Standalone timelock encryption, decryption and envelope inspection.
pub mod timelock;
/// Public upgrade inspection and explicit multisig upgrade approval.
pub mod upgrade;
/// Local wallet management and public account views.
pub mod wallet;
/// Validator weight submission commands.
pub mod weights;

/// Build the exact installed command tree, including compiled optional groups.
pub fn command_tree() -> Result<clap::Command, crate::error::CliError> {
    let commands = vec![
        crate::completions::command(),
        crate::call::command(),
        config::command(),
        catalog::tools_command(),
        catalog::explain_command(),
        generated::tx_command()?,
        generated::query_command()?,
        hparams::command()?,
        stake::command()?,
        proxy::command()?,
        multisig::command(),
        wallet::command(),
        addresses::command(),
        root::command()?,
        conviction::command()?,
        subnets::command()?,
        {
            let misc = misc::command()
                .subcommand(weights::command())
                .subcommand(axon::command())
                .subcommand(collateral::command()?)
                .subcommand(timelock::command())
                .subcommand(crowd::command())
                .subcommand(upgrade::command());
            #[cfg(feature = "extension")]
            let misc = misc.subcommand(extension::command());
            misc
        },
    ];
    #[cfg(feature = "evm")]
    let commands = {
        let mut commands = commands;
        commands.push(evm::command()?);
        commands
    };
    Ok(crate::application::command_tree(commands))
}
