// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/call.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/multisig_helpers.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/config.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Raw metadata-checked calls, immutable origin wrappers and multisig rounds.

use std::{collections::BTreeSet, path::PathBuf, sync::Arc};

use crate::prompt::shell_quote;
use futures::StreamExt;
use rtao::client::config::{ConfigFile, MAX_JSON_FILE_BYTES, Paths, read_bounded_json_file};
use rtao_runtime::models::{MultisigInfo, MultisigTimepoint};
use std::{collections::BTreeMap, path::Path, time::Duration};

use clap::{Arg, ArgAction, ArgMatches, Command};
use rtao::{
    client::{Client, Inclusion, View, ViewExt},
    core::{codec::Value as CoreValue, value_ext::ValueExt},
    primitives::{AccountId, CryptoType},
    signer::{BoxFuture, DynKeySource, SignerRole},
    tx::{
        ApproveOptions, ExecuteOptions, ExtrinsicResult, Multisig,
        multisig::{FundsRequest, check_multisig_funds},
    },
    wallet::Wallet,
};
use rtao_runtime::{Call, CallBytes, CallDescriptor, RuntimeHandle, models::ProxyType};
use serde_json::{Map, Value as Json, json};

use crate::{
    books::{
        AccountLookup, FsAccountLookup, NameResolver, is_bittensor_address,
        read_public_json_for_update, split_signatory_refs, write_public_json,
    },
    call_names::{GeneratedCallSchema, report_resolved_account, resolve_builder_params_with},
    context::AppContext,
    error::CliError,
    globals::{Tier, with_globals},
    output::{ReviewSection, StepState},
};

/// Raw escape-hatch command; it owns `--signer` as both a role and backend.
pub fn command() -> Command {
    with_globals(
        Command::new("call")
            .about("Submit any raw chain call; prefer tx for validated intents.")
            .arg(
                Arg::new("target")
                    .required(true)
                    .value_name("TARGET")
                    .help("Pallet.function, e.g. System.set_code."),
            )
            .arg(
                Arg::new("args_json")
                    .long("args")
                    .help("Call parameters as a JSON object (at most 16 MiB)."),
            )
            .arg(
                Arg::new("args_file")
                    .long("args-file")
                    .help("Read JSON parameters from a regular file (at most 16 MiB)."),
            )
            .arg(
                Arg::new("sudo")
                    .long("sudo")
                    .action(ArgAction::SetTrue)
                    .help("Wrap the raw call in Sudo.sudo."),
            )
            .arg(
                Arg::new("multisig")
                    .long("multisig")
                    .help("Named saved multisig."),
            )
            .arg(
                Arg::new("multisig_threshold")
                    .long("multisig-threshold")
                    .value_parser(clap::value_parser!(u16))
                    .help("Multisig approvals required."),
            )
            .arg(
                Arg::new("signatories")
                    .long("signatories")
                    .help("Full signer set, comma/space separated."),
            )
            .arg(
                Arg::new("other_signatories")
                    .long("other-signatories")
                    .help("Other signer set; add this wallet's selected key."),
            )
            .arg(
                Arg::new("signer_backend")
                    .long("signer")
                    .alias("key")
                    .default_value("coldkey")
                    .value_parser([
                        "coldkey",
                        "hotkey",
                        "wallet",
                        "vault",
                        "ledger",
                        "extension",
                        "remote",
                    ])
                    .help("Key role or signing backend."),
            ),
        Tier::Tx,
    )
}

/// Public signer selection; acquiring this source must not unlock its key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SigningSelection {
    /// Local wallet name, or a public external member label.
    pub wallet: String,
    /// Optional public approval label, independent of the selected wallet file.
    pub display_name: Option<String>,
    /// `wallet`, `vault`, `ledger`, or `extension`.
    pub backend: String,
    /// Expected external or multisig member account, when known.
    pub address: Option<AccountId>,
}

impl SigningSelection {
    /// Public member label for review and progress, never key acquisition.
    pub fn display_wallet(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.wallet)
    }
}

/// Resolved multisig settings, preserving preset order for labels and followup.
#[derive(Clone, Debug)]
pub struct RawMultisig {
    /// Approval threshold, revalidated against live runtime caps.
    pub threshold: u16,
    /// Distinct members in original resolution order.
    pub signatories: Vec<AccountId>,
    /// Saved preset used by replay commands.
    pub preset: Option<String>,
    /// Original wallet/contact references for portable replay labels.
    pub refs: Vec<String>,
}

/// Public origin and replay data for a composed raw or semantic multisig call.
/// An unknown target is represented by `None` and is never written to the cache.
#[derive(Clone, Debug)]
pub struct MultisigCallSpec {
    /// Decoded `Pallet.function`, when available.
    pub target: Option<String>,
    /// Original decoded parameters, retaining exact integer values.
    pub params: Map<String, Json>,
    /// Original parameter file for portable replay, when available.
    pub args_file: Option<PathBuf>,
    /// Whether Sudo was applied inside Proxy.
    pub sudo: bool,
    /// Resolved Proxy dispatch account.
    pub proxy_for: Option<AccountId>,
    /// Optional forced Proxy permission.
    pub force_proxy_type: Option<ProxyType>,
    /// Role used by the approving member.
    pub role: SignerRole,
}

impl From<&RawCallRequest> for MultisigCallSpec {
    fn from(request: &RawCallRequest) -> Self {
        Self {
            target: Some(request.target.clone()),
            params: request.params.clone(),
            args_file: request.args_file.clone(),
            sudo: request.sudo,
            proxy_for: request.proxy_for,
            force_proxy_type: request.force_proxy_type,
            role: request.role,
        }
    }
}

impl MultisigCallSpec {
    /// Retain an explicitly unknown decoded target without inventing replay data.
    pub fn unknown(role: SignerRole) -> Self {
        Self {
            target: None,
            params: Map::new(),
            args_file: None,
            sudo: false,
            proxy_for: None,
            force_proxy_type: None,
            role,
        }
    }
}

/// Decode actual composed bytes for semantic followup, unwrapping Proxy and Sudo.
/// Failed or unrecognized cosmetic decoding returns an explicit unknown spec;
/// the caller must retain the original [`CallBytes`] for approval and receipts.
pub async fn decode_multisig_call_spec(
    client: &Client,
    call: &CallBytes,
    role: SignerRole,
) -> MultisigCallSpec {
    let decoded = match client.decode_scale("Call", call.as_bytes()).await {
        Ok(value) => value.to_json(),
        Err(_) => return MultisigCallSpec::unknown(role),
    };
    decoded_call_spec(&decoded, role, 0).unwrap_or_else(|| MultisigCallSpec::unknown(role))
}

/// Decode cosmetic followup details using the pending operation's pinned runtime.
pub(crate) fn decode_multisig_call_spec_at(
    runtime: &RuntimeHandle,
    call: &CallBytes,
    role: SignerRole,
) -> MultisigCallSpec {
    if call.ensure_runtime(runtime).is_err() {
        return MultisigCallSpec::unknown(role);
    }
    let Ok(decoded) =
        runtime
            .core()
            .decode_spec(&rtao::core::runtime::TypeSpec::Call, call.as_bytes(), true)
    else {
        return MultisigCallSpec::unknown(role);
    };
    let decoded = rtao::core::DynValue::new(decoded).to_json();
    decoded_call_spec(&decoded, role, 0).unwrap_or_else(|| MultisigCallSpec::unknown(role))
}

fn decoded_call_spec(value: &Json, role: SignerRole, depth: usize) -> Option<MultisigCallSpec> {
    if depth >= 128 {
        return None;
    }
    let module = value.get("call_module")?.as_str()?;
    let function = value.get("call_function")?.as_str()?;
    let params: Map<String, Json> = value
        .get("call_args")?
        .as_array()?
        .iter()
        .filter_map(|arg| {
            Some((
                arg.get("name")?.as_str()?.to_owned(),
                arg.get("value")?.clone(),
            ))
        })
        .collect();
    if (module, function) == ("Sudo", "sudo") {
        let mut spec = decoded_call_spec(params.get("call")?, role, depth + 1)?;
        spec.sudo = true;
        return Some(spec);
    }
    if (module, function) == ("Proxy", "proxy") {
        let real = params.get("real")?;
        let address = real
            .as_str()
            .or_else(|| {
                let object = real.as_object().filter(|object| object.len() == 1)?;
                object.values().next()?.as_str()
            })?
            .parse()
            .ok()?;
        let mut spec = decoded_call_spec(params.get("call")?, role, depth + 1)?;
        spec.proxy_for = Some(address);
        spec.force_proxy_type = params.get("force_proxy_type").and_then(|value| {
            let name = value.as_str().or_else(|| {
                let object = value.as_object().filter(|object| object.len() == 1)?;
                object.keys().next().map(String::as_str)
            })?;
            serde_json::from_value(json!(name)).ok()
        });
        return Some(spec);
    }
    if module.is_empty() || function.is_empty() {
        return None;
    }
    Some(MultisigCallSpec {
        target: Some(format!("{module}.{function}")),
        params,
        role,
        ..MultisigCallSpec::unknown(role)
    })
}

#[cfg(test)]
mod pinned_spec_tests {
    use super::*;

    #[test]
    fn pinned_followup_retains_semantic_role_and_refuses_foreign_runtime_stamps()
    -> Result<(), Box<dyn std::error::Error>> {
        let metadata = include_bytes!("../../../vendor/metadata/finney/478.v15.scale");
        let runtime = RuntimeHandle::parse(metadata, 478, 1, 42)?;
        let call = rtao_runtime::calls::system::remark(vec![1, 2]).compose(&runtime)?;
        let wrapped = rtao_runtime::calls::sudo::sudo(call).compose(&runtime)?;
        let spec = decode_multisig_call_spec_at(&runtime, &wrapped, SignerRole::Hotkey);
        assert_eq!(spec.target.as_deref(), Some("System.remark"));
        assert_eq!(spec.role, SignerRole::Hotkey);
        assert!(spec.sudo);
        assert!(spec.params.contains_key("remark"));
        let upgraded = RuntimeHandle::parse(metadata, 479, 1, 42)?;
        let refused = decode_multisig_call_spec_at(&upgraded, &wrapped, SignerRole::Coldkey);
        assert!(refused.target.is_none());
        assert_eq!(refused.role, SignerRole::Coldkey);
        Ok(())
    }

    #[test]
    fn multisig_wallet_alias_order_is_natural_and_keeps_equal_name_ties_stable() {
        use crate::books::{NameCandidate, ResolveError};

        struct Lookup(Vec<NameCandidate>);
        impl AccountLookup for Lookup {
            fn coldkey(&self, _: &str) -> Result<String, ResolveError> {
                Err(ResolveError::new(
                    "this fixture exposes public candidates only",
                ))
            }
            fn hotkey(&self, _: &str, _: &str) -> Result<String, ResolveError> {
                Err(ResolveError::new(
                    "this fixture exposes public candidates only",
                ))
            }
            fn candidates(&self) -> Vec<NameCandidate> {
                self.0.clone()
            }
        }
        let first = AccountId([7; 32]);
        let second = AccountId([8; 32]);
        let outsider = AccountId([9; 32]);
        for equal_names in [["wallet02", "wallet2"], ["wallet2", "wallet02"]] {
            let candidate = |name: &str, account: AccountId, source: &str| NameCandidate {
                name: name.into(),
                address: account.to_string(),
                source: source.into(),
            };
            let lookup = Lookup(vec![
                candidate("wallet10", first, "wallet"),
                candidate(equal_names[0], first, "wallet"),
                candidate("wallet1", second, "wallet"),
                candidate(equal_names[1], first, "wallet"),
                candidate("wallet0", first, "hotkey"),
                candidate("wallet4", outsider, "wallet"),
                NameCandidate {
                    name: "wallet9".into(),
                    address: "invalid-public-address".into(),
                    source: "wallet".into(),
                },
            ]);
            assert_eq!(
                local_multisig_wallets(&lookup, &[first, second]),
                vec![
                    (equal_names[0].to_owned(), first),
                    (equal_names[1].to_owned(), first),
                    ("wallet10".to_owned(), first),
                    ("wallet1".to_owned(), second),
                ]
            );
        }
    }
}

