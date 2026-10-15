// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/evm/precompiles_cmd.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/evm/precompiles.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Native precompile discovery and read-only or signed contract calls.

use std::{future::Future, pin::Pin};

use clap::{Arg, ArgMatches, Command};
use rtao::evm::{ContractArtifact, Precompile};
use serde_json::{Map, Value, json};

use crate::{
    context::AppContext,
    error::CliError,
    globals::{Tier, with_globals},
    output::TableSpec,
};

use super::{failure, key_info, optional, rpc, submit_evm_tx, tao_to_wei};

/// Build the three pinned precompile commands.
pub fn commands() -> Vec<Command> {
    vec![
        with_globals(
            Command::new("precompiles").about("List Bittensor's native EVM precompiles."),
            Tier::Read,
        ),
        with_globals(
            Command::new("abi")
                .about("Show a precompile address and its ABI JSON.")
                .arg(Arg::new("name").required(true)),
            Tier::Read,
        ),
        with_globals(
            Command::new("call")
                .about("Call a precompile or contract; views need no signing key.")
                .arg(Arg::new("name").required(true))
                .arg(Arg::new("function"))
                .arg(
                    Arg::new("args")
                        .num_args(0..)
                        .allow_negative_numbers(true)
                        .help("Arguments in declaration order; arrays accept JSON or comma-separated values."),
                )
                .arg(Arg::new("abi_path").long("abi"))
                .arg(Arg::new("value_tao").long("value-tao"))
                .arg(Arg::new("evm_key").long("evm-key"))
                .arg(Arg::new("rpc_url").long("rpc-url")),
            Tier::Tx,
        ),
    ]
}

/// Reserve local fields from global wallet target aliases.
pub fn reserved(name: &str) -> Vec<&'static str> {
    match name {
        "abi" => vec!["name"],
        "call" => vec![
            "name",
            "function",
            "args",
            "abi_path",
            "value_tao",
            "evm_key",
            "rpc_url",
        ],
        _ => Vec::new(),
    }
}

/// A read-only EVM operation, injectable without a network or a keyfile.
pub type ReadFuture<'a> = Pin<Box<dyn Future<Output = Result<Vec<u8>, CliError>> + Send + 'a>>;

/// Boundary used only after a function has been identified as view or pure.
pub trait ReadBackend: Send + Sync {
    /// Execute exactly one `eth_call`; this boundary cannot sign or submit.
    fn eth_call<'a>(
        &'a self,
        ctx: &'a AppContext,
        rpc_url: Option<&'a str>,
        tx: Value,
    ) -> ReadFuture<'a>;
}

struct NativeRead;
impl ReadBackend for NativeRead {
    fn eth_call<'a>(
        &'a self,
        ctx: &'a AppContext,
        rpc_url: Option<&'a str>,
        tx: Value,
    ) -> ReadFuture<'a> {
        Box::pin(async move {
            let (_, rpc) = rpc(ctx, rpc_url).await?;
            rpc.eth_call(&tx).await.map_err(failure)
        })
    }
}

fn input(error: impl std::fmt::Display) -> CliError {
    CliError::input(error.to_string())
}

/// Validate aggregate input before copying large CLI argument lists.
pub(super) fn arguments(leaf: &ArgMatches) -> Result<Vec<Value>, CliError> {
    let Some(args) = leaf.get_many::<String>("args") else {
        return Ok(Vec::new());
    };
    let mut total = 0usize;
    args.map(|raw| {
        total = total
            .checked_add(raw.len())
            .ok_or_else(|| input("ABI arguments exceed 16 MiB limit"))?;
        if total > rtao::evm::precompiles::MAX_ABI_BYTES {
            return Err(input("ABI arguments exceed 16 MiB limit"));
        }
        Ok(Value::String(raw.clone()))
    })
    .collect()
}

struct CallTarget {
    label: String,
    address: String,
    artifact: ContractArtifact,
    precompile: Option<&'static Precompile>,
}

