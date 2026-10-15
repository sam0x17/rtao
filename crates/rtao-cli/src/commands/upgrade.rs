// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/upgrade.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Discover, verify and approve runtime-upgrade proposals using existing endpoints.

mod artifacts;
mod chain;

use crate::{
    books::split_signatory_refs,
    call::{RawMultisig, RawSignerFactory, SigningSelection},
    context::AppContext,
    error::CliError,
    globals::{Tier, with_globals},
    output::ReviewSection,
    prompt::{PromptHistory, WalletConfirmation},
};
use artifacts::{BundleInputs, GithubFetch, ProposalBundle, ReleaseRef, UpgradeFetch};
use clap::{Arg, ArgMatches, Command};
use rtao::{
    client::{BlockRef, Client, ViewExt},
    core::value_ext::ValueExt,
    primitives::{AccountId, CryptoType},
    signer::{KeyView, SignerRole},
    tx::{ExecuteOptions, Multisig},
};
use serde_json::{Map, Value, json};

fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}
fn text<'a>(matches: &'a ArgMatches, field: &str) -> Option<&'a str> {
    matches
        .try_get_one::<String>(field)
        .ok()
        .flatten()
        .map(String::as_str)
}

/// Construct the three pinned runtime-upgrade routes.
pub fn command() -> Command {
    let source = |name: &'static str| {
        Command::new(name)
            .arg(
                Arg::new("url")
                    .long("url")
                    .help("GitHub release page or OWNER/REPO@TAG."),
            )
            .arg(
                Arg::new("hex_url")
                    .long("hex-url")
                    .help("URL of the raw call-data hex."),
            )
            .arg(
                Arg::new("hex_file")
                    .long("hex-file")
                    .help("Local call-data hex file."),
            )
            .arg(
                Arg::new("wasm")
                    .long("wasm")
                    .help("Already-built runtime wasm to compare with the proposal."),
            )
    };
    Command::new("upgrade")
        .about("Discover, verify, and sign runtime-upgrade proposals.")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(with_globals(
            Command::new("pending").arg(
                Arg::new("repo")
                    .long("repo")
                    .default_value("RaoFoundation/subtensor"),
            ),
            Tier::Read,
        ))
        .subcommand(with_globals(source("check"), Tier::Read))
        .subcommand(with_globals(
            source("sign")
                .arg(Arg::new("multisig").long("multisig"))
                .arg(
                    Arg::new("multisig_threshold")
                        .long("multisig-threshold")
                        .value_parser(clap::value_parser!(u16).range(1..)),
                )
                .arg(Arg::new("signatories").long("signatories"))
                .arg(Arg::new("other_signatories").long("other-signatories")),
            Tier::Tx,
        ))
}

/// Family-owned flags excluded from shared global resolution.
pub fn reserved(group: &ArgMatches) -> Result<Vec<&'static str>, CliError> {
    match group.subcommand_name() {
        Some("pending") => Ok(vec!["repo"]),
        Some("check") => Ok(vec!["url", "hex_url", "hex_file", "wasm"]),
        Some("sign") => Ok(vec![
            "url",
            "hex_url",
            "hex_file",
            "wasm",
            "multisig",
            "multisig_threshold",
            "signatories",
            "other_signatories",
        ]),
        _ => Err(CliError::input("missing upgrade command")),
    }
}
fn bundle_inputs(matches: &ArgMatches) -> BundleInputs {
    BundleInputs {
        release_url: text(matches, "url").map(str::to_owned),
        hex_url: text(matches, "hex_url").map(str::to_owned),
        hex_file: text(matches, "hex_file").map(str::to_owned),
        wasm_path: text(matches, "wasm").map(str::to_owned),
    }
}
fn validate_inputs(inputs: &BundleInputs) -> Result<(), CliError> {
    if [&inputs.release_url, &inputs.hex_url, &inputs.hex_file]
        .into_iter()
        .filter(|value| value.as_ref().is_some_and(|value| !value.is_empty()))
        .count()
        != 1
    {
        return Err(failure(
            "pass exactly one of --url, --hex-url, or --hex-file",
        ));
    }
    if let Some(url) = inputs.release_url.as_ref().filter(|url| !url.is_empty()) {
        ReleaseRef::parse(url).map_err(failure)?;
    }
    Ok(())
}