/// One immutable composed call and its public multisig followup inputs.
pub struct MultisigFollowup<'a> {
    /// Optional decoded call and replay data.
    pub spec: &'a MultisigCallSpec,
    /// Resolved members and original labels.
    pub group: &'a RawMultisig,
    /// Live-runtime-validated multisig identity.
    pub multisig: &'a Multisig,
    /// Exact bytes approved by every round.
    pub call: &'a CallBytes,
    /// Receipt used to detect execution even when inner dispatch failed.
    pub result: &'a ExtrinsicResult,
}

/// Fully resolved public raw-call input, with no acquired private signer.
#[derive(Clone, Debug)]
pub struct RawCallRequest {
    /// Exact `Pallet.function` target.
    pub target: String,
    /// Account-resolved original JSON parameters.
    pub params: Map<String, Json>,
    /// Parameter-file path retained for co-signer replay.
    pub args_file: Option<PathBuf>,
    /// Wrap in Sudo before wrapping in Proxy.
    pub sudo: bool,
    /// Selected coldkey or hotkey role.
    pub role: SignerRole,
    /// External backend override, otherwise use the context's configured backend.
    pub backend: Option<String>,
    /// Outer Proxy dispatch identity, already resolved to account bytes.
    pub proxy_for: Option<AccountId>,
    /// Optional exact Proxy permission enum.
    pub force_proxy_type: Option<ProxyType>,
    /// Optional resolved multisig signer set.
    pub multisig: Option<RawMultisig>,
    /// Ordered signing members; one member for an ordinary call.
    pub rounds: Vec<SigningSelection>,
}

impl RawCallRequest {
    /// Resolve command-owned flags and public references before network/signing.
    ///
    /// The caller applies ordinary globals while reserving `signer_backend`.
    pub fn from_matches(ctx: &AppContext, matches: &ArgMatches) -> Result<Self, CliError> {
        let target = matches
            .get_one::<String>("target")
            .ok_or_else(|| CliError::input("missing TARGET"))?
            .clone();
        resolve_builder(&target)?;
        let selected = matches
            .get_one::<String>("signer_backend")
            .map(String::as_str)
            .unwrap_or("coldkey");
        let (role, backend) = match selected {
            "coldkey" => (SignerRole::Coldkey, None),
            "hotkey" => (SignerRole::Hotkey, None),
            "wallet" => (SignerRole::Coldkey, None),
            "vault" | "ledger" | "extension" | "remote" => {
                (SignerRole::Coldkey, Some(selected.to_owned()))
            }
            _ => {
                return Err(CliError::input(
                    "--signer must be coldkey, hotkey, wallet, vault, ledger, extension, or remote",
                ));
            }
        };
        let args_file = matches
            .get_one::<String>("args_file")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from);
        let params = load_params(
            matches.get_one::<String>("args_json").map(String::as_str),
            args_file.as_deref(),
        )?;
        let lookup = FsAccountLookup::new(ctx.globals.wallet_path.clone()).map_err(public_input)?;
        let names = NameResolver::new(
            &ctx.books,
            &lookup,
            &ctx.globals.wallet_name,
            &ctx.globals.hotkey_name,
        );
        let hint = if args_file.is_some() {
            "--args-file"
        } else {
            "--args"
        };
        let mut output_error = None;
        let params = resolve_builder_params_with(
            &names,
            &GeneratedCallSchema,
            &target,
            &params,
            hint,
            &mut |param, resolved| {
                if output_error.is_none() {
                    output_error = report_resolved_account(&ctx.output, param, resolved).err();
                }
            },
        );
        if let Some(error) = output_error {
            return Err(public_failure(error));
        }
        let params = params.map_err(public_input)?;
        let proxy_for = ctx.dispatch_proxy(None)?;
        let force_proxy_type = ctx
            .globals
            .force_proxy_type
            .as_deref()
            .map(|name| serde_json::from_value(json!(name)).map_err(public_input))
            .transpose()?;
        if force_proxy_type.is_some() && proxy_for.is_none() {
            return Err(CliError::input("--force-proxy-type requires --proxy-for"));
        }
        let threshold = matches.get_one::<u16>("multisig_threshold").copied();
        let full = matches.get_one::<String>("signatories").map(String::as_str);
        let others = matches
            .get_one::<String>("other_signatories")
            .map(String::as_str);
        let inline = threshold.is_some()
            || full.is_some_and(|v| !v.is_empty())
            || others.is_some_and(|v| !v.is_empty());
        let preset = matches
            .get_one::<String>("multisig")
            .filter(|preset| !preset.is_empty())
            .cloned()
            .or_else(|| {
                (!inline && ctx.books.multisig(&ctx.globals.wallet_name).is_some())
                    .then(|| ctx.globals.wallet_name.clone())
            });
        if preset.is_some() && inline {
            return Err(CliError::input(
                "use either --multisig NAME or inline multisig flags, not both",
            ));
        }
        let multisig = if let Some(preset) = preset {
            Some(resolve_preset_with(&names, ctx, &preset)?)
        } else if let Some(threshold) = threshold {
            if threshold == 0 {
                return Err(CliError::input("threshold must be >= 1"));
            }
            if full.is_some_and(|v| !v.is_empty()) && others.is_some_and(|v| !v.is_empty()) {
                return Err(CliError::input(
                    "pass either --signatories or --other-signatories, not both",
                ));
            }
            let raw = full
                .filter(|v| !v.is_empty())
                .or(others.filter(|v| !v.is_empty()))
                .ok_or_else(|| {
                    CliError::input(
                        "with --multisig-threshold, pass --signatories or --other-signatories",
                    )
                })?;
            let mut refs: Vec<_> = split_signatory_refs(raw)
                .into_iter()
                .map(str::to_owned)
                .collect();
            let mut signatories = resolve_members(&names, &refs)?;
            if others.is_some_and(|v| !v.is_empty()) {
                let self_address = match role {
                    SignerRole::Coldkey => lookup.coldkey(&ctx.globals.wallet_name),
                    SignerRole::Hotkey => {
                        lookup.hotkey(&ctx.globals.wallet_name, &ctx.globals.hotkey_name)
                    }
                }
                .map_err(public_input)?
                .parse()
                .map_err(public_input)?;
                if !signatories.contains(&self_address) {
                    signatories.push(self_address);
                }
                refs.push(ctx.globals.wallet_name.clone());
            }
            Some(RawMultisig {
                threshold,
                signatories,
                preset: None,
                refs,
            })
        } else {
            None
        };
        if let Some(group) = &multisig
            && group.signatories.len() < usize::from(group.threshold)
        {
            return Err(CliError::input(format!(
                "need at least {} signatories, got {}",
                group.threshold,
                group.signatories.len()
            )));
        }
        if multisig.is_none() && !ctx.globals.signatories.is_empty() {
            return Err(CliError::input(
                "--signatory only applies when dispatching through a multisig",
            ));
        }
        let mut request = Self {
            target,
            params,
            args_file,
            sudo: matches.get_flag("sudo"),
            role,
            backend,
            proxy_for,
            force_proxy_type,
            multisig,
            rounds: Vec::new(),
        };
        request.rounds = signing_rounds(
            ctx,
            &lookup,
            &names,
            request.multisig.as_ref(),
            request.role,
            request.backend.as_deref(),
            true,
        )?;
        // Required-shield refusal is an input error before connecting or unlocking.
        let (default, required) = shield_policy_for_target(&request.target);
        ctx.resolve_mev_shield(default, required, &request.target)?;
        Ok(request)
    }
}

/// Read at most 16 MiB from exactly one input; preserve all integer digits.
pub fn load_params(
    args: Option<&str>,
    args_file: Option<&std::path::Path>,
) -> Result<Map<String, Json>, CliError> {
    let args_file = args_file.filter(|path| !path.as_os_str().is_empty());
    if args.is_some_and(|args| !args.is_empty()) && args_file.is_some() {
        return Err(CliError::input(
            "pass either --args or --args-file, not both",
        ));
    }
    if args.is_some_and(|args| args.len() > MAX_JSON_FILE_BYTES) {
        return Err(CliError::input("JSON input exceeds 16 MiB limit"));
    }
    let file = args_file
        .map(|path| {
            read_bounded_json_file(path)
                .and_then(|bytes| {
                    String::from_utf8(bytes).map_err(|error| {
                        std::io::Error::new(std::io::ErrorKind::InvalidData, error)
                    })
                })
                .map_err(|error| CliError::input(format!("cannot read {path:?}: {error}")))
        })
        .transpose()?;
    let raw = file.as_deref().or(args).unwrap_or("");
    if crate::prompt::python_strip(raw).is_empty() {
        return Ok(Map::new());
    }
    match rtao::primitives::json::from_str::<Json>(raw)
        .map_err(|error| CliError::input(format!("invalid JSON: {error}")))?
    {
        Json::Object(params) => Ok(params),
        _ => Err(CliError::input(
            "call parameters must be a JSON object (name -> value)",
        )),
    }
}

/// Lookup one of the pinned generated builders, preserving source diagnostics.
pub fn resolve_builder(target: &str) -> Result<&'static CallDescriptor, CliError> {
    let (pallet, function) = target
        .split_once('.')
        .filter(|(_, function)| !function.is_empty())
        .ok_or_else(|| CliError::input(format!("expected Pallet.function, got {target:?}")))?;
    if !rtao_runtime::calls::ALL
        .iter()
        .any(|descriptor| descriptor.pallet == pallet)
    {
        return Err(CliError::input(format!("unknown pallet {pallet:?}")));
    }
    rtao_runtime::calls::ALL
        .iter()
        .copied()
        .find(|descriptor| descriptor.pallet == pallet && descriptor.name == function)
        .ok_or_else(|| CliError::input(format!("unknown call {function:?} in pallet {pallet:?}")))
}

/// Match shield defaults/requirements against all pinned Python intent wrappers.
pub fn shield_policy_for_target(target: &str) -> (bool, bool) {
    let Some((pallet, function)) = target.split_once('.') else {
        return (false, false);
    };
    let mut default = false;
    for spec in &rtao_catalog::INTENTS {
        if !spec.wraps.contains(&(pallet, function)) {
            continue;
        }
        if spec.mev_shield_required {
            return (true, true);
        }
        default |= spec.mev_shield_default;
    }
    (default, false)
}

/// Compose exactly once, with Sudo inside Proxy and both runtime stamps checked.
pub fn compose(runtime: &RuntimeHandle, request: &RawCallRequest) -> Result<CallBytes, CliError> {
    let mut call = compose_target(runtime, &request.target, &request.params, 0)?;
    if request.sudo {
        call = rtao::client::compose::sudo(runtime, call).map_err(public_input)?;
    }
    if let Some(proxy) = request.proxy_for {
        call = rtao::client::compose::proxy(runtime, proxy, request.force_proxy_type, call)
            .map_err(public_input)?;
    } else if request.force_proxy_type.is_some() {
        return Err(CliError::input("--force-proxy-type requires --proxy-for"));
    }
    Ok(call)
}