impl CallTarget {
    fn load(name: &str, abi_path: Option<&str>) -> Result<Self, CliError> {
        if name.starts_with("0x") || rtao::evm::is_h160(&format!("0x{name}")) {
            let address = rtao::evm::normalize_h160(name).map_err(input)?;
            // Typed validation also refuses a historical text shape containing
            // whitespace rather than twenty actual address bytes.
            rtao::evm::addresses::parse_h160(&address).map_err(input)?;
            let path = abi_path.ok_or_else(|| {
                input(format!(
                    "{address} is a contract address — pass its interface with --abi"
                ))
                .with_help("accepts a Hardhat/Foundry artifact JSON or a bare ABI array")
            })?;
            let artifact = rtao::evm::load_artifact(path).map_err(input)?;
            if artifact.abi.is_none() {
                return Err(input(format!("{path} contains bytecode but no ABI")));
            }
            return Ok(Self {
                label: address.clone(),
                address,
                artifact,
                precompile: None,
            });
        }
        let precompile = rtao::evm::get_precompile(name).map_err(input)?;
        Ok(Self {
            label: precompile.name.into(),
            address: precompile.address(),
            artifact: ContractArtifact::from_text(precompile.abi_json(), precompile.name)
                .map_err(input)?,
            precompile: Some(precompile),
        })
    }