/// Reject invalid public input before opening a chain connection.
pub fn validate_before_connect(group: &ArgMatches) -> Result<(), CliError> {
    let (name, leaf) = group
        .subcommand()
        .ok_or_else(|| CliError::input("missing upgrade command"))?;
    if name == "pending" {
        let repo = text(leaf, "repo").unwrap_or("RaoFoundation/subtensor");
        if repo.split('/').count() != 2
            || repo.split('/').any(|part| {
                part.is_empty()
                    || !part.chars().all(|character| {
                        character.is_alphanumeric() || matches!(character, '_' | '-' | '.')
                    })
            })
        {
            return Err(CliError::input("--repo must be OWNER/REPO"));
        }
        return Ok(());
    }
    validate_inputs(&bundle_inputs(leaf))
}

/// Run the family with the ordinary connector and lazy native signer factory.
pub async fn run(
    ctx: &mut AppContext,
    group: &ArgMatches,
    history: &mut PromptHistory,
) -> Result<(), CliError> {
    validate_before_connect(group)?;
    let (name, leaf) = group
        .subcommand()
        .ok_or_else(|| CliError::input("missing upgrade command"))?;
    let fetch = GithubFetch::new()?;
    let bundle = if name == "pending" {
        None
    } else {
        Some(load_bundle(ctx, leaf, &fetch).await?)
    };
    if name == "sign" {
        ctx.confirm_wallet(
            &WalletConfirmation {
                help: "Wallet whose coldkey is a sudo multisig signatory.".into(),
                require_coldkey: true,
                ..WalletConfirmation::default()
            },
            history,
        )?;
    }
    let factory = crate::signer_backends::NativeSignerFactory::new(ctx);
    let ctx = ctx.clone();
    let app = ctx.clone();
    ctx.run(move |client| async move {
        execute_loaded(&app, &client, group, &fetch, &factory, bundle).await
    })
    .await
}

struct FetchAdapter<'a>(&'a dyn UpgradeFetch);
impl crate::commands::multisig::UpgradeFetch for FetchAdapter<'_> {
    fn bytes<'a>(
        &'a self,
        url: &'a str,
        api: bool,
    ) -> rtao::signer::BoxFuture<'a, Result<Vec<u8>, CliError>> {
        self.0.bytes(url, api)
    }
}

fn render_checks(ctx: &AppContext, outcome: &chain::Outcome) -> Result<(), CliError> {
    let payload = outcome.payload();
    let fields = outcome
        .checks
        .iter()
        .map(|check| {
            (
                check.name.into(),
                json!(format!(
                    "{} {}",
                    match check.ok {
                        Some(true) => "✓",
                        Some(false) => "✗ FAIL:",
                        None => "– skipped:",
                    },
                    check.detail
                )),
            )
        })
        .collect::<Vec<_>>();
    ctx.output
        .detail(Some("upgrade check"), &fields, Some(&payload))
        .map_err(failure)
}
fn ensure_checks(outcome: &chain::Outcome) -> Result<(), CliError> {
    if outcome.ok() {
        Ok(())
    } else {
        Err(failure(
            "verification failed: ".to_owned()
                + &outcome
                    .checks
                    .iter()
                    .filter(|check| check.ok == Some(false))
                    .map(|check| check.name)
                    .collect::<Vec<_>>()
                    .join("; "),
        )
        .with_help("refusing to sign; resolve every failed check first"))
    }
}

async fn load_bundle(
    ctx: &AppContext,
    leaf: &ArgMatches,
    fetch: &dyn UpgradeFetch,
) -> Result<ProposalBundle, CliError> {
    ctx.output
        .message("fetching proposal artifacts…")
        .map_err(failure)?;
    let bundle = artifacts::fetch_proposal_bundle(&bundle_inputs(leaf), fetch)
        .await
        .map_err(failure)?;
    for note in &bundle.notes {
        ctx.output
            .message(&format!("note: {note}"))
            .map_err(failure)?;
    }
    Ok(bundle)
}