fn compose_target(
    runtime: &RuntimeHandle,
    target: &str,
    params: &Map<String, Json>,
    depth: usize,
) -> Result<CallBytes, CliError> {
    let descriptor = resolve_builder(target)?;
    if let Some(unknown) = params.keys().find(|name| {
        !descriptor
            .fields
            .iter()
            .any(|field| field.name == name.as_str())
    }) {
        return Err(CliError::input(format!(
            "unknown parameter {unknown:?} for {target}"
        )));
    }
    let args = params
        .iter()
        .map(|(name, value)| {
            Ok((
                CoreValue::Str(name.clone()),
                json_to_core(runtime, value, depth)?,
            ))
        })
        .collect::<Result<Vec<_>, CliError>>()?;
    Call::new(descriptor, CoreValue::Dict(args), Vec::new())
        .compose(runtime)
        .map_err(public_input)
}

fn json_to_core(
    runtime: &RuntimeHandle,
    value: &Json,
    depth: usize,
) -> Result<CoreValue, CliError> {
    if depth >= 128 {
        return Err(CliError::input(
            "call JSON nesting exceeds parser's 128-level bound",
        ));
    }
    let next = depth.saturating_add(1);
    match value {
        Json::Null => Ok(CoreValue::Null),
        Json::Bool(value) => Ok(CoreValue::Bool(*value)),
        Json::String(value) => Ok(CoreValue::Str(value.clone())),
        Json::Number(number) => number
            .as_i128()
            .map(CoreValue::Int)
            .or_else(|| number.as_u128().map(CoreValue::Uint))
            .map(Ok)
            .unwrap_or_else(|| decimal_u256(number.as_str()).map(CoreValue::U256)),
        Json::Array(items) => items
            .iter()
            .map(|item| json_to_core(runtime, item, next))
            .collect::<Result<Vec<_>, _>>()
            .map(CoreValue::List),
        Json::Object(object) => {
            if let Some((target, params)) = nested_spec(object) {
                return compose_target(runtime, &target, &params, next)
                    .map(|call| CoreValue::Bytes(call.as_bytes().to_vec()));
            }
            object
                .iter()
                .map(|(key, value)| {
                    Ok((
                        CoreValue::Str(key.clone()),
                        json_to_core(runtime, value, next)?,
                    ))
                })
                .collect::<Result<Vec<_>, _>>()
                .map(CoreValue::Dict)
        }
    }
}

fn nested_spec(object: &Map<String, Json>) -> Option<(String, Map<String, Json>)> {
    for keys in [
        ["module", "function", "params"],
        ["call_module", "call_function", "call_args"],
    ] {
        if let (Some(module), Some(function)) = (
            object.get(keys[0]).and_then(Json::as_str),
            object.get(keys[1]).and_then(Json::as_str),
        ) {
            let target = format!("{module}.{function}");
            let params = match object.get(keys[2]) {
                Some(Json::Object(params)) => params.clone(),
                None | Some(Json::Null) => Map::new(),
                _ => return None,
            };
            return Some((target, params));
        }
    }
    if object.len() == 1 {
        let (module, functions) = object.iter().next()?;
        let functions = functions
            .as_object()
            .filter(|functions| functions.len() == 1)?;
        let (function, params) = functions.iter().next()?;
        let target = format!("{module}.{function}");
        if resolve_builder(&target).is_ok() {
            return Some((target, params.as_object()?.clone()));
        }
    }
    None
}

fn decimal_u256(digits: &str) -> Result<[u8; 32], CliError> {
    if digits.is_empty() || !digits.bytes().all(|digit| digit.is_ascii_digit()) {
        return Err(CliError::input(
            "raw SCALE numbers must be integers in their runtime type's range",
        ));
    }
    let mut bytes = [0u8; 32];
    for digit in digits.bytes() {
        let mut carry = u16::from(digit - b'0');
        for byte in &mut bytes {
            let value = u16::from(*byte) * 10 + carry;
            *byte = (value & 0xff) as u8;
            carry = value >> 8;
        }
        if carry != 0 {
            return Err(CliError::input(
                "raw integer exceeds unsigned 256-bit range",
            ));
        }
    }
    Ok(bytes)
}

/// Sources for per-round local/external signing, acquired only after confirmation.
pub trait RawSignerFactory: Send + Sync {
    /// Attach the reviewed label before this lazy source acquires its UI.
    fn set_summary(&self, _selection: &SigningSelection, _keys: &dyn DynKeySource, _summary: &str) {
    }
    /// Resolve public identity for review without acquiring a signing session.
    /// Ordinary previews bypass this hook; pinned accounts need no backend I/O.
    fn public_selection<'a>(
        &'a self,
        _ctx: &'a AppContext,
        selection: &'a SigningSelection,
    ) -> BoxFuture<'a, Result<SigningSelection, CliError>> {
        Box::pin(async move { Ok(selection.clone()) })
    }
    /// Resolve a public source for this selected member, without unlocking it.
    fn source<'a>(
        &'a self,
        ctx: &'a AppContext,
        selection: &'a SigningSelection,
    ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, CliError>>;
    /// Report outcome and release an external session, on success and failure.
    fn release<'a>(
        &'a self,
        _selection: &'a SigningSelection,
        _source: &'a dyn DynKeySource,
        _success: bool,
    ) -> BoxFuture<'a, Result<(), CliError>> {
        Box::pin(async { Ok(()) })
    }
}

/// Local wallet factory; external backends require an installed signing factory.
#[derive(Clone, Copy, Debug, Default)]
pub struct WalletSignerFactory;

impl RawSignerFactory for WalletSignerFactory {
    fn source<'a>(
        &'a self,
        ctx: &'a AppContext,
        selection: &'a SigningSelection,
    ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, CliError>> {
        Box::pin(async move {
            if crate::globals::normalize_signer_backend(&selection.backend) != "wallet" {
                return Err(CliError::input(format!(
                    "{} signing backend has no installed CLI factory",
                    selection.backend
                )));
            }
            let wallet = Wallet::new(
                &selection.wallet,
                &ctx.globals.hotkey_name,
                &ctx.globals.wallet_path,
            )
            .map_err(public_failure)?
            .with_password_sources(ctx.passwords.clone());
            Ok(Arc::new(crate::context::CliWalletKeys::new(
                wallet,
                ctx.output.clone(),
            )) as Arc<dyn DynKeySource>)
        })
    }
}

/// Run the actual raw command with the selected lazy backend and shared cache.
pub async fn run(ctx: &mut AppContext, matches: &ArgMatches, replay: &str) -> Result<(), CliError> {
    let request = RawCallRequest::from_matches(ctx, matches)?;
    let cache = ctx
        .config_store
        .as_ref()
        .map(|store| FsRawCallCache::new(store.paths()))
        .map(Ok)
        .unwrap_or_else(FsRawCallCache::from_process)?;
    let factory = crate::signer_backends::NativeSignerFactory::new(ctx);
    run_with(ctx, &request, replay, &factory, &cache).await
}

/// Run with injectable signing backends and cache persistence.
pub async fn run_with(
    ctx: &AppContext,
    request: &RawCallRequest,
    replay: &str,
    factory: &dyn RawSignerFactory,
    cache: &dyn RawCallCache,
) -> Result<(), CliError> {
    ctx.run(|client| async move {
        execute_rounds_with(ctx, &client, request, replay, factory, cache)
            .await
            .map(|_| ())
    })
    .await
}

/// Execute one raw-call source through actual production compose/submit paths.
///
/// Useful for callers that already own a source; source unlock remains lazy.
pub async fn execute_with(
    ctx: &AppContext,
    client: &Client,
    keys: &dyn DynKeySource,
    request: &RawCallRequest,
    replay: &str,
) -> Result<Option<ExtrinsicResult>, CliError> {
    validate_rounds(request)?;
    let runtime = View::runtime(client).await.map_err(CliError::from)?;
    let call = compose(&runtime, request)?;
    let executor = rtao::Executor::new(client.clone());
    let multisig = request
        .multisig
        .as_ref()
        .map(|group| {
            Multisig::new(&runtime, &group.signatories, group.threshold).map_err(public_input)
        })
        .transpose()?;
    let (default, required) = shield_policy_for_target(&request.target);
    let shield = ctx.resolve_mev_shield(default, required, &request.target)?;
    if ctx.globals.dry_run {
        show_dry_run(ctx, request, &multisig, shield.enabled, replay)?;
        return Ok(None);
    }
    show_raw_review(ctx, request, shield.enabled)?;
    ctx.confirm(&confirmation(request, shield.enabled))?;
    let mut result = approve_or_submit(
        ctx,
        &executor,
        keys,
        request,
        &multisig,
        call.clone(),
        RawShield {
            choice: shield,
            required,
        },
    )
    .await?;
    if let Some(group) = &multisig {
        let cache = ctx
            .config_store
            .as_ref()
            .map(|store| FsRawCallCache::new(store.paths()))
            .map(Ok)
            .unwrap_or_else(FsRawCallCache::from_process)
            .map_err(|error| {
                crate::context::known_receipt_error(continuation_failure(error, &call), &result)
            })?;
        let followup = multisig_followup(ctx, client, request, group, &call, &result, &cache)
            .await
            .map_err(|error| {
                crate::context::known_receipt_error(continuation_failure(error, &call), &result)
            })?;
        result.data.insert("multisig_followup".into(), followup);
    }
    if !ctx
        .output
        .result(&result, &success_message(request, shield.enabled))
        .map_err(|error| crate::context::known_receipt_error(public_failure(error), &result))?
    {
        return Err(CliError::failure(""));
    }
    Ok(Some(result))
}