    fn table(&self) -> Result<TableSpec, CliError> {
        let mut functions = self.artifact.functions();
        functions.sort_by(|left, right| left.name.cmp(&right.name));
        let rows = functions
            .into_iter()
            .map(|function| {
                let value = serde_json::to_value(function).map_err(failure)?;
                let mut mutability = value
                    .get("stateMutability")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                if let Some(note) = self.precompile.and_then(|precompile| {
                    rtao::evm::function_deprecation(precompile.name, &function.name)
                }) {
                    mutability = format!("{mutability} (deprecated: {note})");
                }
                let params = |key: &str, names: bool| {
                    value
                        .get(key)
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(|param| {
                            let ty = param
                                .get("type")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            if names {
                                format!(
                                    "{ty} {}",
                                    param
                                        .get("name")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                )
                            } else {
                                ty.into()
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                Ok(vec![
                    json!(function.name),
                    json!(params("inputs", true)),
                    json!(params("outputs", false)),
                    json!(mutability),
                ])
            })
            .collect::<Result<Vec<_>, CliError>>()?;
        Ok(TableSpec::new(
            format!("{} at {}", self.label, self.address),
            ["function", "inputs", "outputs", "mutability"]
                .map(str::to_owned)
                .to_vec(),
            rows,
        ))
    }
}

struct PreparedCall {
    target: CallTarget,
    function: String,
    data: Vec<u8>,
    view: bool,
    deprecation: Option<&'static str>,
    caller_role: Option<&'static str>,
    registers_network: bool,
    preview: Map<String, Value>,
    summary: String,
}

impl PreparedCall {
    fn new(target: CallTarget, function: String, args: &[Value]) -> Result<Self, CliError> {
        let function_abi = target.artifact.function(&function).map_err(input)?;
        let function_value = serde_json::to_value(function_abi).map_err(failure)?;
        let view = matches!(
            function_value
                .get("stateMutability")
                .and_then(Value::as_str),
            Some("view" | "pure")
        );
        // Metadata uses the ABI name; encoding and decoding keep the full selection.
        let deprecation = target.precompile.and_then(|precompile| {
            rtao::evm::function_deprecation(precompile.name, &function_abi.name)
        });
        let caller_role = target
            .precompile
            .and_then(|precompile| rtao::evm::caller_role(precompile.name, &function_abi.name));
        let registers_network = target.precompile.is_some_and(|precompile| {
            precompile.name == "subnet" && function_abi.name == "registerNetwork"
        });
        let encoded = match target.precompile {
            Some(precompile) => precompile.encode_call(&function, args),
            None => rtao::evm::encode_call(function_abi, args),
        }
        .map_err(input)?;
        let data = hex::decode(encoded.strip_prefix("0x").unwrap_or(&encoded)).map_err(input)?;
        // Python treats argument descriptions as best-effort human preview.
        // Native amount rejection already happened during precompile encoding.
        let preview = rtao::evm::describe_arguments(function_abi, args)
            .unwrap_or_default()
            .into_iter()
            .map(|(key, value)| (format!("arg {key}"), Value::String(value)))
            .collect();
        let summary = format!(
            "call {}.{function}({})",
            target.label,
            args.iter()
                .map(|value| value.as_str().unwrap_or_default())
                .collect::<Vec<_>>()
                .join(", ")
        );
        Ok(Self {
            target,
            function,
            data,
            view,
            deprecation,
            caller_role,
            registers_network,
            preview,
            summary,
        })
    }

    fn decode(&self, raw: &[u8]) -> Result<Value, CliError> {
        let function = self
            .target
            .artifact
            .function(&self.function)
            .map_err(input)?;
        let mut values =
            rtao::evm::precompiles::decode_result_bytes(function, raw).map_err(failure)?;
        Ok(if values.len() == 1 {
            values.remove(0)
        } else {
            Value::Array(values)
        })
    }
}

fn catalog() -> TableSpec {
    let mut entries = rtao::evm::PRECOMPILES.iter().collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.index);
    let mut records = Vec::new();
    let rows = entries.into_iter().map(|entry| {
        records.push(json!({"name":entry.name,"address":entry.address(),"description":entry.description,"deprecated":entry.deprecated}));
        let description = if entry.deprecated { format!("{} (deprecated)", entry.description) } else { entry.description.into() };
        vec![json!(entry.name), json!(entry.address()), json!(description)]
    }).collect();
    let mut table = TableSpec::new(
        "Bittensor EVM precompiles",
        ["name", "address", "description"]
            .map(str::to_owned)
            .to_vec(),
        rows,
    );
    table.records = Some(Value::Array(records));
    table
}

/// Execute catalog discovery, function listing, read-only calls or signed writes.
pub async fn run(ctx: &AppContext, name: &str, leaf: &ArgMatches) -> Result<(), CliError> {
    run_with_backend(ctx, name, leaf, &NativeRead).await
}

/// Execute with injected read-only RPC, retaining real parsing and ABI handling.
pub async fn run_with_backend(
    ctx: &AppContext,
    name: &str,
    leaf: &ArgMatches,
    backend: &dyn ReadBackend,
) -> Result<(), CliError> {
    match name {
        "precompiles" => return ctx.output.table(&catalog()).map_err(failure),
        "abi" => {
            let name = optional(leaf, "name").ok_or_else(|| input("missing precompile name"))?;
            let precompile = rtao::evm::get_precompile(name).map_err(input)?;
            let abi: Value = serde_json::from_str(precompile.abi_json()).map_err(failure)?;
            return ctx
                .output
                .value(&json!({"address":precompile.address(),"abi":abi}))
                .map_err(failure);
        }
        "call" => {}
        _ => return Err(input("unknown EVM precompile command")),
    }
    let name = optional(leaf, "name")
        .ok_or_else(|| input("missing call target"))?
        .to_owned();
    let abi = optional(leaf, "abi_path").map(str::to_owned);
    let target = rtao::core::offload(move || CallTarget::load(&name, abi.as_deref()))
        .await
        .map_err(failure)??;
    let Some(function) = optional(leaf, "function") else {
        let table = rtao::core::offload(move || target.table())
            .await
            .map_err(failure)??;
        return ctx.output.table(&table).map_err(failure);
    };
    let args = arguments(leaf)?;
    let function_owned = function.to_owned();
    let mut prepared =
        rtao::core::offload(move || PreparedCall::new(target, function_owned, &args))
            .await
            .map_err(failure)??;
    if let Some(note) = prepared.deprecation {
        ctx.output
            .message(&format!(
                "warning: {}.{function} is deprecated — {note}",
                prepared.target.label
            ))
            .map_err(failure)?;
    }
    if prepared.view {
        let data = if prepared.data.len() > 64 * 1024 {
            let bytes = std::mem::take(&mut prepared.data);
            rtao::core::offload(move || hex::encode(bytes))
                .await
                .map_err(failure)?
        } else {
            hex::encode(&prepared.data)
        };
        let tx = json!({"to":prepared.target.address,"data":format!("0x{data}")});
        let raw = backend.eth_call(ctx, optional(leaf, "rpc_url"), tx).await?;
        let result = rtao::core::offload(move || prepared.decode(&raw))
            .await
            .map_err(failure)??;
        return ctx.output.value(&result).map_err(failure);
    }
    if let Some(role) = prepared.caller_role {
        let info = key_info(ctx, optional(leaf, "evm_key")).await?;
        prepared.preview.insert(
            "caller mirror".into(),
            json!(format!("{} acts as the {role}", info.ss58_mirror)),
        );
    }
    if prepared.registers_network {
        ctx.output.message("warning: the caller mirror becomes the subnet owner, and some owner operations (notably start-call, which activates emissions) have no precompile — they require a native coldkey signature").map_err(failure)?;
    }
    let value = optional(leaf, "value_tao")
        .map(tao_to_wei)
        .transpose()?
        .unwrap_or_default();
    submit_evm_tx(
        ctx,
        optional(leaf, "evm_key"),
        Some(&prepared.target.address),
        value,
        prepared.data,
        prepared.summary,
        optional(leaf, "rpc_url"),
        prepared.preview,
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transfer_target() -> Result<CallTarget, CliError> {
        let artifact = ContractArtifact::from_text(
            r#"[{"type":"function","name":"transfer","inputs":[{"type":"address","name":"to"},{"type":"uint256","name":"amount"}],"outputs":[{"name":"","type":"bool"}],"stateMutability":"nonpayable"}]"#,
            "fixture.json",
        ).map_err(input)?;
        Ok(CallTarget {
            label: "fixture".into(),
            address: "0x1111111111111111111111111111111111111111".into(),
            artifact,
            precompile: None,
        })
    }

    #[test]
    fn generic_contract_amount_keeps_all_256_bits_and_known_erc20_selector() -> Result<(), CliError>
    {
        let prepared = PreparedCall::new(
            transfer_target()?,
            "transfer".into(),
            &[
                json!("0x2222222222222222222222222222222222222222"),
                json!(format!("0x{}", "f".repeat(64))),
            ],
        )?;
        assert!(!prepared.view);
        assert_eq!(prepared.deprecation, None);
        assert_eq!(prepared.caller_role, None);
        assert!(!prepared.registers_network);
        assert_eq!(
            hex::encode(&prepared.data),
            format!(
                "a9059cbb{}{}{}",
                "0".repeat(24),
                "22".repeat(20),
                "f".repeat(64)
            )
        );
        let mut encoded_bool = vec![0; 32];
        encoded_bool[31] = 1;
        assert_eq!(prepared.decode(&encoded_bool)?, json!(true));
        Ok(())
    }

    #[test]
    fn canonical_view_keeps_deprecation_calldata_and_result() -> Result<(), CliError> {
        let args = [json!(1), json!(2)];
        let bare = PreparedCall::new(
            CallTarget::load("metagraph", None)?,
            "getTrust".into(),
            &args,
        )?;
        let signature = "getTrust(uint16,uint16)";
        let canonical = PreparedCall::new(
            CallTarget::load("metagraph", None)?,
            signature.into(),
            &args,
        )?;
        assert_eq!(canonical.function, signature);
        assert_eq!(canonical.data, bare.data);
        assert_eq!(canonical.preview, bare.preview);
        for prepared in [&bare, &canonical] {
            assert!(prepared.view);
            assert_eq!(
                prepared.deprecation,
                Some("always returns 0 on chain; trust is no longer computed")
            );
            assert_eq!(prepared.caller_role, None);
            assert!(!prepared.registers_network);
            assert_eq!(prepared.decode(&[0; 32])?, json!(0));
        }
        Ok(())
    }

    #[test]
    fn canonical_stake_keeps_caller_role_and_calldata() -> Result<(), CliError> {
        let args = [
            json!(format!("0x{}", "11".repeat(32))),
            json!(1_000_000_000),
            json!(1),
        ];
        let bare = PreparedCall::new(
            CallTarget::load("staking-v2", None)?,
            "addStake".into(),
            &args,
        )?;
        let signature = "addStake(bytes32,uint256,uint256)";
        let canonical = PreparedCall::new(
            CallTarget::load("staking-v2", None)?,
            signature.into(),
            &args,
        )?;
        assert_eq!(canonical.function, signature);
        assert_eq!(canonical.data, bare.data);
        assert_eq!(canonical.preview, bare.preview);
        for prepared in [&bare, &canonical] {
            assert!(!prepared.view);
            assert_eq!(prepared.deprecation, None);
            assert_eq!(
                prepared.caller_role,
                Some("coldkey (owns the resulting stake)")
            );
            assert!(!prepared.registers_network);
        }
        Ok(())
    }

    #[test]
    fn canonical_network_overloads_keep_owner_notices_and_selection() -> Result<(), CliError> {
        let mut selectors = Vec::new();
        for (signature, string_count) in [
            ("registerNetwork(bytes32)", 0),
            (
                "registerNetwork(bytes32,string,string,string,string,string,string,string)",
                7,
            ),
            (
                "registerNetwork(bytes32,string,string,string,string,string,string,string,string)",
                8,
            ),
        ] {
            let mut args = vec![json!(format!("0x{}", "11".repeat(32)))];
            args.extend((0..string_count).map(|_| json!("fixture")));
            let prepared =
                PreparedCall::new(CallTarget::load("subnet", None)?, signature.into(), &args)?;
            assert_eq!(prepared.function, signature);
            assert!(!prepared.view);
            assert_eq!(prepared.deprecation, None);
            assert_eq!(
                prepared.caller_role,
                Some("subnet owner coldkey (becomes the owner)")
            );
            assert!(prepared.registers_network);
            selectors.push(prepared.data[..4].to_vec());
            if string_count == 0 {
                let bare = PreparedCall::new(
                    CallTarget::load("subnet", None)?,
                    "registerNetwork".into(),
                    &args,
                )?;
                assert_eq!(prepared.data, bare.data);
                assert_eq!(prepared.preview, bare.preview);
                assert_eq!(prepared.caller_role, bare.caller_role);
                assert_eq!(prepared.registers_network, bare.registers_network);
            }
        }
        selectors.sort();
        selectors.dedup();
        assert_eq!(selectors.len(), 3);
        Ok(())
    }

    #[test]
    fn native_precompile_checks_rao_at_exact_u64_boundary() -> Result<(), CliError> {
        let args = [
            json!(format!("0x{}", "11".repeat(32))),
            json!(u64::MAX.to_string()),
            json!(1),
        ];
        let prepared = PreparedCall::new(
            CallTarget::load("staking-v2", None)?,
            "addStake".into(),
            &args,
        )?;
        // Selector, hotkey, exact native amount, then netuid.
        assert_eq!(prepared.data.len(), 4 + 32 * 3);
        assert_eq!(
            &prepared.data[4 + 32..4 + 64],
            &[
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255,
                255, 255, 255, 255, 255, 255
            ]
        );
        let mut overflow = args;
        overflow[1] = json!("18446744073709551616");
        assert!(
            PreparedCall::new(
                CallTarget::load("staking-v2", None)?,
                "addStake".into(),
                &overflow
            )
            .is_err()
        );
        Ok(())
    }
}