#[cfg(test)]
async fn execute_with(
    ctx: &AppContext,
    client: &Client,
    group: &ArgMatches,
    fetch: &dyn UpgradeFetch,
    factory: &dyn RawSignerFactory,
) -> Result<(), CliError> {
    validate_before_connect(group)?;
    let (name, leaf) = group
        .subcommand()
        .ok_or_else(|| CliError::input("missing upgrade command"))?;
    let bundle = if name == "pending" {
        None
    } else {
        Some(load_bundle(ctx, leaf, fetch).await?)
    };
    execute_loaded(ctx, client, group, fetch, factory, bundle).await
}

/// Execute through injectable artifact and signer boundaries. No node is started.
async fn execute_loaded(
    ctx: &AppContext,
    client: &Client,
    group: &ArgMatches,
    fetch: &dyn UpgradeFetch,
    factory: &dyn RawSignerFactory,
    bundle: Option<ProposalBundle>,
) -> Result<(), CliError> {
    let (name, leaf) = group
        .subcommand()
        .ok_or_else(|| CliError::input("missing upgrade command"))?;
    if name == "pending" {
        let snapshot = client.at(BlockRef::Best).await.map_err(failure)?;
        let sudo = snapshot
            .storage(&rtao_runtime::storage::sudo::KEY, ())
            .await
            .map_err(failure)?;
        let records = match sudo {
            Some(sudo) => {
                crate::commands::multisig::load_pending_upgrades_from_repo(
                    ctx,
                    &snapshot,
                    sudo,
                    &FetchAdapter(fetch),
                    text(leaf, "repo").unwrap_or("RaoFoundation/subtensor"),
                )
                .await?
            }
            None => Vec::new(),
        };
        if ctx.globals.json {
            return ctx
                .output
                .value(&json!({"pending_upgrades":records,"count":records.len()}))
                .map_err(failure);
        }
        if records.is_empty() {
            return ctx
                .output
                .detail(Some("pending runtime upgrades"), &[], None)
                .map_err(failure);
        }
        for (index, record) in records.iter().enumerate() {
            ctx.output
                .detail(
                    (index == 0).then_some("pending runtime upgrade"),
                    &crate::commands::multisig::upgrade_fields(record),
                    None,
                )
                .map_err(failure)?;
        }
        return Ok(());
    }
    let bundle = bundle.ok_or_else(|| failure("upgrade artifact bundle is missing"))?;
    let snapshot = client.at(BlockRef::Best).await.map_err(failure)?;
    let outcome = chain::verify(&snapshot, &bundle).await?;
    render_checks(ctx, &outcome)?;
    if name == "check" {
        reproduce_recipe(ctx, &bundle)?;
        return ensure_checks(&outcome);
    }
    ensure_checks(&outcome)?;
    if name != "sign" {
        return Err(CliError::input("unknown upgrade command"));
    }
    let group = resolve_group(ctx, leaf, bundle.manifest.as_ref())?;
    let multisig = Multisig::new(
        &snapshot.block().runtime,
        &group.signatories,
        group.threshold,
    )
    .map_err(failure)?;
    if multisig.address != outcome.sudo {
        return Err(failure(format!(
            "the resolved signer set derives {}, which is not the chain's sudo key {}",
            multisig.address, outcome.sudo
        ))
        .with_help("check the threshold and signatory list (or the saved multisig entry)"));
    }
    let mut selection = crate::call::select_member(ctx, &group, SignerRole::Coldkey, None)?;
    if selection.address.is_none() && selection.backend == "wallet" {
        selection.address = Some(ctx.wallet()?.coldkey_address().map_err(failure)?);
    }
    let signer = selection.address.ok_or_else(|| {
        CliError::input("upgrade signing needs a public member address; pass --signer-address")
    })?;
    if !group.signatories.contains(&signer) {
        return Err(failure(format!(
            "wallet {:?} coldkey {signer} is not in the sudo multisig signer set",
            ctx.globals.wallet_name
        )));
    }
    let pending = outcome
        .pending
        .as_ref()
        .ok_or_else(|| failure("verified proposal disappeared"))?;
    let finalizing = chain::finalizing(&snapshot.block().runtime, &bundle.blob, pending)?;
    let layer = chain::sudo_layer(&snapshot, outcome.sudo, finalizing.hash()).await?;
    let (position, prompt) =
        signing_position(layer.as_ref(), group.threshold, signer, &outcome.data);
    if position == "signed" {
        ctx.output.message(&prompt).map_err(failure)?;
        share_hint(ctx, text(leaf, "url"), layer.as_ref(), &group.signatories)?;
        return Ok(());
    }
    if bundle.wasm_source != Some("local") {
        ctx.output.message("note: the runtime was taken from the release, not a local srtool build — anyone reproducing the build should pass --wasm").map_err(failure)?;
    }
    if ctx.globals.dry_run {
        let payload = json!({"position":position,"action":prompt,"sudo_multisig":multisig.address,"finalizing_call_hash":format!("0x{}",hex::encode(finalizing.hash()))});
        return detail(ctx, "dry run: upgrade sign", payload);
    }
    ctx.output
        .transaction_card(&[
            ReviewSection {
                title: "Upgrade".into(),
                rows: vec![
                    ("action".into(), json!(prompt)),
                    ("position".into(), json!(position)),
                    (
                        "spec".into(),
                        outcome
                            .data
                            .get("manifest")
                            .and_then(|manifest| manifest.get("spec_version"))
                            .cloned()
                            .unwrap_or(json!("?")),
                    ),
                    ("sudo multisig".into(), json!(multisig.address)),
                ],
            },
            ReviewSection {
                title: "Fees".into(),
                rows: vec![("estimated fee".into(), json!("unavailable"))],
            },
            ReviewSection {
                title: "Signer".into(),
                rows: vec![
                    ("signer".into(), json!(signer)),
                    ("multisig".into(), json!(multisig.address)),
                    (
                        "threshold".into(),
                        json!(format!(
                            "{} of {}",
                            group.threshold,
                            group.signatories.len()
                        )),
                    ),
                ],
            },
            ReviewSection {
                title: "Transaction".into(),
                rows: vec![
                    ("operation".into(), json!("runtime-upgrade approval")),
                    (
                        "call hash".into(),
                        json!(format!("0x{}", hex::encode(finalizing.hash()))),
                    ),
                ],
            },
        ])
        .map_err(failure)?;
    ctx.confirm(&prompt)?;
    submit_verified(
        ctx,
        client,
        &bundle,
        &group,
        &selection,
        signer,
        outcome.sudo,
        finalizing.hash(),
        layer.as_ref().map(|layer| &layer.when),
        text(leaf, "url"),
        factory,
    )
    .await
}