/// Execute ordered approvals, reusing one composed call and stopping on execution.
pub async fn execute_rounds_with(
    ctx: &AppContext,
    client: &Client,
    request: &RawCallRequest,
    replay: &str,
    factory: &dyn RawSignerFactory,
    cache: &dyn RawCallCache,
) -> Result<Option<ExtrinsicResult>, CliError> {
    validate_rounds(request)?;
    let runtime = View::runtime(client).await.map_err(CliError::from)?;
    let call = compose(&runtime, request)?;
    let executor = rtao::Executor::new(client.clone());
    let multisig = request
        .multisig
        .as_ref()
        .map(|group| {
            Multisig::new(&runtime, &group.signatories, group.threshold).map_err(public_input)
        })
        .transpose()?;
    let (default, required) = shield_policy_for_target(&request.target);
    let shield = ctx.resolve_mev_shield(default, required, &request.target)?;
    if ctx.globals.dry_run {
        show_dry_run(ctx, request, &multisig, shield.enabled, replay)?;
        return Ok(None);
    }
    if request.rounds.is_empty() {
        return Err(CliError::input("no selected signing member"));
    }
    let mut reviewed = request.clone();
    reviewed.rounds.clear();
    for selection in &request.rounds {
        reviewed
            .rounds
            .push(factory.public_selection(ctx, selection).await?);
    }
    let request = &reviewed;
    show_raw_review(ctx, request, shield.enabled)?;
    ctx.confirm(&confirmation(request, shield.enabled))?;
    let mut last: Option<ExtrinsicResult> = None;
    let mut opening: Option<MultisigTimepoint> = None;
    for (index, selection) in request.rounds.iter().enumerate() {
        let mut continuation = None;
        if let Some(result) = &last {
            if multisig_executed(result) {
                ctx.output.message(&format!("the call executed after {index} approval(s) — skipping the remaining signatories")).map_err(public_failure)?;
                break;
            }
            if !result.success {
                break;
            }
            if let Some(group) = &multisig {
                let observed = approval_timepoint(result, group, &call)
                    .map_err(|error| continuation_failure(error, &call))?;
                if opening
                    .as_ref()
                    .is_some_and(|expected| *expected != observed)
                {
                    return Err(continuation_failure(
                        CliError::failure("multisig approval receipt changed the original opening"),
                        &call,
                    ));
                }
                opening = Some(observed.clone());
                continuation = Some(
                    prepare_continuation(client, group, &call, selection, observed)
                        .await
                        .map_err(|error| continuation_failure(error, &call))?,
                );
            }
        }
        ctx.output
            .step(
                &format!("approval {}", index.saturating_add(1)),
                &format!("{} via {}", selection.display_wallet(), selection.backend),
                StepState::Running,
            )
            .map_err(public_failure)?;
        let source = factory.source(ctx, selection).await.map_err(|error| {
            if index > 0 {
                continuation_failure(error, &call)
            } else {
                error
            }
        })?;
        let result = async {
            factory.set_summary(
                selection,
                source.as_ref(),
                &raw_label(request, shield.enabled),
            );
            if let Some(expected) = selection.address
                && public_signer(source.as_ref(), request.role)? != expected
            {
                return Err(CliError::input(
                    "selected signing source does not match the public member account",
                ));
            }
            if let Some(continuation) = continuation {
                let use_shield =
                    honor_raw_shield(ctx, client, shield, required, &request.target).await?;
                return executor
                    .submit_call_dyn(
                        continuation,
                        source.as_ref(),
                        request.role,
                        &ExecuteOptions {
                            shielded: use_shield,
                            period: use_shield
                                .then_some(rtao_catalog::py::MEV_SHIELD_ERA_PERIOD as u32),
                            ..Default::default()
                        },
                    )
                    .await
                    .map_err(CliError::transaction);
            }
            approve_or_submit(
                ctx,
                &executor,
                source.as_ref(),
                request,
                &multisig,
                call.clone(),
                RawShield {
                    choice: shield,
                    required,
                },
            )
            .await
        }
        .await;
        // Like Python's finally hook: retain the original outcome, release on
        // both paths, and never let a best-effort close mask the chain result.
        let _ = factory
            .release(
                selection,
                source.as_ref(),
                result.as_ref().is_ok_and(|receipt| receipt.success),
            )
            .await;
        last = Some(result.map_err(|error| {
            if index > 0 {
                continuation_failure(error, &call)
            } else {
                error
            }
        })?);
    }
    let mut result = last.ok_or_else(|| CliError::failure("no approval result"))?;
    if let Some(group) = &multisig {
        let followup = multisig_followup(ctx, client, request, group, &call, &result, cache)
            .await
            .map_err(|error| {
                crate::context::known_receipt_error(continuation_failure(error, &call), &result)
            })?;
        result.data.insert("multisig_followup".into(), followup);
    }
    if !ctx
        .output
        .result(&result, &success_message(request, shield.enabled))
        .map_err(|error| crate::context::known_receipt_error(public_failure(error), &result))?
    {
        return Err(CliError::failure(""));
    }
    Ok(Some(result))
}

fn continuation_failure(mut error: CliError, call: &CallBytes) -> CliError {
    let recovery = format!(
        "earlier approvals may remain pending for call hash 0x{}; run `rtao multisig pending` to finish or cancel before retrying",
        hex::encode(call.hash()),
    );
    error.help = Some(match error.help.take() {
        Some(help) => format!("{help}; {recovery}"),
        None => recovery,
    });
    error
}

async fn prepare_continuation(
    client: &Client,
    group: &Multisig,
    call: &CallBytes,
    selection: &SigningSelection,
    expected: MultisigTimepoint,
) -> Result<CallBytes, CliError> {
    let signer = selection
        .address
        .ok_or_else(|| CliError::input("continued approval requires a known public member"))?;
    let snapshot = View::pin(client).await.map_err(CliError::from)?;
    let estimate = client
        .estimate_weight(call.clone(), signer, CryptoType::Sr25519)
        .await
        .map_err(CliError::from)?;
    let outer = rtao::client::compose::as_multi_existing(
        &snapshot,
        signer,
        &group.signatories,
        group.threshold,
        call.clone(),
        &estimate,
        expected,
    )
    .await
    .map_err(CliError::from)?;
    check_multisig_funds(
        client,
        &snapshot,
        FundsRequest {
            signer,
            threshold: group.threshold,
            opening: false,
            outer_call: Some(&outer),
            crypto_type: CryptoType::Sr25519,
            signer_label: Some(selection.display_wallet()),
        },
    )
    .await
    .map_err(CliError::transaction)?;
    // The existing opening is encoded in `outer`. If state disappears after
    // this read, submission keeps Some(expected) and cannot open a new operation.
    Ok(outer)
}

#[derive(Clone, Copy)]
struct RawShield {
    choice: crate::context::ShieldChoice,
    required: bool,
}

async fn approve_or_submit(
    ctx: &AppContext,
    executor: &rtao::Executor,
    keys: &dyn DynKeySource,
    request: &RawCallRequest,
    multisig: &Option<Multisig>,
    call: CallBytes,
    shield: RawShield,
) -> Result<ExtrinsicResult, CliError> {
    let payer = public_signer(keys, request.role)?;
    // Raw calls only degrade for an unavailable shield key, exactly as call.py;
    // the library performs real carrier funding/signing checks on submission.
    let use_shield = honor_raw_shield(
        ctx,
        executor.client(),
        shield.choice,
        shield.required,
        &request.target,
    )
    .await?;
    if let Some(multisig) = multisig {
        return multisig
            .approve(
                executor,
                call,
                keys,
                &ApproveOptions {
                    signer: request.role,
                    shielded: use_shield,
                    signer_label: Some(payer.to_string()),
                    ..Default::default()
                },
            )
            .await
            .map_err(CliError::transaction);
    }
    executor
        .submit_call_dyn(
            call,
            keys,
            request.role,
            &ExecuteOptions {
                shielded: use_shield,
                period: use_shield.then_some(rtao_catalog::py::MEV_SHIELD_ERA_PERIOD as u32),
                ..Default::default()
            },
        )
        .await
        .map_err(CliError::transaction)
}

async fn honor_raw_shield(
    ctx: &AppContext,
    client: &Client,
    shield: crate::context::ShieldChoice,
    required: bool,
    target: &str,
) -> Result<bool, CliError> {
    if !shield.enabled {
        return Ok(false);
    }
    let key = client
        .storage(&rtao_runtime::storage::mev_shield::NEXT_KEY, ())
        .await
        .map_err(CliError::from)?;
    if key.is_some() {
        return Ok(true);
    }
    if shield.forced || required {
        return Err(CliError::input(
            "MEV shield is not active on this network (MevShield.NextKey is unset)",
        )
        .with_help(if required {
            format!("{target} cannot submit unshielded")
        } else {
            "pass --no-mev-shield to submit unshielded".into()
        }));
    }
    ctx.output
        .message("MEV shield is not active on this network — submitting unshielded")
        .map_err(public_failure)?;
    Ok(false)
}

fn show_dry_run(
    ctx: &AppContext,
    request: &RawCallRequest,
    multisig: &Option<Multisig>,
    shield: bool,
    replay: &str,
) -> Result<(), CliError> {
    let mut fields = Map::from_iter([
        ("target".into(), json!(request.target)),
        ("sudo".into(), json!(request.sudo)),
        ("signer".into(), json!(request.role.as_str())),
        ("params".into(), Json::Object(for_display(&request.params))),
    ]);
    if let Some(proxy) = request.proxy_for {
        fields.insert("proxy_for".into(), json!(proxy));
        if let Some(force) = request.force_proxy_type {
            fields.insert("force_proxy_type".into(), json!(force));
        }
    }
    if shield {
        fields.insert("mev_shield".into(), json!(true));
    }
    if let Some(multisig) = multisig {
        fields.insert("multisig_threshold".into(), json!(multisig.threshold));
        if let Some(group) = &request.multisig {
            fields.insert("signatories".into(), json!(group.signatories));
            if let Some(preset) = &group.preset {
                fields.insert("multisig_preset".into(), json!(preset));
            }
        }
        fields.insert("multisig_address".into(), json!(multisig.address));
        if request.rounds.len() > 1 {
            fields.insert(
                "approvals_this_run".into(),
                json!(
                    request
                        .rounds
                        .iter()
                        .map(|round| format!("{} ({})", round.display_wallet(), round.backend))
                        .collect::<Vec<_>>()
                        .join(" → ")
                ),
            );
        }
    }
    fields.insert("command".into(), json!(replay));
    ctx.output
        .detail(
            Some("dry run: raw call"),
            &fields
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<Vec<_>>(),
            None,
        )
        .map_err(public_failure)
}

fn raw_label(request: &RawCallRequest, shield: bool) -> String {
    let mut label = request.target.clone();
    if request.sudo {
        label.push_str(" via Sudo.sudo");
    }
    if let Some(proxy) = request.proxy_for {
        label.push_str(&format!(" as {proxy} via proxy"));
    }
    if shield {
        label.push_str(" [MEV-shielded]");
    }
    label
}

fn success_message(request: &RawCallRequest, shield: bool) -> String {
    let action = if request.multisig.is_some() {
        "multisig approved"
    } else {
        "submitted"
    };
    format!("{action} {}", raw_label(request, shield))
}

fn confirmation(request: &RawCallRequest, shield: bool) -> String {
    let label = raw_label(request, shield);
    if let Some(group) = &request.multisig {
        let who = group
            .preset
            .clone()
            .unwrap_or_else(|| format!("{}-of-{}", group.threshold, group.signatories.len()));
        if request.rounds.len() > 1 {
            format!(
                "multisig-approve {label} ({who}, {} approvals)?",
                request.rounds.len()
            )
        } else {
            format!(
                "multisig-approve {label} ({who}, signed by {})?",
                request.role
            )
        }
    } else {
        format!("submit raw call {label} (signed by {})?", request.role)
    }
}

fn show_raw_review(
    ctx: &AppContext,
    request: &RawCallRequest,
    shield: bool,
) -> Result<(), CliError> {
    let mut call = vec![("target".into(), json!(request.target))];
    if request.sudo {
        call.push(("sudo".into(), json!("yes")));
    }
    if let Some(proxy) = request.proxy_for {
        call.push(("proxy for".into(), json!(proxy)));
    }
    call.push(("summary".into(), json!(raw_label(request, shield))));
    let mut signer = vec![("signer".into(), json!(ctx.globals.wallet_name))];
    for (index, selection) in request.rounds.iter().enumerate() {
        signer.push((
            if request.rounds.len() == 1 {
                "member".into()
            } else {
                format!("approval {}", index + 1)
            },
            json!(format!(
                "{} ({})",
                selection.display_wallet(),
                selection.backend
            )),
        ));
        if let Some(account) = selection.address {
            signer.push((
                if request.rounds.len() == 1 {
                    "account".into()
                } else {
                    format!("approval {} account", index + 1)
                },
                json!(account),
            ));
        }
    }
    if let Some(group) = &request.multisig {
        signer.push((
            "multisig".into(),
            json!(group.preset.clone().unwrap_or_else(|| format!(
                "{}-of-{}",
                group.threshold,
                group.signatories.len()
            ))),
        ));
        signer.push((
            "threshold".into(),
            json!(format!(
                "{} of {}",
                group.threshold,
                group.signatories.len()
            )),
        ));
    }
    if let Some(proxy) = request.proxy_for {
        signer.push(("proxy for".into(), json!(proxy)));
    }
    signer.push((
        "mev shield".into(),
        json!(if shield {
            "on — encrypted in the mempool until executed"
        } else {
            "off — no protection (call is visible in the mempool)"
        }),
    ));
    let mut transaction = vec![("operation".into(), json!(request.target))];
    transaction.extend(
        for_display(&request.params)
            .into_iter()
            .map(|(key, value)| (key.replace('_', " "), value)),
    );
    ctx.output
        .transaction_card(&[
            ReviewSection {
                title: "Call".into(),
                rows: call,
            },
            ReviewSection {
                title: "Fees".into(),
                rows: vec![("estimated fee".into(), json!("unavailable"))],
            },
            ReviewSection {
                title: "Signer".into(),
                rows: signer,
            },
            ReviewSection {
                title: "Transaction".into(),
                rows: transaction,
            },
        ])
        .map_err(public_failure)
}