fn detail(ctx: &AppContext, title: &str, payload: Value) -> Result<(), CliError> {
    let fields = payload
        .as_object()
        .ok_or_else(|| failure("detail payload is not an object"))?
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Vec<_>>();
    ctx.output
        .detail(Some(title), &fields, Some(&payload))
        .map_err(failure)
}

fn sign_command(ctx: &AppContext, url: &str) -> String {
    let prefix = if ctx.globals.network == "finney" {
        "btcli".into()
    } else {
        format!(
            "btcli -n {}",
            crate::prompt::shell_quote(&ctx.globals.network)
        )
    };
    format!(
        "{prefix} upgrade sign --url {} -w <your-wallet>",
        crate::prompt::shell_quote(url)
    )
}

fn share_hint(
    ctx: &AppContext,
    url: Option<&str>,
    layer: Option<&rtao_runtime::models::MultisigInfo>,
    members: &[AccountId],
) -> Result<(), CliError> {
    if ctx.globals.json {
        return Ok(());
    }
    let Some(url) = url else {
        return Ok(());
    };
    let remaining = members
        .iter()
        .filter(|member| layer.is_none_or(|layer| !layer.approvals.contains(member)))
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if !remaining.is_empty() {
        ctx.output
            .message(&format!("waiting on: {}", remaining.join(", ")))
            .map_err(failure)?;
        ctx.output
            .message(&format!("they can run: {}", sign_command(ctx, url)))
            .map_err(failure)?;
    }
    Ok(())
}

fn reproduce_recipe(ctx: &AppContext, bundle: &ProposalBundle) -> Result<(), CliError> {
    if ctx.globals.json || bundle.wasm_source == Some("local") {
        return Ok(());
    }
    let manifest = bundle.manifest.as_ref();
    let field = |name: &str| {
        manifest
            .and_then(|manifest| manifest.get(name))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
    };
    let repo = field("repo").unwrap_or("RaoFoundation/subtensor");
    let tag = field("tag").unwrap_or("<tag>");
    let rustc = field("srtool_rustc").unwrap_or("<rustc-tag>");
    ctx.output.message("").map_err(failure)?;
    ctx.output.message("to verify against code you compiled yourself, build the runtime with srtool and re-run with --wasm:").map_err(failure)?;
    let slash = "\\";
    for line in [
        format!(
            "git clone -- {} && cd -- {}",
            crate::prompt::shell_quote(&format!("https://github.com/{repo}")),
            crate::prompt::shell_quote(repo.split_once('/').map_or(repo, |(_, name)| name))
        ),
        format!(
            "git checkout --detach {} --",
            crate::prompt::shell_quote(&format!("refs/tags/{tag}"))
        ),
        "ln -s . runtime/node-subtensor".into(),
        format!("docker run --rm --user root --platform=linux/amd64 {slash}"),
        format!("  -e PACKAGE=node-subtensor-runtime {slash}"),
        format!("  -e BUILD_OPTS=\"--features=metadata-hash\" {slash}"),
        format!("  -e PROFILE=production {slash}"),
        format!("  -v \"$(pwd)\":/build {slash}"),
        format!(
            "  {} /srtool/build --app",
            crate::prompt::shell_quote(&format!("paritytech/srtool:{rustc}"))
        ),
    ] {
        ctx.output.message(&format!("  {line}")).map_err(failure)?;
    }
    Ok(())
}

fn resolve_group(
    ctx: &AppContext,
    matches: &ArgMatches,
    manifest: Option<&Value>,
) -> Result<RawMultisig, CliError> {
    let threshold = matches
        .try_get_one::<u16>("multisig_threshold")
        .ok()
        .flatten()
        .copied();
    if let Some(preset) = text(matches, "multisig").filter(|value| !value.is_empty()) {
        if threshold.is_some()
            || ["signatories", "other_signatories"]
                .into_iter()
                .any(|field| text(matches, field).is_some_and(|value| !value.is_empty()))
        {
            return Err(CliError::input(
                "use either --multisig NAME or inline multisig flags, not both",
            ));
        }
        return crate::call::resolve_preset(ctx, preset);
    }
    if let Some(threshold) = threshold {
        let full = text(matches, "signatories").filter(|value| !value.is_empty());
        let other = text(matches, "other_signatories").filter(|value| !value.is_empty());
        if full.is_some() && other.is_some() {
            return Err(CliError::input(
                "pass --signatories or --other-signatories, not both",
            ));
        }
        if full.is_none() && other.is_none() {
            return Err(CliError::input(
                "with --multisig-threshold, pass --signatories or --other-signatories",
            ));
        }
        let refs = split_signatory_refs(full.or(other).unwrap_or(""))
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut signatories = Vec::new();
        for reference in &refs {
            let account = ctx
                .resolve_address("coldkey_ss58", Some(reference))?
                .ok_or_else(|| CliError::input("cannot resolve signatory"))?;
            if !signatories.contains(&account) {
                signatories.push(account);
            }
        }
        if other.is_some() {
            let account = ctx
                .resolve_address("coldkey_ss58", None)?
                .ok_or_else(|| CliError::input("cannot resolve wallet coldkey"))?;
            if !signatories.contains(&account) {
                signatories.push(account);
            }
        }
        return Ok(RawMultisig {
            threshold,
            signatories,
            preset: None,
            refs,
        });
    }
    let sudo = manifest.and_then(|manifest| manifest.get("sudo"));
    let threshold = sudo
        .and_then(|sudo| sudo.get("threshold"))
        .and_then(Value::as_u64)
        .and_then(|threshold| u16::try_from(threshold).ok())
        .filter(|threshold| *threshold > 0);
    let signatories = sudo
        .and_then(|sudo| sudo.get("signatories"))
        .and_then(Value::as_array);
    if let (Some(threshold), Some(signatories)) = (
        threshold,
        signatories.filter(|signatories| !signatories.is_empty()),
    ) {
        let mut accounts = Vec::new();
        let mut refs = Vec::new();
        for value in signatories {
            let reference = value.as_str().ok_or_else(|| {
                CliError::input("manifest sudo signatory must be an SS58 address")
            })?;
            let account = reference.parse().map_err(failure)?;
            if !accounts.contains(&account) {
                accounts.push(account);
                refs.push(reference.to_owned());
            }
        }
        return Ok(RawMultisig {
            threshold,
            signatories: accounts,
            preset: None,
            refs,
        });
    }
    Err(failure(
        "cannot determine the sudo multisig signer set: the release manifest has no sudo block — pass `--multisig NAME` (a saved multisig) or `--multisig-threshold N --signatories a,b,c`",
    ))
}