/// Truncate only top-level long strings for Python's raw dry-run display shape.
pub fn for_display(params: &Map<String, Json>) -> Map<String, Json> {
    params
        .iter()
        .map(|(key, value)| {
            let value = match value {
                Json::String(value) if value.chars().count() > 80 => json!(format!(
                    "{}… (+{} chars)",
                    value.chars().take(80).collect::<String>(),
                    value.chars().count().saturating_sub(80)
                )),
                _ => value.clone(),
            };
            (key.clone(), value)
        })
        .collect()
}

fn public_signer(keys: &dyn DynKeySource, role: SignerRole) -> Result<AccountId, CliError> {
    match role {
        SignerRole::Coldkey => keys.coldkey_address(),
        SignerRole::Hotkey => keys.hotkey_address(),
    }
    .or_else(|error| keys.bare_signer_account().ok_or(error))
    .map_err(public_failure)
}

fn resolve_members(names: &NameResolver<'_>, refs: &[String]) -> Result<Vec<AccountId>, CliError> {
    let mut members = Vec::new();
    for reference in refs {
        let member = names
            .resolve_address_ref("coldkey_ss58", reference)
            .map_err(public_input)?
            .address
            .parse()
            .map_err(public_input)?;
        if !members.contains(&member) {
            members.push(member);
        }
    }
    Ok(members)
}

/// Resolve a saved multisig using only books and public wallet identities.
/// Runtime caps are validated later by [`Multisig::new`] against live metadata.
pub fn resolve_preset(ctx: &AppContext, preset: &str) -> Result<RawMultisig, CliError> {
    let lookup = FsAccountLookup::new(ctx.globals.wallet_path.clone()).map_err(public_input)?;
    let names = NameResolver::new(
        &ctx.books,
        &lookup,
        &ctx.globals.wallet_name,
        &ctx.globals.hotkey_name,
    );
    resolve_preset_with(&names, ctx, preset)
}