fn signing_position(
    layer: Option<&rtao_runtime::models::MultisigInfo>,
    threshold: u16,
    signer: AccountId,
    data: &Map<String, Value>,
) -> (&'static str, String) {
    let spec = data
        .get("manifest")
        .and_then(|manifest| manifest.get("spec_version"))
        .filter(|value| !value.is_null())
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string())
        })
        .unwrap_or("?".into());
    if threshold == 1 {
        return (
            "final",
            format!(
                "submit the FINAL approval for runtime upgrade {spec} — the upgrade executes in this extrinsic"
            ),
        );
    }
    let Some(layer) = layer else {
        return (
            "first",
            format!(
                "open the sudo-multisig approval for runtime upgrade {spec} (approval 1 of {threshold})"
            ),
        );
    };
    if layer.approvals.contains(&signer) {
        return (
            "signed",
            format!(
                "your approval is already recorded ({} of {threshold} so far)",
                layer.approvals.len()
            ),
        );
    }
    if layer.approvals.len() >= usize::from(threshold.saturating_sub(1)) {
        return (
            "final",
            format!(
                "submit the FINAL approval for runtime upgrade {spec} — the upgrade executes in this extrinsic"
            ),
        );
    }
    (
        "interior",
        format!(
            "add approval {} of {threshold} for runtime upgrade {spec}",
            layer.approvals.len().saturating_add(1)
        ),
    )
}

#[allow(clippy::too_many_arguments)]
async fn submit_verified(
    ctx: &AppContext,
    client: &Client,
    bundle: &ProposalBundle,
    group: &RawMultisig,
    selection: &SigningSelection,
    signer: AccountId,
    reviewed_sudo: AccountId,
    reviewed_hash: [u8; 32],
    reviewed_when: Option<&rtao_runtime::models::MultisigTimepoint>,
    release_url: Option<&str>,
    factory: &dyn RawSignerFactory,
) -> Result<(), CliError> {
    // Re-read every trust check and both proposal identities after confirmation.
    // Advancing approvals is safe; replacing either opening requires a new review.
    let snapshot = client.at(BlockRef::Best).await.map_err(failure)?;
    let outcome = chain::verify(&snapshot, bundle).await?;
    ensure_checks(&outcome)?;
    if outcome.sudo != reviewed_sudo {
        return Err(failure(
            "the chain sudo key changed since verification; rerun upgrade sign",
        ));
    }
    let current_group = Multisig::new(
        &snapshot.block().runtime,
        &group.signatories,
        group.threshold,
    )
    .map_err(failure)?;
    if current_group.address != reviewed_sudo {
        return Err(failure(
            "the sudo multisig signer set changed since review; rerun upgrade sign",
        ));
    }
    let pending=outcome.pending.as_ref().ok_or_else(||failure("the deployment-multisig proposal disappeared from chain state (executed or cancelled since verification)"))?;
    let finalizing = chain::finalizing(&snapshot.block().runtime, &bundle.blob, pending)?;
    if finalizing.hash() != reviewed_hash {
        return Err(failure(
            "the deployment proposal opening changed since review; rerun upgrade sign",
        ));
    }
    let layer = chain::sudo_layer(&snapshot, reviewed_sudo, finalizing.hash()).await?;
    if let Some(expected) = reviewed_when
        && layer.as_ref().is_none_or(|layer| &layer.when != expected)
    {
        return Err(failure(
            "the sudo proposal opening changed since review; rerun upgrade sign",
        ));
    }
    if layer
        .as_ref()
        .is_some_and(|layer| layer.approvals.contains(&signer))
    {
        ctx.output
            .message("your approval is already recorded on-chain; nothing to submit")
            .map_err(failure)?;
        share_hint(ctx, release_url, layer.as_ref(), &group.signatories)?;
        return Ok(());
    }
    let estimate = client
        .estimate_weight(finalizing.clone(), signer, CryptoType::Sr25519)
        .await
        .map_err(failure)?;
    let padded = |name: &str| -> Result<u64, CliError> {
        estimate
            .weight
            .field(name)
            .and_then(ValueExt::as_u128)
            .and_then(|value| value.checked_mul(11))
            .map(|value| value / 10)
            .and_then(|value| u64::try_from(value).ok())
            .ok_or_else(|| failure(format!("estimated {name} exceeds native u64 weight")))
    };
    let weight = chain::weight(padded("ref_time")?, padded("proof_size")?);
    let mut others = group
        .signatories
        .iter()
        .copied()
        .filter(|account| *account != signer)
        .collect::<Vec<_>>();
    others.sort();
    others.dedup();
    let when = layer.as_ref().map(|layer| layer.when.clone());
    let call = if group.threshold == 1 {
        rtao_runtime::calls::multisig::as_multi_threshold_1(others, finalizing.clone())
    } else if layer
        .as_ref()
        .is_none_or(|layer| layer.approvals.len() < usize::from(group.threshold.saturating_sub(1)))
    {
        rtao_runtime::calls::multisig::approve_as_multi(
            group.threshold,
            others,
            when,
            finalizing.hash(),
            weight,
        )
    } else {
        rtao_runtime::calls::multisig::as_multi(
            group.threshold,
            others,
            when,
            finalizing.clone(),
            weight,
        )
    }
    .compose(&snapshot.block().runtime)
    .map_err(failure)?;
    // Acquiring a private-capable source starts only after public review and all
    // post-confirmation checks. Its lazy signer is invoked by the executor.
    let keys = factory.source(ctx, selection).await?;
    let result = async {
        let actual = keys
            .bare_signer_account()
            .or_else(|| keys.coldkey_address().ok())
            .ok_or_else(|| failure("signing source has no public coldkey"))?;
        if actual != signer {
            return Err(CliError::input(
                "signing source does not match the reviewed sudo signatory",
            ));
        }
        rtao::Executor::new(client.clone())
            .submit_call_dyn(
                call,
                keys.as_ref(),
                SignerRole::Coldkey,
                &ExecuteOptions {
                    wait_for_finalization: false,
                    ..Default::default()
                },
            )
            .await
            .map_err(failure)
    }
    .await;
    let _ = factory
        .release(
            selection,
            keys.as_ref(),
            result.as_ref().is_ok_and(|result| result.success),
        )
        .await;
    let result = result?;
    if !ctx
        .output
        .result(&result, "runtime-upgrade approval submitted")
        .map_err(failure)?
    {
        return Err(failure(""));
    }
    // A successful submission remains the authoritative outcome if follow-up
    // status is unavailable; never turn it into an invitation to submit again.
    let latest = match client.at(BlockRef::Best).await {
        Ok(latest) => chain::sudo_layer(&latest, reviewed_sudo, finalizing.hash()).await,
        Err(error) => Err(failure(error)),
    };
    match latest {
        Ok(Some(layer))=>{
            let mut fields=json!({"approvals":format!("{} of {}",layer.approvals.len(),group.threshold),"opened at":format!("{}:{}",layer.when.height,layer.when.index),"call_hash":format!("0x{}",hex::encode(finalizing.hash()))});
            let remaining=group.signatories.iter().filter(|account|!layer.approvals.contains(account)).map(ToString::to_string).collect::<Vec<_>>();
            if !remaining.is_empty(){
                fields["waiting on"]=json!(remaining.join(", "));
                if let Some(url)=release_url{fields["they run"]=json!(sign_command(ctx,url));}
            }
            detail(ctx,"sudo multisig status",fields)?;
        }
        Ok(None)=>ctx.output.message("the sudo multisig operation is complete — the upgrade should now execute; verify with `rtao misc upgrade pending` and the chain's spec version").map_err(failure)?,
        Err(_)=>ctx.output.message("approval submitted; follow-up status is unavailable — inspect `rtao misc upgrade pending` before taking another action").map_err(failure)?,
    }
    Ok(())
}

#[cfg(test)]
#[path = "upgrade/tests.rs"]
mod tests;