fn resolve_preset_with(
    names: &NameResolver<'_>,
    ctx: &AppContext,
    preset: &str,
) -> Result<RawMultisig, CliError> {
    let entry = ctx.books.multisig(preset).ok_or_else(|| {
        CliError::input(format!(
            "unknown multisig {preset:?}; run `rtao multisig list`"
        ))
    })?;
    let threshold = entry
        .get("threshold")
        .and_then(Json::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            CliError::input("saved multisig threshold must be a positive u16 integer")
        })?;
    let refs = entry
        .get("signatories")
        .and_then(Json::as_array)
        .ok_or_else(|| CliError::input("saved multisig signatories must be a list"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| CliError::input("saved multisig signatory must be a string"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let signatories = resolve_members(names, &refs)?;
    if signatories.len() < usize::from(threshold) {
        return Err(CliError::input(format!(
            "need at least {threshold} signatories, got {}",
            signatories.len()
        )));
    }
    Ok(RawMultisig {
        threshold,
        signatories,
        preset: Some(preset.into()),
        refs,
    })
}

/// Plan public signing members for raw or semantic calls, without acquiring keys.
/// Repeated members retain their order; automatic rounds prefer local secrets.
pub fn plan_rounds(
    ctx: &AppContext,
    group: Option<&RawMultisig>,
    role: SignerRole,
    backend: Option<&str>,
) -> Result<Vec<SigningSelection>, CliError> {
    let lookup = FsAccountLookup::new(ctx.globals.wallet_path.clone()).map_err(public_input)?;
    let names = NameResolver::new(
        &ctx.books,
        &lookup,
        &ctx.globals.wallet_name,
        &ctx.globals.hotkey_name,
    );
    signing_rounds(ctx, &lookup, &names, group, role, backend, true)
}

/// Select one public multisig member, without automatically chaining approvals.
/// Call [`plan_rounds`] when repeated `--signatory` values are permitted.
pub fn select_member(
    ctx: &AppContext,
    group: &RawMultisig,
    role: SignerRole,
    backend: Option<&str>,
) -> Result<SigningSelection, CliError> {
    if ctx.globals.signatories.len() > 1 {
        return Err(CliError::input(
            "select_member requires at most one --signatory; use plan_rounds",
        ));
    }
    let lookup = FsAccountLookup::new(ctx.globals.wallet_path.clone()).map_err(public_input)?;
    let names = NameResolver::new(
        &ctx.books,
        &lookup,
        &ctx.globals.wallet_name,
        &ctx.globals.hotkey_name,
    );
    signing_rounds(ctx, &lookup, &names, Some(group), role, backend, false)?
        .into_iter()
        .next()
        .ok_or_else(|| CliError::input("no selected signing member"))
}

// Keep Python's naturally ordered wallet scan local to multisig consumers.
// Both sorts are stable: account order takes precedence, while aliases keep
// natural wallet order and equal natural names retain the original scan order.
fn local_multisig_wallets(
    lookup: &dyn AccountLookup,
    signatories: &[AccountId],
) -> Vec<(String, AccountId)> {
    let mut locals = lookup
        .candidates()
        .into_iter()
        .filter(|candidate| candidate.source == "wallet")
        .filter_map(|candidate| {
            candidate
                .address
                .parse::<AccountId>()
                .ok()
                .filter(|address| signatories.contains(address))
                .map(|address| (candidate.name, address))
        })
        .collect::<Vec<_>>();
    locals.sort_by(|left, right| crate::commands::wallet::natural_cmp(&left.0, &right.0));
    locals.sort_by_key(|(_, account)| signatories.iter().position(|member| member == account));
    locals
}

fn signing_rounds(
    ctx: &AppContext,
    lookup: &FsAccountLookup,
    names: &NameResolver<'_>,
    group: Option<&RawMultisig>,
    role: SignerRole,
    backend_override: Option<&str>,
    automatic: bool,
) -> Result<Vec<SigningSelection>, CliError> {
    let selected_backend = backend_override.or(ctx.globals.signer_backend.as_deref());
    let backend = selected_backend.unwrap_or("wallet");
    let normalized_backend = crate::globals::normalize_signer_backend(backend);
    let explicit = &ctx.globals.signatories;
    let Some(group) = group else {
        let address = if normalized_backend == "extension"
            && ctx
                .globals
                .signer_address
                .as_deref()
                .is_none_or(str::is_empty)
            && explicit.is_empty()
        {
            // A saved extension account is a preference for the public picker,
            // not an explicit member pin that bypasses source filtering.
            None
        } else if normalized_backend != "wallet" {
            signer_address_for_backend(ctx, backend)?
        } else {
            ctx.globals
                .signer_address
                .as_deref()
                .map(|address| {
                    names
                        .resolve_address_ref("coldkey_ss58", address)
                        .map_err(public_input)?
                        .address
                        .parse()
                        .map_err(public_input)
                })
                .transpose()?
        };
        return Ok(vec![SigningSelection {
            display_name: None,
            wallet: ctx.globals.wallet_name.clone(),
            backend: backend.into(),
            address,
        }]);
    };
    let locals = local_multisig_wallets(lookup, &group.signatories);
    let external = |name: &str, address: AccountId| {
        ctx.books
            .address_entry(name)
            .or_else(|| ctx.books.address_entry(&address.to_string()))
            .and_then(|entry| entry.get("signer"))
            .and_then(Json::as_str)
            .map(crate::globals::normalize_signer_backend)
            .filter(|backend| {
                ["vault", "ledger", "extension", "remote"].contains(&backend.as_str())
            })
    };
    let member = |reference: &str,
                  wallet_only: bool,
                  exact_wallet: bool|
     -> Result<SigningSelection, CliError> {
        let (reference, forced) = reference
            .split_once('=')
            .map(|(reference, forced)| {
                (
                    reference,
                    Some(crate::globals::normalize_signer_backend(forced))
                        .filter(|backend| explicit.len() == 1 || !backend.is_empty()),
                )
            })
            .unwrap_or((reference, None));
        if let Some(forced) = &forced
            && !["wallet", "vault", "ledger", "extension", "remote"].contains(&forced.as_str())
        {
            return Err(CliError::input(format!(
                "unknown backend {forced:?} in --signatory"
            )));
        }
        // Local single/picker choices bind the public wallet directly. Repeated
        // refs follow Python's member namespace: raw address, member contact,
        // then a local member name; proxy/saved multisig aliases do not enter it.
        let local_address = || {
            locals
                .iter()
                .find(|(name, member)| name == reference || member.to_string() == reference)
                .map(|(_, member)| *member)
        };
        let member_address = || {
            reference
                .parse::<AccountId>()
                .ok()
                .filter(|address| group.signatories.contains(address))
                .or_else(|| {
                    ctx.books
                        .addresses
                        .iter()
                        .find(|entry| entry.get("name").and_then(Json::as_str) == Some(reference))
                        .and_then(|entry| entry.get("address"))
                        .and_then(Json::as_str)
                        .and_then(|address| address.parse::<AccountId>().ok())
                        .filter(|address| group.signatories.contains(address))
                })
                .or_else(|| {
                    locals
                        .iter()
                        .find(|(name, _)| name == reference)
                        .map(|(_, address)| *address)
                })
        };
        let address = (exact_wallet && forced.as_deref().is_none_or(|backend| backend == "wallet"))
            .then(local_address)
            .flatten()
            .or_else(member_address)
            .ok_or_else(|| {
                CliError::input(format!(
                    "--signatory {reference:?} is not a member of this multisig"
                ))
            })?;
        if !group.signatories.contains(&address) {
            return Err(CliError::input(format!(
                "--signatory {reference:?} is not a member of this multisig"
            )));
        }
        // Single explicit and interactive wallet choices retain their exact
        // file. Python's chained driver selects by address and uses the first
        // natural alias.
        let local = exact_wallet
            .then(|| {
                locals
                    .iter()
                    .find(|(name, member)| name == reference && *member == address)
            })
            .flatten()
            .or_else(|| locals.iter().find(|(_, member)| *member == address));
        let inferred = if wallet_only || local.is_some() {
            Some("wallet".into())
        } else {
            external(reference, address)
        };
        let selected = match forced.or(inferred) {
            Some(selected) => selected,
            None => prompt_member_backend(ctx,reference,address)?.ok_or_else(|| CliError::input(format!("member {reference:?} has no local wallet and no saved signing backend; pass --signatory {reference}=vault (or ledger/extension)")))?,
        };
        let wallet = if selected == "wallet" {
            local
                .map(|(name, _)| name.clone())
                .ok_or_else(|| CliError::input("selected member has no local wallet"))?
        } else {
            reference.to_owned()
        };
        let display = if is_bittensor_address(reference) {
            locals
                .iter()
                .find(|(_, member)| *member == address)
                .map(|(name, _)| name.clone())
                .or_else(|| {
                    ctx.books
                        .address_entry(&address.to_string())
                        .and_then(|entry| entry.get("name").and_then(Json::as_str))
                        .filter(|name| !name.is_empty())
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| address.to_string())
        } else {
            reference.to_owned()
        };
        Ok(SigningSelection {
            display_name: (display != wallet).then_some(display),
            wallet,
            backend: selected,
            address: Some(address),
        })
    };
    if explicit.len() > 1 {
        if role != SignerRole::Coldkey {
            return Err(CliError::input(
                "chained approvals sign with member coldkeys; with --signer hotkey pass one --signatory per invocation",
            ));
        }
        if explicit.len() > usize::from(group.threshold) {
            return Err(CliError::input(
                "pass at most the multisig threshold number of --signatory members",
            ));
        }
        let rounds = explicit
            .iter()
            .map(|reference| member(reference, false, false))
            .collect::<Result<Vec<_>, _>>()?;
        let mut unique = BTreeSet::new();
        if rounds
            .iter()
            .filter_map(|round| round.address)
            .any(|address| !unique.insert(address))
        {
            return Err(CliError::input(
                "--signatory repeats a member; each member approves once",
            ));
        }
        return Ok(rounds);
    }
    if normalized_backend != "wallet" {
        let address = signer_address_for_backend(ctx, backend)?.ok_or_else(|| {
            CliError::input("external multisig signer needs --signer-address or --signatory")
        })?;
        if !group.signatories.contains(&address) {
            return Err(CliError::input(
                "external signer account is not a multisig signatory",
            ));
        }
        return Ok(vec![SigningSelection {
            display_name: None,
            wallet: explicit
                .first()
                .cloned()
                .unwrap_or_else(|| ctx.globals.wallet_name.clone()),
            backend: backend.into(),
            address: Some(address),
        }]);
    }
    if let Some(reference) = explicit.first() {
        // An explicitly chosen root wallet backend suppresses external tag
        // inference for a single member. Repeated approvals keep their own
        // per-member inference, matching Python's separate rounds planner.
        let wallet_only = selected_backend.is_some_and(|backend| !backend.is_empty());
        return member(reference, wallet_only, true).map(|selected| vec![selected]);
    }
    if automatic
        && role == SignerRole::Coldkey
        && normalized_backend == "wallet"
        && selected_backend.is_none_or(str::is_empty)
        && ctx.globals.signer_address.is_none()
        && group.threshold >= 2
    {
        let mut rounds = Vec::new();
        for address in &group.signatories {
            // Python's dict keeps the last natural alias for auto admission
            // and display. Its chained driver selects by account afterwards,
            // so the first natural alias is the actual wallet source.
            let last_alias = locals
                .iter()
                .rev()
                .find(|(_, member)| member == address)
                .map(|(name, _)| name);
            if let Some(last) = last_alias
                && lookup.root().join(last).join("coldkey").is_file()
            {
                let first = locals
                    .iter()
                    .find(|(_, member)| member == address)
                    .map(|(name, _)| name)
                    .unwrap_or(last);
                rounds.push(SigningSelection {
                    display_name: (last != first).then(|| last.clone()),
                    wallet: first.clone(),
                    backend: "wallet".into(),
                    address: Some(*address),
                });
            } else if let Some(backend) = external(&address.to_string(), *address)
                .or_else(|| last_alias.and_then(|name| external(name, *address)))
            {
                let name = ctx
                    .books
                    .address_entry(&address.to_string())
                    .or_else(|| last_alias.and_then(|name| ctx.books.address_entry(name)))
                    .and_then(|entry| entry.get("name").and_then(Json::as_str))
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .or_else(|| last_alias.cloned())
                    .unwrap_or_else(|| address.to_string());
                rounds.push(SigningSelection {
                    display_name: None,
                    wallet: name,
                    backend,
                    address: Some(*address),
                });
            }
        }
        if rounds.len() >= 2 {
            rounds.sort_by_key(|round| round.backend != "wallet");
            rounds.truncate(usize::from(group.threshold));
            ctx.output
                .message("no --signatory given — approvals planned from local wallets and address-book signer tags")
                .map_err(public_failure)?;
            return Ok(rounds);
        }
    }
    let own = match role {
        SignerRole::Coldkey => lookup.coldkey(&ctx.globals.wallet_name),
        SignerRole::Hotkey => lookup.hotkey(&ctx.globals.wallet_name, &ctx.globals.hotkey_name),
    }
    .ok()
    .and_then(|address| address.parse().ok());
    if let Some(address) = own.filter(|address| group.signatories.contains(address)) {
        return Ok(vec![SigningSelection {
            display_name: None,
            wallet: ctx.globals.wallet_name.clone(),
            backend: "wallet".into(),
            address: Some(address),
        }]);
    }
    if locals.len() == 1 {
        return Ok(vec![SigningSelection {
            display_name: None,
            wallet: locals[0].0.clone(),
            backend: "wallet".into(),
            address: Some(locals[0].1),
        }]);
    }
    if locals.is_empty() {
        return Err(CliError::input(
            "no local multisig signatory wallet; select an external member with --signatory",
        ));
    }
    if ctx.globals.assume_yes || ctx.globals.json || ctx.globals.quiet {
        return Err(CliError::input(
            "multiple local signatories; pass --signatory to select one",
        ));
    }
    let mut terminal = ctx
        .terminal
        .lock()
        .map_err(|_| CliError::failure("terminal lock poisoned"))?;
    if !terminal.stdin_is_tty() || !terminal.stderr_is_tty() {
        return Err(CliError::input(
            "multiple local signatories; pass --signatory to select one",
        ));
    }
    let mut spec = crate::prompt::PromptSpec::new("signatory_wallet", "--signatory");
    spec.default = Some(locals[0].0.clone());
    spec.help = Some("Which local member of this multisig signs this approval.".into());
    let prompt = crate::prompt::PromptContext {
        json: ctx.globals.json,
        quiet: ctx.globals.quiet,
        assume_yes: ctx.globals.assume_yes,
        ..Default::default()
    };
    let (selected, _) = crate::prompt::ask_with(&prompt, &mut **terminal, &spec, |raw| {
        locals
            .iter()
            .find(|(name, _)| name == raw)
            .map(|(name, _)| name.clone())
            .ok_or_else(|| {
                crate::prompt::PromptValidation::Message(format!(
                    "unknown local signatory {raw:?}; choose one of: {}",
                    locals
                        .iter()
                        .map(|(name, _)| name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })
    })
    .map_err(|error| CliError {
        code: error.exit_code(),
        ..crate::error::CliError::failure(error.to_string())
    })?;
    drop(terminal);
    member(&selected, false, true).map(|member| vec![member])
}

fn signer_address_for_backend(
    ctx: &AppContext,
    backend: &str,
) -> Result<Option<AccountId>, CliError> {
    // Raw call owns --signer, so its backend override is not applied to globals.
    // Reuse the semantic route's account precedence with a side-effect-free
    // context snapshot; none of these clones acquires a key or opens a session.
    let mut globals = ctx.globals.clone();
    globals.signer_backend = Some(backend.into());
    let selected = AppContext {
        globals,
        config: ctx.config.clone(),
        config_store: ctx.config_store.clone(),
        books: ctx.books.clone(),
        output: ctx.output.clone(),
        passwords: ctx.passwords.clone(),
        terminal: ctx.terminal.clone(),
        interruption: ctx.interruption.clone(),
        track_heads: ctx.track_heads,
    };
    let resolved = selected.external_signer_address()?;
    if selected
        .globals
        .signer_address
        .as_deref()
        .is_some_and(|address| !address.is_empty())
        && resolved.is_none()
    {
        return Err(CliError::input(
            "--signer-address is not a valid SS58 address or known account reference",
        ));
    }
    Ok(resolved)
}

fn prompt_member_backend(
    ctx: &AppContext,
    reference: &str,
    address: AccountId,
) -> Result<Option<String>, CliError> {
    let mut terminal = ctx
        .terminal
        .lock()
        .map_err(|_| CliError::failure("terminal lock poisoned"))?;
    let prompt = crate::prompt::PromptContext {
        json: ctx.globals.json,
        quiet: ctx.globals.quiet,
        assume_yes: ctx.globals.assume_yes,
        ..Default::default()
    };
    if !prompt.interactive(&**terminal) {
        return Ok(None);
    }
    let mut spec = crate::prompt::PromptSpec::new("member_backend", "--signer");
    spec.default = Some("vault".into());
    spec.placeholder = Some("vault/ledger/extension/remote".into());
    spec.help = Some(format!(
        "How does member {reference} sign? It has no local wallet or saved tag."
    ));
    let (backend, _) = crate::prompt::ask_with(&prompt, &mut **terminal, &spec, |raw| {
        let raw = raw.to_lowercase();
        if ["vault", "ledger", "extension", "remote"].contains(&raw.as_str()) {
            Ok(raw)
        } else {
            Err(crate::prompt::PromptValidation::Message(
                "choose one of: extension, ledger, remote, vault".into(),
            ))
        }
    })
    .map_err(|error| CliError {
        code: error.exit_code(),
        ..crate::error::CliError::failure(error.to_string())
    })?;
    drop(terminal);
    if let Some(entry) = ctx
        .books
        .address_entry(reference)
        .or_else(|| ctx.books.address_entry(&address.to_string()))
        .filter(|entry| {
            entry
                .get("name")
                .and_then(Json::as_str)
                .is_some_and(|name| !name.is_empty())
        })
    {
        let name = entry
            .get("name")
            .and_then(Json::as_str)
            .ok_or_else(|| CliError::input("contact name is missing"))?;
        let mut updated = entry.clone();
        updated["signer"] = json!(backend);
        let paths = ctx
            .config_store
            .as_ref()
            .map(|store| Ok(store.paths().clone()))
            .unwrap_or_else(|| Paths::from_process().map_err(public_failure))?;
        let address_path = paths.path(ConfigFile::Addresses);
        let mut entries: Vec<_> = raw_load_contacts(address_path)?
            .into_iter()
            .filter(|entry| entry.get("name").and_then(Json::as_str) != Some(name))
            .collect();
        entries.push(updated);
        raw_write_json(paths.path(ConfigFile::Addresses), &json!(entries))?;
        ctx.output.message(&format!("saved signer tag on contact {name} — future runs won't ask (change with `rtao addr add {name} --signer …`)")).map_err(public_failure)?;
    }
    Ok(Some(backend))
}

fn raw_load_contacts(path: &Path) -> Result<Vec<Json>, CliError> {
    let (value, _) = read_public_json_for_update(path, false).map_err(public_failure)?;
    Ok(value.as_array().cloned().unwrap_or_default())
}

fn validate_rounds(request: &RawCallRequest) -> Result<(), CliError> {
    if request.rounds.len() > 1 {
        let group = request
            .multisig
            .as_ref()
            .ok_or_else(|| CliError::input("multiple signing rounds require a multisig"))?;
        if request.role != SignerRole::Coldkey
            || request.rounds.len() > usize::from(group.threshold)
        {
            return Err(CliError::input(
                "invalid chained multisig approval count or role",
            ));
        }
        let mut seen = BTreeSet::new();
        for member in &request.rounds {
            let address = member
                .address
                .ok_or_else(|| CliError::input("chained approval needs a public member account"))?;
            if !group.signatories.contains(&address) || !seen.insert(address) {
                return Err(CliError::input(
                    "invalid or repeated multisig signing member",
                ));
            }
        }
    }
    Ok(())
}

fn public_input(error: impl std::fmt::Display) -> CliError {
    CliError::input(error.to_string())
}
fn public_failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}

/// Injected public call-spec persistence; never receives private signing state.
pub trait RawCallCache: Send + Sync {
    /// Save one normalized call hash/spec with the shared btcli cache layout.
    /// Filesystem persistence authenticates `call_data` against this exact hash.
    fn save(&self, hash: String, spec: Json) -> BoxFuture<'_, Result<(), CliError>>;
}

/// Filesystem adapter for exactly the resolved shared multisig-cache path.
pub struct FsRawCallCache {
    path: PathBuf,
}

impl FsRawCallCache {
    /// Construction neither creates a file nor changes the process environment.
    pub fn new(paths: &Paths) -> Self {
        Self {
            path: paths.path(ConfigFile::MultisigCache).to_owned(),
        }
    }

    /// Resolve the shared process path without reading or writing its cache.
    pub fn from_process() -> Result<Self, CliError> {
        let paths = Paths::from_process().map_err(|error| CliError::failure(error.to_string()))?;
        Ok(Self::new(&paths))
    }

    /// Recover exact call bytes from a Python-compatible shared entry.
    /// The key, optional stored hash, and blake2_256 of `call_data` must agree.
    /// Complete runtime decoding/re-encoding authenticates the SCALE layout;
    /// cached target/params, labels and network strings are never trusted.
    /// This checks call identity, not chain origin or approval authority.
    pub async fn load_call(
        &self,
        hash: &str,
        runtime: &RuntimeHandle,
    ) -> Result<Option<CallBytes>, CliError> {
        let hash = normalize_cache_hash(hash)?;
        let path = self.path.clone();
        let runtime = runtime.clone();
        rtao::core::offload(move || {
            let (entries, _) = read_public_json_for_update(&path, true).map_err(public_failure)?;
            let Some(spec) = entries.get(&hash) else {
                return Ok(None);
            };
            let bytes = authenticated_cache_data(&hash, spec)?;
            CallBytes::decode(&bytes, &runtime)
                .map(Some)
                .map_err(public_input)
        })
        .await
        .map_err(|_| CliError::failure("multisig cache worker failed"))?
    }
}

impl RawCallCache for FsRawCallCache {
    fn save(&self, hash: String, mut spec: Json) -> BoxFuture<'_, Result<(), CliError>> {
        let path = self.path.clone();
        Box::pin(async move {
            rtao::core::offload(move || {
                let hash = normalize_cache_hash(&hash)?;
                let bytes = authenticated_cache_data(&hash, &spec)?;
                spec["call_hash"] = json!(hash);
                spec["call_data"] = json!(format!("0x{}", hex::encode(bytes)));
                let (value, corrupt) =
                    read_public_json_for_update(&path, true).map_err(public_failure)?;
                let mut entries = value
                    .as_object()
                    .cloned()
                    .ok_or_else(|| CliError::failure("multisig cache is not an object"))?;
                entries.insert(hash, spec);
                entries.sort_keys();
                write_public_json(&path, &Json::Object(entries), corrupt).map_err(public_failure)
            })
            .await
            .map_err(|error| CliError::failure(format!("multisig cache worker failed: {error}")))?
        })
    }
}

const RAW_CACHE_MAX_BYTES: usize = 16 * 1024 * 1024;

fn normalize_cache_hash(hash: &str) -> Result<String, CliError> {
    let hash = hash.strip_prefix("0x").unwrap_or(hash);
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CliError::input(
            "multisig call hash must be 32 bytes of hex",
        ));
    }
    Ok(format!("0x{}", hash.to_ascii_lowercase()))
}

fn authenticated_cache_data(hash: &str, spec: &Json) -> Result<Vec<u8>, CliError> {
    if !spec.is_object() {
        return Err(CliError::input(
            "multisig call cache entry must be an object",
        ));
    }
    if let Some(stored) = spec.get("call_hash") {
        let stored = stored
            .as_str()
            .ok_or_else(|| CliError::input("stored multisig call hash must be hex"))?;
        if normalize_cache_hash(stored)? != hash {
            return Err(CliError::input(
                "stored multisig call hash does not match its cache key",
            ));
        }
    }
    let data = spec
        .get("call_data")
        .and_then(Json::as_str)
        .ok_or_else(|| CliError::input("multisig call cache entry requires exact call_data hex"))?;
    let data = data.strip_prefix("0x").unwrap_or(data);
    if data.is_empty() || data.len() > RAW_CACHE_MAX_BYTES || !data.len().is_multiple_of(2) {
        return Err(CliError::input(
            "multisig call data is empty, oversized or invalid hex",
        ));
    }
    let bytes = hex::decode(data).map_err(|_| CliError::input("multisig call data must be hex"))?;
    let computed =
        rtao::core::codec::storage::hash_param("Blake2_256", &bytes).map_err(public_failure)?;
    if format!("0x{}", hex::encode(computed)) != hash {
        return Err(CliError::input(
            "multisig call data does not match its call hash",
        ));
    }
    Ok(bytes)
}

fn raw_write_json(path: &Path, value: &Json) -> Result<(), CliError> {
    write_public_json(path, value, false).map_err(public_failure)
}

/// Match receipt events, including a threshold-reaching inner dispatch failure.
pub fn multisig_executed(result: &ExtrinsicResult) -> bool {
    result
        .events
        .iter()
        .any(|event| event.matches("Multisig", "MultisigExecuted"))
}

/// Authenticate the opening that an included approval actually created or used.
///
/// `NewMultisig` takes its opening from the included receipt's height and index;
/// `MultisigApproval` carries the original opening in its typed event fields.
/// Both require the expected group/hash and the receipt's extrinsic position.
/// Missing, conflicting or unobserved evidence refuses automatic continuation.
/// The receipt-height inference assumes pinned v478's multisig
/// `BlockNumberProvider = System`; a changed provider requires fresh source review.
pub fn approval_timepoint(
    receipt: &ExtrinsicResult,
    group: &Multisig,
    call: &CallBytes,
) -> Result<MultisigTimepoint, CliError> {
    let refuse = || {
        CliError::failure(format!(
            "cannot authenticate the multisig opening for 0x{}; inspect `rtao multisig pending` before continuing",
            hex::encode(call.hash()),
        ))
    };
    if !receipt.success || !matches!(receipt.inclusion, Inclusion::InBlock | Inclusion::Finalized) {
        return Err(refuse());
    }
    let included = receipt
        .extrinsic_id
        .as_deref()
        .and_then(|id| {
            let (height, index) = id.split_once('-')?;
            Some(MultisigTimepoint {
                height: height.parse().ok()?,
                index: index.parse().ok()?,
            })
        })
        .ok_or_else(refuse)?;
    let mut opening = None;
    for event in &receipt.events {
        if !event.matches("Multisig", "NewMultisig")
            && !event.matches("Multisig", "MultisigApproval")
        {
            continue;
        }
        let attributes = rtao::core::DynValue::new(event.attributes.clone()).to_json();
        let account: AccountId =
            serde_json::from_value(attributes.get("multisig").cloned().ok_or_else(refuse)?)
                .map_err(|_| refuse())?;
        // Core event hashes retain byte values; the human JSON adapter renders
        // those as hex strings, while H256's serde representation is an array.
        // Authenticate the original decoded bytes before any display conversion.
        let hash: [u8; 32] = event
            .attributes
            .field("call_hash")
            .and_then(ValueExt::value_bytes)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(refuse)?;
        if account != group.address || hash != call.hash() {
            continue;
        }
        if event.phase != "ApplyExtrinsic"
            || event.extrinsic_idx != Some(included.index)
            || opening.is_some()
        {
            return Err(refuse());
        }
        opening = Some(if event.event_id == "NewMultisig" {
            included.clone()
        } else {
            serde_json::from_value(attributes.get("timepoint").cloned().ok_or_else(refuse)?)
                .map_err(|_| refuse())?
        });
    }
    opening.ok_or_else(refuse)
}

fn raw_has_multisig_event(result: &ExtrinsicResult) -> bool {
    result
        .events
        .iter()
        .any(|event| event.module_id == "Multisig" && !event.event_id.is_empty())
}

/// Cover RPC head lag between already-included approval rounds, as in Python.
/// Absence after eight polls is not execution evidence and returns quietly.
pub async fn await_pending_visible(
    client: &Client,
    ms: &Multisig,
    call: &CallBytes,
) -> Result<(), CliError> {
    for _ in 0..8 {
        if raw_pending(client, ms.address, call.hash())
            .await?
            .is_some()
        {
            return Ok(());
        }
        // Python sleeps after the last unsuccessful poll too.
        tokio::time::sleep(Duration::from_millis(1_500)).await;
    }
    Ok(())
}

async fn raw_pending(
    client: &Client,
    address: AccountId,
    hash: [u8; 32],
) -> Result<Option<MultisigInfo>, CliError> {
    client
        .storage(&rtao_runtime::storage::multisig::MULTISIGS, (address, hash))
        .await
        .map_err(CliError::from)
}

async fn raw_query_pending(
    client: &Client,
    ms: &Multisig,
    call: &CallBytes,
) -> Result<Option<MultisigInfo>, CliError> {
    for attempt in 0..6 {
        if let Some(pending) = raw_pending(client, ms.address, call.hash()).await? {
            return Ok(Some(pending));
        }
        if attempt + 1 < 6 {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    // Same pending-map fallback as Python, narrowed to the known first key.
    // Iteration is pinned at one fresh head and retains live metadata decoding.
    let stream = client.storage_iter(&rtao_runtime::storage::multisig::MULTISIGS, (ms.address,));
    tokio::pin!(stream);
    while let Some(row) = stream.next().await {
        let (keys, pending) = row.map_err(CliError::from)?;
        let key = keys
            .first()
            .filter(|_| keys.len() == 1)
            .ok_or_else(|| CliError::failure("multisig map did not return its call-hash key"))?;
        let decoded = match key.as_core() {
            rtao::core::codec::Value::Bytes(bytes) => bytes.clone(),
            rtao::core::codec::Value::Str(text) => {
                hex::decode(text.strip_prefix("0x").unwrap_or(text))
                    .map_err(|_| CliError::failure("multisig map call-hash key is not hex"))?
            }
            _ => return Err(CliError::failure("multisig map call-hash key is not bytes")),
        };
        if decoded.len() != 32 {
            return Err(CliError::failure(
                "multisig map call-hash key is not 32 bytes",
            ));
        }
        if decoded.as_slice() == call.hash().as_slice() {
            return Ok(Some(pending));
        }
    }
    Ok(None)
}

fn raw_soft_resolve_coldkey(
    ctx: &AppContext,
    lookup: &FsAccountLookup,
    reference: &str,
) -> Option<AccountId> {
    if is_bittensor_address(reference) {
        return reference.parse().ok();
    }
    if let Some(address) = ctx.books.addresses.iter().find_map(|entry| {
        (entry.get("name").and_then(Json::as_str) == Some(reference))
            .then(|| entry.get("address").and_then(Json::as_str))
            .flatten()
    }) {
        return address.parse().ok();
    }
    lookup.coldkey(reference).ok()?.parse().ok()
}

/// Resolve portable member labels from preset refs, local wallets and contacts.
pub fn signatory_labels(
    ctx: &AppContext,
    raw: &RawMultisig,
) -> Result<BTreeMap<AccountId, String>, CliError> {
    let lookup = FsAccountLookup::new(ctx.globals.wallet_path.clone())
        .map_err(|error| CliError::failure(error.to_string()))?;
    let mut preset_by_addr = BTreeMap::new();
    for reference in &raw.refs {
        if let Some(account) = raw_soft_resolve_coldkey(ctx, &lookup, reference) {
            preset_by_addr.insert(account, reference.clone());
        }
    }
    let locals = local_multisig_wallets(&lookup, &raw.signatories);
    let mut labels = BTreeMap::new();
    for account in &raw.signatories {
        let preset = preset_by_addr
            .get(account)
            .filter(|reference| !is_bittensor_address(reference));
        let local = locals.iter().find(|(_, member)| member == account);
        let contact = ctx.books.addresses.iter().find(|entry| {
            entry
                .get("address")
                .and_then(Json::as_str)
                .and_then(|address| address.parse::<AccountId>().ok())
                == Some(*account)
        });
        let label = preset
            .cloned()
            .or_else(|| local.map(|(name, _)| name.clone()))
            .or_else(|| {
                contact
                    .and_then(|entry| entry.get("name").and_then(Json::as_str))
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| account.to_string());
        labels.insert(*account, label);
    }
    Ok(labels)
}

fn raw_display_member(account: AccountId, name: &str) -> String {
    let address = account.to_string();
    if address == name {
        address
    } else {
        format!("{name} ({address})")
    }
}

fn raw_contact_backend(ctx: &AppContext, reference: &str) -> Option<String> {
    let entry = ctx.books.address_entry(reference)?;
    let backend =
        crate::globals::normalize_signer_backend(entry.get("signer").and_then(Json::as_str)?);
    ["vault", "ledger", "extension", "remote"]
        .contains(&backend.as_str())
        .then_some(backend)
}

fn proxy_type_name(proxy_type: Option<ProxyType>) -> Result<Option<String>, CliError> {
    proxy_type
        .map(|proxy| {
            serde_json::to_value(proxy)
                .map_err(|error| CliError::failure(error.to_string()))?
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| {
                    CliError::failure("proxy type did not serialize to its variant name")
                })
        })
        .transpose()
}

fn raw_ascii_json(params: &Map<String, Json>) -> Result<String, CliError> {
    // json.dumps(..., separators=(',', ':')) uses ensure_ascii=True.
    let compact =
        serde_json::to_string(params).map_err(|error| CliError::failure(error.to_string()))?;
    let mut text = String::with_capacity(compact.len());
    for character in compact.chars() {
        if character.is_ascii() {
            text.push(character);
        } else {
            let mut units = [0u16; 2];
            for unit in character.encode_utf16(&mut units).iter() {
                use std::fmt::Write as _;
                write!(&mut text, "\\u{unit:04x}")
                    .map_err(|error| CliError::failure(error.to_string()))?;
            }
        }
    }
    Ok(text)
}

/// Build one member's portable replay command for a known public call spec.
/// Unknown targets cannot yield a command and return an explicit input error.
pub fn multisig_replay_command(
    ctx: &AppContext,
    request: &MultisigCallSpec,
    raw: &RawMultisig,
    member: AccountId,
    labels: &BTreeMap<AccountId, String>,
) -> Result<String, CliError> {
    let target = request
        .target
        .as_deref()
        .filter(|target| !target.is_empty())
        .ok_or_else(|| CliError::input("cannot replay a multisig call with an unknown target"))?;
    let address = member.to_string();
    let label = labels.get(&member).map(String::as_str).unwrap_or(&address);
    let backend = raw_contact_backend(ctx, &address).or_else(|| raw_contact_backend(ctx, label));
    let mut tokens = vec!["rtao".to_owned()];
    if let Some(backend) = backend {
        tokens.extend(["--signer".into(), backend]);
    }
    let (environment, network) = crate::prompt::shareable_network(&ctx.globals.network);
    if let Some(network) = network {
        tokens.extend(["-n".into(), network.into()]);
    }
    tokens.extend(["call".into(), target.into()]);
    if let Some(path) = &request.args_file {
        let path = path.to_str().ok_or_else(|| {
            CliError::input("args-file path cannot be represented in a replay command")
        })?;
        tokens.extend(["--args-file".into(), path.into()]);
    } else if !request.params.is_empty() {
        tokens.extend(["--args".into(), raw_ascii_json(&request.params)?]);
    }
    if request.sudo {
        tokens.push("--sudo".into());
    }
    if let Some(proxy) = request.proxy_for {
        // Raw account identity keeps Proxy.proxy bytes independent of books.
        tokens.extend(["--proxy-for".into(), proxy.to_string()]);
        if let Some(proxy_type) = proxy_type_name(request.force_proxy_type)? {
            tokens.extend(["--force-proxy-type".into(), proxy_type]);
        }
    }
    if let Some(preset) = &raw.preset {
        tokens.extend([
            "-w".into(),
            preset.clone(),
            "--signatory".into(),
            label.into(),
        ]);
    } else {
        let other_labels: Vec<_> = raw
            .signatories
            .iter()
            .filter(|account| **account != member)
            .map(|account| {
                labels
                    .get(account)
                    .cloned()
                    .unwrap_or_else(|| account.to_string())
            })
            .collect();
        let portable_others = !other_labels.is_empty()
            && !is_bittensor_address(label)
            && other_labels
                .iter()
                .all(|reference| !is_bittensor_address(reference));
        tokens.extend(["--multisig-threshold".into(), raw.threshold.to_string()]);
        if portable_others {
            tokens.extend(["--other-signatories".into(), other_labels.join(",")]);
        } else {
            tokens.extend([
                "--signatories".into(),
                raw.signatories
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
            ]);
        }
        tokens.extend(["-w".into(), label.into()]);
    }
    if request.role != rtao::signer::SignerRole::Coldkey {
        tokens.extend(["--signer".into(), request.role.as_str().into()]);
    }
    let command = tokens
        .iter()
        .map(|token| shell_quote(token))
        .collect::<Vec<_>>()
        .join(" ");
    Ok(format!("{environment}{command}"))
}

/// Build pending approval data from immutable bytes and known public state.
/// Unknown targets retain all receipt data but produce no replay commands.
pub fn pending_multisig_followup(
    ctx: &AppContext,
    request: &MultisigCallSpec,
    raw: &RawMultisig,
    ms: &Multisig,
    call: &CallBytes,
    pending: Option<&MultisigInfo>,
) -> Result<Json, CliError> {
    let labels = signatory_labels(ctx, raw)?;
    let approvals = pending
        .map(|pending| pending.approvals.as_slice())
        .unwrap_or(&[]);
    let remaining: Vec<_> = raw
        .signatories
        .iter()
        .copied()
        .filter(|account| !approvals.contains(account))
        .collect();
    let label = |account: AccountId| {
        raw_display_member(
            account,
            labels
                .get(&account)
                .map(String::as_str)
                .unwrap_or(&account.to_string()),
        )
    };
    let mut commands = Vec::new();
    for member in remaining.iter().filter(|_| {
        request
            .target
            .as_deref()
            .is_some_and(|target| !target.is_empty())
    }) {
        let address = member.to_string();
        let name = labels.get(member).map(String::as_str).unwrap_or(&address);
        commands.push(json!({
            "ss58": address,
            "label": raw_display_member(*member, name),
            "name": name,
            "command": multisig_replay_command(ctx, request, raw, *member, &labels)?,
        }));
    }
    let timepoint = pending
        .map(|pending| json!({"height":pending.when.height,"index":pending.when.index}))
        .unwrap_or_else(|| json!({"height":0,"index":0}));
    let timepoint_display = pending
        .map(|pending| format!("{}:{}", pending.when.height, pending.when.index))
        .unwrap_or_else(|| "?".into());
    let depositor = pending.map(|pending| pending.depositor.to_string());
    let depositor_label = pending.map(|pending| label(pending.depositor));
    let commands_available = !commands.is_empty();
    let decode_note = (!commands_available).then_some("call details unknown — the opening block may be pruned on this RPC node, the op may have used approve_as_multi (hash only), or local cache may be missing");
    let decode_hint = (!commands_available)
        .then_some("pass `--call-data 0x..` once, or re-open the op with a current SDK build");
    Ok(json!({
        "status":"pending", "target":request.target, "sudo":request.sudo,
        "proxy_for":request.proxy_for.map(|account|account.to_string()),
        "force_proxy_type":proxy_type_name(request.force_proxy_type)?, "params":request.params,
        "approvals":approvals.len(), "threshold":raw.threshold,
        "call_hash":format!("0x{}",hex::encode(call.hash())),
        "call_data":format!("0x{}",hex::encode(call.as_bytes())),
        "timepoint":timepoint, "timepoint_display":timepoint_display,
        "multisig_address":ms.address.to_string(), "multisig_preset":raw.preset,
        "depositor":depositor, "depositor_label":depositor_label,
        "approvals_so_far":approvals.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "approval_labels":approvals.iter().copied().map(label).collect::<Vec<_>>(),
        "remaining_signatories":remaining.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "remaining_labels":remaining.iter().copied().map(label).collect::<Vec<_>>(),
        "co_signer_commands":commands, "commands_available":commands_available,
        "decode_note":decode_note, "decode_hint":decode_hint,
    }))
}

/// Cache the composed public spec and attach accurate approval followup.
/// Caller inserts this returned value as result.data["multisig_followup"].
async fn multisig_followup(
    ctx: &AppContext,
    client: &Client,
    request: &RawCallRequest,
    ms: &Multisig,
    call: &CallBytes,
    result: &ExtrinsicResult,
    cache: &dyn RawCallCache,
) -> Result<Json, CliError> {
    let raw = request
        .multisig
        .as_ref()
        .ok_or_else(|| CliError::input("multisig followup requires multisig settings"))?;
    multisig_followup_for_spec(
        ctx,
        client,
        MultisigFollowup {
            spec: &MultisigCallSpec::from(request),
            group: raw,
            multisig: ms,
            call,
            result,
        },
        cache,
    )
    .await
}

/// Persist a known public spec and report executed, pending or submitted status.
/// This also supports semantic intent calls whose decoded target is unknown:
/// those calls retain their actual bytes and never shadow a later cache decode.
pub async fn multisig_followup_for_spec(
    ctx: &AppContext,
    client: &Client,
    followup: MultisigFollowup<'_>,
    cache: &dyn RawCallCache,
) -> Result<Json, CliError> {
    let MultisigFollowup {
        spec: request,
        group: raw,
        multisig: ms,
        call,
        result,
    } = followup;
    let hash = format!("0x{}", hex::encode(call.hash()));
    let data = format!("0x{}", hex::encode(call.as_bytes()));
    let args_file = request
        .args_file
        .as_ref()
        .map(|path| {
            path.to_str().map(str::to_owned).ok_or_else(|| {
                CliError::input("args-file path cannot be represented in the multisig cache")
            })
        })
        .transpose()?;
    let spec = json!({
        "target":request.target, "params":request.params, "args_file":args_file,
        "sudo":request.sudo, "proxy_for":request.proxy_for.map(|account|account.to_string()),
        "force_proxy_type":proxy_type_name(request.force_proxy_type)?, "call_data":data, "call_hash":hash,
        "threshold":raw.threshold, "signatories":raw.signatories.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "multisig_address":ms.address.to_string(), "multisig_preset":raw.preset,
        "network":ctx.globals.network,
    });
    if request
        .target
        .as_deref()
        .is_some_and(|target| !target.is_empty())
    {
        cache.save(hash.clone(), spec).await?;
    }
    let common = |status| {
        json!({
            "status":status, "target":request.target, "sudo":request.sudo,
            "proxy_for":request.proxy_for.map(|account|account.to_string()),
            "call_hash":hash, "call_data":data,
            "multisig_address":ms.address.to_string(), "multisig_preset":raw.preset,
        })
    };
    if multisig_executed(result) {
        return Ok(common("executed"));
    }
    if let Some(pending) = raw_query_pending(client, ms, call).await? {
        return pending_multisig_followup(ctx, request, raw, ms, call, Some(&pending));
    }
    if raw_has_multisig_event(result) {
        let mut followup = pending_multisig_followup(ctx, request, raw, ms, call, None)?;
        followup["decode_note"] =
            json!("approval recorded on-chain but pending state is not visible yet");
        followup["decode_hint"] =
            json!("run `rtao multisig pending` to inspect co-signer commands");
        return Ok(followup);
    }
    let mut followup = common("submitted");
    followup["decode_note"] = json!("approval submitted but pending state is not visible yet");
    followup["decode_hint"] =
        json!("run `rtao multisig pending` shortly to inspect co-signer commands");
    Ok(followup)
}
