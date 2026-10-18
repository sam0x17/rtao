//! Authenticate owner policy against Python allowlists, Rust setters and metadata.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};
use quote::ToTokens;
use rtao_core::{ir::MetadataIr, runtime::Runtime};
use scale_info::TypeDef;
use serde_json::{Value, json};
use syn::visit::{self, Visit};
use toml_edit::DocumentMut;

use super::{Resolver, header, pretty, rust_string, rustfmt, value};

const PYTHON: &str = "sdk/python/bittensor/intents/hyperparameters.py";
const UNITS: &str = "sdk/python/bittensor/hyperparams.py";
const ADMIN: &str = "pallets/admin-utils/src/lib.rs";
const TEMPO: &str = "pallets/subtensor/src/coinbase/tempo_control.rs";
const RATE: &str = "pallets/subtensor/src/utils/rate_limiting.rs";
const MISC: &str = "pallets/subtensor/src/utils/misc.rs";
const COINBASE: &str = "pallets/subtensor/src/coinbase/run_coinbase.rs";

fn tokens(node: &impl ToTokens) -> String {
    node.to_token_stream().to_string()
}

fn dictionary(document: &Value, name: &str) -> Result<BTreeMap<String, Value>> {
    let mut result = BTreeMap::new();
    for pair in value(document, name)?["dict"]
        .as_array()
        .context("owner table is not a dictionary")?
    {
        let pair = pair.as_array().context("owner table row is not a pair")?;
        ensure!(pair.len() == 2, "owner table row length changed");
        let name = pair[0].as_str().context("owner table key is not text")?;
        ensure!(
            result.insert(name.to_owned(), pair[1].clone()).is_none(),
            "duplicate owner table key {name}"
        );
    }
    Ok(result)
}

fn methods(items: &[syn::Item]) -> Result<BTreeMap<String, syn::ImplItemFn>> {
    let mut result = BTreeMap::new();
    for item in items {
        match item {
            syn::Item::Mod(module) => {
                if module.ident != "pallet" {
                    continue;
                }
                ensure!(
                    module.attrs.iter().all(|a| !a.path().is_ident("cfg")),
                    "conditional policy module"
                );
                if let Some((_, items)) = &module.content {
                    for (name, method) in methods(items)? {
                        ensure!(
                            result.insert(name, method).is_none(),
                            "duplicate policy method"
                        );
                    }
                }
            }
            syn::Item::Impl(implementation) => {
                for item in &implementation.items {
                    let syn::ImplItem::Fn(method) = item else {
                        continue;
                    };
                    if method.sig.ident.to_string().starts_with("sudo_")
                        || method.sig.ident.to_string().starts_with("do_set_")
                        || matches!(
                            method.sig.ident.to_string().as_str(),
                            "get_alpha_values"
                                | "get_epoch_consensus"
                                | "is_in_admin_freeze_window"
                                | "ensure_admin_window_open"
                                | "blocks_until_next_auto_epoch"
                                | "ensure_sn_owner_or_root_with_limits"
                                | "record_owner_rl"
                        )
                    {
                        ensure!(
                            implementation.attrs.iter().all(
                                |a| !a.path().is_ident("cfg") && !a.path().is_ident("cfg_attr")
                            ),
                            "conditional policy implementation"
                        );
                        ensure!(
                            method.attrs.iter().all(
                                |a| !a.path().is_ident("cfg") && !a.path().is_ident("cfg_attr")
                            ),
                            "conditional policy setter"
                        );
                        ensure!(
                            result
                                .insert(method.sig.ident.to_string(), method.clone())
                                .is_none(),
                            "duplicate policy setter"
                        );
                    }
                }
            }
            _ => {}
        }
    }
    Ok(result)
}

#[derive(Default)]
struct Calls<'a>(Vec<&'a syn::ExprCall>);
impl<'ast> Visit<'ast> for Calls<'ast> {
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        self.0.push(call);
        visit::visit_expr_call(self, call);
    }
}

fn call_name(call: &syn::ExprCall) -> Option<String> {
    let syn::Expr::Path(path) = &*call.func else {
        return None;
    };
    path.path
        .segments
        .last()
        .map(|segment| segment.ident.to_string())
}

fn calls(method: &syn::ImplItemFn) -> Calls<'_> {
    let mut calls = Calls::default();
    calls.visit_block(&method.block);
    calls
}

fn single_call<'a>(calls: &'a Calls<'a>, name: &str) -> Result<&'a syn::ExprCall> {
    let mut found = calls
        .0
        .iter()
        .copied()
        .filter(|call| call_name(call).as_deref() == Some(name));
    let call = found
        .next()
        .with_context(|| format!("owner policy missing {name}"))?;
    ensure!(found.next().is_none(), "owner policy repeats {name}");
    Ok(call)
}

fn check_target(call: &syn::ExprCall, name: &str, subtensor: bool) -> Result<()> {
    let expected = if subtensor {
        format!("pallet_subtensor :: Pallet :: < T > :: {name}")
    } else {
        format!("Self :: {name}")
    };
    ensure!(
        tokens(&call.func) == expected,
        "owner policy helper {name} target changed"
    );
    Ok(())
}

fn params(method: &syn::ImplItemFn) -> Result<Vec<String>> {
    let names = method
        .sig
        .inputs
        .iter()
        .map(|arg| {
            let syn::FnArg::Typed(arg) = arg else {
                bail!("owner setter has receiver");
            };
            let syn::Pat::Ident(name) = &*arg.pat else {
                bail!("owner setter argument is not named");
            };
            ensure!(
                name.by_ref.is_none() && name.subpat.is_none(),
                "owner setter argument shape changed"
            );
            Ok(name.ident.to_string())
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        names.len() >= 3 && names[0] == "origin" && names[1] == "netuid",
        "owner setter origin/netuid signature changed"
    );
    Ok(names[2..].to_vec())
}

fn limit(expression: &syn::Expr, variants: &BTreeSet<String>) -> Result<Value> {
    let syn::Expr::Reference(reference) = expression else {
        bail!("owner limit is not a slice");
    };
    let syn::Expr::Array(array) = &*reference.expr else {
        bail!("owner limit slice is not literal");
    };
    ensure!(
        array.elems.len() == 1,
        "owner limit must have exactly one key"
    );
    let expression = &array.elems[0];
    let (path, kind) = match expression {
        syn::Expr::MethodCall(call) if call.method == "into" && call.args.is_empty() => {
            let syn::Expr::Path(path) = &*call.receiver else {
                bail!("owner Hyperparameter key is not a path");
            };
            (&path.path, "owner")
        }
        syn::Expr::Path(path) => (&path.path, "transaction"),
        _ => bail!("owner limit expression changed"),
    };
    ensure!(
        path.segments.len() == 2
            && path.segments[0].ident
                == if kind == "owner" {
                    "Hyperparameter"
                } else {
                    "TransactionType"
                },
        "owner limit key path changed"
    );
    let variant = path.segments[1].ident.to_string();
    if kind == "owner" {
        ensure!(
            variants.contains(&variant),
            "unknown Hyperparameter variant {variant}"
        );
    }
    Ok(json!({"kind":kind,"variant":variant}))
}

fn direct_policy(method: &syn::ImplItemFn, variants: &BTreeSet<String>) -> Result<Value> {
    let body = calls(method);
    check_target(
        single_call(&body, "ensure_admin_window_open")?,
        "ensure_admin_window_open",
        true,
    )?;
    let guards = body
        .0
        .iter()
        .filter(|call| call_name(call).as_deref() == Some("ensure_sn_owner_or_root_with_limits"))
        .count();
    if guards == 0 {
        check_target(
            single_call(&body, "ensure_subnet_owner_or_root")?,
            "ensure_subnet_owner_or_root",
            true,
        )?;
        ensure!(
            !tokens(&method.block).contains("record_owner_rl")
                && !tokens(&method.block).contains("passes_rate_limit"),
            "unclassified owner cooldown"
        );
        return Ok(json!({"kind":"none"}));
    }
    let guard = single_call(&body, "ensure_sn_owner_or_root_with_limits")?;
    let record = single_call(&body, "record_owner_rl")?;
    check_target(guard, "ensure_sn_owner_or_root_with_limits", true)?;
    check_target(record, "record_owner_rl", true)?;
    ensure!(
        guard.args.len() == 3
            && record.args.len() == 3
            && tokens(&guard.args[1]) == "netuid"
            && tokens(&record.args[1]) == "netuid",
        "owner limit arguments changed"
    );
    let policy = limit(&guard.args[2], variants)?;
    ensure!(
        policy == limit(&record.args[2], variants)?,
        "owner guard/record cooldown keys differ"
    );
    Ok(policy)
}

fn delegated_policy(
    method: &syn::ImplItemFn,
    delegate: &syn::ImplItemFn,
    variants: &BTreeSet<String>,
) -> Result<Value> {
    let body = calls(method);
    let call = single_call(&body, &delegate.sig.ident.to_string())?;
    check_target(call, &delegate.sig.ident.to_string(), true)?;
    let expected = std::iter::once("origin".to_owned())
        .chain(std::iter::once("netuid".to_owned()))
        .chain(params(method)?)
        .collect::<Vec<_>>();
    ensure!(
        call.args.iter().map(tokens).collect::<Vec<_>>() == expected,
        "delegated owner setter arguments differ"
    );
    let body = calls(delegate);
    check_target(
        single_call(&body, "ensure_subnet_owner_or_root")?,
        "ensure_subnet_owner_or_root",
        false,
    )?;
    check_target(
        single_call(&body, "ensure_admin_window_open")?,
        "ensure_admin_window_open",
        false,
    )?;
    let mut tx = delegate.block.stmts.iter().filter_map(|stmt| {
        let syn::Stmt::Local(local) = stmt else {
            return None;
        };
        if !matches!(&local.pat,syn::Pat::Ident(pat) if pat.ident == "tx") {
            return None;
        }
        local.init.as_ref().map(|init| &*init.expr)
    });
    let tx_expr = tx.next().context("delegated owner transaction missing")?;
    ensure!(tx.next().is_none(), "duplicate delegated owner transaction");
    let policy = match tx_expr {
        syn::Expr::Path(path) if tokens(path) == "TransactionType :: TempoUpdate" => {
            json!({"kind":"transaction","variant":"TempoUpdate"})
        }
        syn::Expr::Call(call)
            if tokens(&call.func) == "TransactionType :: OwnerHyperparamUpdate"
                && call.args.len() == 1 =>
        {
            let syn::Expr::Path(path) = &call.args[0] else {
                bail!("delegated Hyperparameter is not a path");
            };
            ensure!(
                path.path.segments.len() == 2 && path.path.segments[0].ident == "Hyperparameter",
                "delegated Hyperparameter path changed"
            );
            let variant = path.path.segments[1].ident.to_string();
            ensure!(
                variants.contains(&variant),
                "unknown delegated Hyperparameter"
            );
            json!({"kind":"owner","variant":variant})
        }
        _ => bail!("delegated owner transaction changed"),
    };
    let text: String = tokens(&delegate.block)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    ensure!(
        text.contains("tx.passes_rate_limit_on_subnet::<T>(who,netuid)")
            && text.contains("tx.set_last_block_on_subnet::<T>(who,netuid,now)"),
        "delegated owner rate check/record changed"
    );
    Ok(policy)
}

fn transaction_codes(source: &str) -> Result<(BTreeMap<String, u16>, BTreeSet<String>)> {
    let file = syn::parse_file(source)?;
    let enumeration = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Enum(item) if item.ident == "Hyperparameter" => Some(item),
            _ => None,
        })
        .context("Hyperparameter enum missing")?;
    let variants = enumeration
        .variants
        .iter()
        .map(|variant| variant.ident.to_string())
        .collect();
    let conversion = file
        .items
        .iter()
        .find_map(|item| {
            let syn::Item::Impl(item) = item else {
                return None;
            };
            if item
                .trait_
                .as_ref()
                .is_some_and(|(_, path, _)| tokens(path) == "From < TransactionType >")
                && tokens(&item.self_ty) == "u16"
            {
                Some(item)
            } else {
                None
            }
        })
        .context("TransactionType code conversion missing")?;
    let [syn::ImplItem::Fn(method)] = conversion.items.as_slice() else {
        bail!("transaction conversion changed");
    };
    let [syn::Stmt::Expr(syn::Expr::Match(mapping), None)] = method.block.stmts.as_slice() else {
        bail!("transaction conversion must be a match");
    };
    let mut codes = BTreeMap::new();
    for arm in &mapping.arms {
        ensure!(
            arm.guard.is_none() && arm.attrs.is_empty(),
            "conditional transaction code"
        );
        let path = match &arm.pat {
            syn::Pat::Path(path) => &path.path,
            syn::Pat::TupleStruct(path) => &path.path,
            _ => bail!("transaction code pattern changed"),
        };
        ensure!(
            path.segments.len() == 2 && path.segments[0].ident == "TransactionType",
            "transaction code path changed"
        );
        let syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Int(code),
            ..
        }) = &*arm.body
        else {
            bail!("transaction code is not literal");
        };
        ensure!(
            codes
                .insert(path.segments[1].ident.to_string(), code.base10_parse()?)
                .is_none(),
            "duplicate transaction code"
        );
    }
    ensure!(
        codes.values().collect::<BTreeSet<_>>().len() == codes.len(),
        "duplicate transaction code value"
    );
    Ok((codes, variants))
}

fn rate_kinds(source: &str) -> Result<BTreeMap<String, Value>> {
    // These are semantic contracts, read from the corresponding match arms.
    // A runtime change outside these supported forms stops extraction.
    let file = syn::parse_file(source)?;
    let implementation = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Impl(item)
                if tokens(&item.self_ty) == "TransactionType" && item.trait_.is_none() =>
            {
                Some(item)
            }
            _ => None,
        })
        .context("TransactionType methods missing")?;
    let mut result = BTreeMap::new();
    for member in &implementation.items {
        let syn::ImplItem::Fn(method) = member else {
            continue;
        };
        if !matches!(
            method.sig.ident.to_string().as_str(),
            "rate_limit" | "rate_limit_on_subnet"
        ) {
            continue;
        }
        let [syn::Stmt::Expr(syn::Expr::Match(mapping), None)] = method.block.stmts.as_slice()
        else {
            bail!("rate method shape changed");
        };
        for arm in &mapping.arms {
            let syn::Pat::Path(path) = &arm.pat else {
                continue;
            };
            if path.path.segments.len() != 2 || path.path.segments[0].ident != "Self" {
                continue;
            }
            let variant = path.path.segments[1].ident.to_string();
            let text = tokens(&arm.body);
            let policy = match variant.as_str() {
                "SetWeightsVersionKey" => {
                    ensure!(
                        text == "(Tempo :: < T > :: get (netuid) as u64) . saturating_mul (WeightsVersionKeyRateLimit :: < T > :: get ())",
                        "weights version rate expression changed"
                    );
                    Some(json!({"kind":"tempo_multiplier","storage":"WeightsVersionKeyRateLimit"}))
                }
                "TempoUpdate" => {
                    ensure!(text == "MIN_TEMPO as u64", "tempo rate expression changed");
                    Some(json!({"kind":"fixed_constant","constant":"MinTempo"}))
                }
                "MechanismCountUpdate" | "MechanismEmission" | "MaxUidsTrimming" => {
                    let syn::Expr::Call(call) = &*arm.body else {
                        bail!("runtime default rate expression changed");
                    };
                    let syn::Expr::Path(path) = &*call.func else {
                        bail!("runtime default rate path missing");
                    };
                    ensure!(
                        call.args.is_empty()
                            && path.path.segments.len() == 2
                            && path.path.segments[1].ident == "get",
                        "runtime default get changed"
                    );
                    Some(
                        json!({"kind":"runtime_default","constant":path.path.segments[0].ident.to_string()}),
                    )
                }
                _ => None,
            };
            if let Some(policy) = policy {
                ensure!(
                    result.insert(variant, policy).is_none(),
                    "duplicate rate policy"
                );
            }
        }
    }
    ensure!(result.len() == 5, "owner transaction rate policy missing");
    let text = tokens(&file);
    ensure!(text.contains("let epochs = OwnerHyperparamRateLimit :: < T > :: get () as u64 ; (Tempo :: < T > :: get (netuid) as u64) . saturating_mul (epochs)"), "owner hyperparameter rate expression changed");
    Ok(result)
}

fn getter_storage(method: &syn::ImplItemFn) -> Result<String> {
    let [syn::Stmt::Expr(syn::Expr::Call(call), None)] = method.block.stmts.as_slice() else {
        bail!("owner current getter changed");
    };
    let syn::Expr::Path(path) = &*call.func else {
        bail!("current getter is not a path");
    };
    ensure!(
        path.path.segments.len() == 2
            && path.path.segments[1].ident == "get"
            && call.args.len() == 1
            && tokens(&call.args[0]) == "netuid",
        "owner current storage getter changed"
    );
    Ok(path.path.segments[0].ident.to_string())
}

fn complete_policy(
    mut policy: Value,
    codes: &BTreeMap<String, u16>,
    rates: &BTreeMap<String, Value>,
) -> Result<Value> {
    if policy["kind"] == "transaction" {
        let variant = policy["variant"]
            .as_str()
            .context("transaction variant missing")?;
        let code = *codes
            .get(variant)
            .context("owner transaction code missing")?;
        let rate = rates
            .get(variant)
            .context("owner transaction rate missing")?
            .clone();
        policy["code"] = json!(code);
        policy["rate"] = rate;
    }
    Ok(policy)
}

fn validate_value_shape(runtime: &Runtime, type_id: u32, shape: &str) -> Result<()> {
    let definition = &runtime
        .types
        .resolve(type_id)
        .context("owner argument type missing")?
        .type_def;
    match shape {
        "epoch_consensus" => {
            let TypeDef::Variant(variants) = definition else {
                bail!("EpochConsensus argument is not an enum");
            };
            ensure!(
                variants
                    .variants
                    .iter()
                    .map(|variant| (variant.name.as_str(), variant.index, variant.fields.len()))
                    .collect::<Vec<_>>()
                    == [("Yuma", 0, 0), ("Null", 1, 0)],
                "EpochConsensus enum representation changed"
            );
        }
        "fixed_bits" => {
            let TypeDef::Composite(composite) = definition else {
                bail!("fixed-point owner argument is not a struct");
            };
            ensure!(
                composite.fields.len() == 1 && composite.fields[0].name.as_deref() == Some("bits"),
                "fixed-point owner argument fields changed"
            );
            let bits = runtime
                .types
                .resolve(composite.fields[0].ty.id)
                .context("fixed-point bits type missing")?;
            ensure!(
                matches!(
                    bits.type_def,
                    TypeDef::Primitive(scale_info::TypeDefPrimitive::U128)
                ),
                "fixed-point owner bits width changed"
            );
        }
        "bool" => ensure!(
            matches!(
                definition,
                TypeDef::Primitive(scale_info::TypeDefPrimitive::Bool)
            ),
            "owner boolean argument type changed"
        ),
        _ => {}
    }
    Ok(())
}

fn freeze_rules(
    misc: &BTreeMap<String, syn::ImplItemFn>,
    coinbase: &BTreeMap<String, syn::ImplItemFn>,
) -> Result<Value> {
    let mut result = serde_json::Map::new();
    for (name, methods, expected) in [
        (
            "is_in_admin_freeze_window",
            misc,
            r#"{
            let tempo = Self::get_tempo(netuid);
            if tempo == 0 { return false; }
            let pending = PendingEpochAt::<T>::get(netuid);
            if pending > 0 && pending > current_block { return true; }
            let remaining = Self::blocks_until_next_auto_epoch(netuid,tempo,current_block);
            let window = AdminFreezeWindow::<T>::get() as u64;
            remaining < window
        }"#,
        ),
        (
            "ensure_admin_window_open",
            misc,
            r#"{
            let now = Self::get_current_block_as_u64();
            ensure!(!Self::is_in_admin_freeze_window(netuid,now),Error::<T>::AdminActionProhibitedDuringWeightsWindow);
            Ok(())
        }"#,
        ),
        (
            "blocks_until_next_auto_epoch",
            coinbase,
            r#"{
            if tempo == 0 { return u64::MAX; }
            let last = LastEpochBlock::<T>::get(netuid);
            let next_auto = last.saturating_add(tempo as u64);
            next_auto.saturating_sub(block_number)
        }"#,
        ),
        (
            "ensure_sn_owner_or_root_with_limits",
            misc,
            r#"{
            let maybe_who = Self::ensure_subnet_owner_or_root(o,netuid)?;
            if let Some(who) = maybe_who.as_ref() {
                for tx in limits.iter() {
                    ensure!(tx.passes_rate_limit_on_subnet::<T>(who,netuid),Error::<T>::TxRateLimitExceeded);
                }
            }
            Ok(maybe_who)
        }"#,
        ),
        (
            "record_owner_rl",
            misc,
            r#"{
            if let Some(who) = maybe_owner {
                let now = Self::get_current_block_as_u64();
                for tx in txs { tx.set_last_block_on_subnet::<T>(&who,netuid,now); }
            }
        }"#,
        ),
    ] {
        let method = methods
            .get(name)
            .with_context(|| format!("owner framework rule {name} missing"))?;
        let expected: syn::Block = syn::parse_str(expected)?;
        ensure!(
            tokens(&method.block) == tokens(&expected),
            "owner framework rule {name} changed"
        );
        result.insert(
            name.into(),
            json!({"signature":tokens(&method.sig),"body":tokens(&method.block)}),
        );
    }
    Ok(Value::Object(result))
}

fn production_slot_duration(runtime: &Runtime) -> Result<u64> {
    let info = runtime
        .constant("Aura", "SlotDuration")
        .context("pinned Aura.SlotDuration missing")?;
    ensure!(
        matches!(
            runtime
                .types
                .resolve(info.ty)
                .context("slot duration type missing")?
                .type_def,
            TypeDef::Primitive(scale_info::TypeDefPrimitive::U64)
        ),
        "pinned Aura.SlotDuration type is not u64"
    );
    let decoded = runtime.decode_spec(
        &rtao_core::runtime::TypeSpec::Id(info.ty),
        &info.value,
        true,
    )?;
    let millis = rtao_core::value_ext::as_u128(&decoded)
        .and_then(|value| u64::try_from(value).ok())
        .context("pinned Aura.SlotDuration is not u64")?;
    ensure!(millis > 0, "pinned Aura.SlotDuration is zero");
    Ok(millis)
}

fn document(root: &Path, pins: &DocumentMut, tables: &Value) -> Result<Value> {
    let mut resolver = Resolver::new(root)?;
    let owners = dictionary(&tables[PYTHON], "OWNER_HYPERPARAMETERS")?;
    let alpha = dictionary(&tables[PYTHON], "_ALPHA_PAIR")?;
    let storage = dictionary(&tables[UNITS], "STORAGE_ITEMS")?;
    let units = dictionary(&tables[UNITS], "HYPERPARAMS")?;
    let ident_kinds = dictionary(&tables[UNITS], "_IDENT_KINDS")?;
    let admin = methods(&syn::parse_file(&resolver.source(ADMIN)?)?.items)?;
    let tempo = methods(&syn::parse_file(&resolver.source(TEMPO)?)?.items)?;
    let misc = methods(&syn::parse_file(&resolver.source(MISC)?)?.items)?;
    let coinbase = methods(&syn::parse_file(&resolver.source(COINBASE)?)?.items)?;
    let freeze = freeze_rules(&misc, &coinbase)?;
    let rate_source = resolver.source(RATE)?;
    let (codes, variants) = transaction_codes(&rate_source)?;
    let rates = rate_kinds(&rate_source)?;
    crate::metadata_pin::verify(root, pins)?;
    let bytes = fs::read(
        root.join(
            crate::metadata_pin::pinned_metadata(pins)?["v15"]
                .as_str()
                .context("metadata pin missing")?,
        ),
    )?;
    let runtime = Runtime::parse(
        &bytes,
        u32::try_from(
            crate::metadata_pin::pinned_metadata(pins)?["spec"]
                .as_integer()
                .context("metadata spec missing")?,
        )?,
        1,
        42,
    )?;
    let ir = MetadataIr::from_runtime(&runtime)?;
    let slot_duration = production_slot_duration(&runtime)?;
    let metadata_admin = ir
        .pallets
        .iter()
        .find(|p| p.name == "AdminUtils")
        .context("AdminUtils metadata missing")?;
    let mut rows = Vec::new();
    let mut setters = BTreeSet::new();
    for (name, row) in &owners {
        let tuple = row["tuple"]
            .as_array()
            .context("owner allowlist row is not a tuple")?;
        ensure!(tuple.len() == 2, "owner allowlist row shape changed");
        let setter = tuple[0].as_str().context("owner setter missing")?;
        let boolean = tuple[1].as_bool().context("owner boolean shape missing")?;
        setters.insert(setter);
        let method = admin
            .get(setter)
            .with_context(|| format!("owner setter {setter} missing in Rust"))?;
        let params = params(method)?;
        let metadata_call = metadata_admin
            .calls
            .iter()
            .find(|call| call.name == setter)
            .context("owner setter absent from metadata")?;
        ensure!(
            metadata_call
                .args
                .iter()
                .map(|arg| arg.name.as_str())
                .collect::<Vec<_>>()
                == std::iter::once("netuid")
                    .chain(params.iter().map(String::as_str))
                    .collect::<Vec<_>>(),
            "owner setter {setter} argument names differ from metadata"
        );
        let policy = if matches!(setter, "sudo_set_tempo" | "sudo_set_activity_cutoff_factor") {
            let delegate = setter.trim_start_matches("sudo_set_");
            delegated_policy(
                method,
                tempo
                    .get(&format!("do_set_{delegate}"))
                    .context("owner delegate missing")?,
                &variants,
            )?
        } else {
            direct_policy(method, &variants)?
        };
        let policy = complete_policy(policy, &codes, &rates)?;
        let (current, shape) = if let Some(index) = alpha.get(name) {
            ensure!(
                setter == "sudo_set_alpha_values"
                    && params == ["alpha_low", "alpha_high"]
                    && index.as_u64().is_some_and(|index| index < 2),
                "alpha pair contract changed"
            );
            (
                json!({"pallet":"SubtensorModule","name":getter_storage(misc.get("get_alpha_values").context("alpha getter missing")?)?}),
                json!({"kind":"alpha_pair","index":index}),
            )
        } else if name == "epoch_consensus" {
            ensure!(params == ["mode"], "epoch consensus argument changed");
            (
                json!({"pallet":"SubtensorModule","name":getter_storage(misc.get("get_epoch_consensus").context("epoch getter missing")?)?}),
                json!({"kind":"epoch_consensus"}),
            )
        } else {
            ensure!(
                params.len() == 1,
                "ordinary owner setter must have one value argument"
            );
            let current = storage.get(name).context("owner current storage missing")?;
            let ident = current["value_type_ident"]
                .as_str()
                .context("owner storage identity missing")?;
            let kind = ident_kinds
                .get(ident)
                .and_then(Value::as_str)
                .or_else(|| units.get(name).and_then(|u| u["fields"]["kind"].as_str()))
                .context("owner unit kind missing")?;
            ensure!(
                boolean == (kind == "bool"),
                "owner boolean allowlist differs from units"
            );
            (
                json!({"pallet":current["pallet"],"name":current["item"]}),
                json!({"kind":if boolean {"bool"} else if kind == "fixed128" {"fixed_bits"} else {"integer"}}),
            )
        };
        validate_value_shape(
            &runtime,
            metadata_call
                .args
                .last()
                .context("owner value argument missing")?
                .type_id,
            shape["kind"].as_str().context("owner shape missing")?,
        )?;
        resolver.validate_descriptor(
            current["pallet"]
                .as_str()
                .context("current pallet missing")?,
            current["name"]
                .as_str()
                .context("current storage name missing")?,
            false,
        )?;
        rows.push(json!({"name":name,"setter":setter,"params":params,"current":current,"shape":shape,"cooldown":policy,"setter_signature":tokens(&method.sig),"setter_body":tokens(&method.block)}));
    }
    ensure!(
        owners.len() == 34 && setters.len() == 33 && alpha.len() == 2,
        "owner allowlist coverage changed"
    );
    let mut operations = Vec::new();
    for setter in [
        "sudo_set_mechanism_count",
        "sudo_set_mechanism_emission_split",
        "sudo_trim_to_max_allowed_uids",
        "sudo_trim_null_uids_batch",
    ] {
        let method = admin
            .get(setter)
            .context("owner operation setter missing")?;
        let params = params(method)?;
        let metadata_call = metadata_admin
            .calls
            .iter()
            .find(|call| call.name == setter)
            .context("owner operation absent from metadata")?;
        ensure!(
            metadata_call
                .args
                .iter()
                .map(|arg| arg.name.as_str())
                .collect::<Vec<_>>()
                == std::iter::once("netuid")
                    .chain(params.iter().map(String::as_str))
                    .collect::<Vec<_>>(),
            "owner operation arguments differ from metadata"
        );
        let (policy, continuation) = if setter == "sudo_trim_null_uids_batch" {
            let expected: syn::Block = syn::parse_str(
                r#"{
                let continuing = pallet_subtensor::NullPruningTarget::<T>::get(netuid) == Some(target);
                let limits: &[TransactionType] = if continuing { &[] } else { &[TransactionType::MaxUidsTrimming] };
                let maybe_owner = pallet_subtensor::Pallet::<T>::ensure_sn_owner_or_root_with_limits(origin,netuid,limits,)?;
                pallet_subtensor::Pallet::<T>::ensure_admin_window_open(netuid)?;
                pallet_subtensor::Pallet::<T>::trim_null_uids_batch(netuid,target)?;
                if !continuing {
                    pallet_subtensor::Pallet::<T>::record_owner_rl(maybe_owner,netuid,&[TransactionType::MaxUidsTrimming],);
                }
                Ok(())
            }"#,
            )?;
            ensure!(
                tokens(&method.block) == tokens(&expected),
                "Null pruning continuation policy changed"
            );
            let body = calls(method);
            let guard = single_call(&body, "ensure_sn_owner_or_root_with_limits")?;
            ensure!(
                guard.args.len() == 3 && tokens(&guard.args[2]) == "limits",
                "Null pruning guard changed"
            );
            single_call(&body, "ensure_admin_window_open")?;
            let record = single_call(&body, "record_owner_rl")?;
            ensure!(
                record.args.len() == 3,
                "Null pruning record arguments changed"
            );
            let policy = limit(&record.args[2], &variants)?;
            ensure!(
                policy["variant"] == "MaxUidsTrimming",
                "Null pruning record changed"
            );
            resolver.validate_descriptor("SubtensorModule", "NullPruningTarget", false)?;
            (
                policy,
                json!({"pallet":"SubtensorModule","name":"NullPruningTarget"}),
            )
        } else {
            (direct_policy(method, &variants)?, Value::Null)
        };
        operations.push(json!({"setter":setter,"params":params,"cooldown":complete_policy(policy,&codes,&rates)?,"continuation_storage":continuation,"setter_signature":tokens(&method.sig),"setter_body":tokens(&method.block)}));
    }
    let provenance = [PYTHON,UNITS,ADMIN,TEMPO,RATE,MISC,COINBASE].into_iter().map(|path|json!({"path":path,"blob":resolver.tree.get(path),"tag":pins["parity"]["tag"].as_str(),"commit":pins["parity"]["commit"].as_str()})).collect::<Vec<_>>();
    let metadata = crate::metadata_pin::pinned_metadata(pins)?;
    Ok(
        json!({"format":1,"provenance":tables[PYTHON]["provenance"],"sources":provenance,
        "metadata_provenance":{"path":metadata["v15"].as_str(),"sha256":metadata["v15_sha256"].as_str(),"spec":metadata["spec"].as_integer(),"finalized_head":metadata["finalized_head"].as_str()},
        "values":{"hyperparameters":rows,"operations":operations,"framework_rules":freeze,"production_slot_duration_ms":slot_duration}}),
    )
}

fn render_storage(storage: &Value) -> Result<String> {
    Ok(format!(
        "OwnerStorage {{pallet:{},name:{}}}",
        rust_string(
            storage["pallet"]
                .as_str()
                .context("storage pallet missing")?
        ),
        rust_string(storage["name"].as_str().context("storage name missing")?)
    ))
}

fn render_cooldown(policy: &Value) -> Result<String> {
    Ok(
        match policy["kind"].as_str().context("cooldown kind missing")? {
            "none" => "OwnerCooldown::None".into(),
            "owner" => format!(
                "OwnerCooldown::OwnerHyperparameter({})",
                rust_string(
                    policy["variant"]
                        .as_str()
                        .context("Hyperparameter variant missing")?
                )
            ),
            "transaction" => {
                let rate = &policy["rate"];
                let rate = match rate["kind"].as_str().context("rate kind missing")? {
                    "tempo_multiplier" => format!(
                        "OwnerRateKind::TempoMultiplier({})",
                        rust_string(rate["storage"].as_str().context("rate storage missing")?)
                    ),
                    "fixed_constant" => format!(
                        "OwnerRateKind::FixedConstant({})",
                        rust_string(rate["constant"].as_str().context("rate constant missing")?)
                    ),
                    "runtime_default" => {
                        let name = rate["constant"]
                            .as_str()
                            .context("runtime default missing")?;
                        let mut constant = String::new();
                        for (index, character) in name.chars().enumerate() {
                            if index > 0 && character.is_ascii_uppercase() {
                                constant.push('_');
                            }
                            constant.push(character.to_ascii_uppercase());
                        }
                        format!("OwnerRateKind::RuntimeDefault(crate::rt::rate_limits::{constant})")
                    }
                    _ => bail!("unsupported owner rate kind"),
                };
                format!("OwnerCooldown::Transaction({}, {rate})", policy["code"])
            }
            _ => bail!("unsupported owner cooldown"),
        },
    )
}

fn render_params(row: &Value) -> Result<String> {
    Ok(format!(
        "&[{}]",
        row["params"]
            .as_array()
            .context("params missing")?
            .iter()
            .map(|param| param.as_str().map(rust_string).context("param is not text"))
            .collect::<Result<Vec<_>>>()?
            .join(",")
    ))
}

fn render(document: &Value) -> Result<Vec<u8>> {
    let mut out = header(document)?;
    out.push_str(
        r#"
//! Owner-settable hyperparameters and the runtime rules used by owner planning.

/// Storage read used by an owner policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnerStorage {
    /// Metadata pallet name.
    pub pallet: &'static str,
    /// Metadata storage item name.
    pub name: &'static str,
}
/// Representation of the setter's value arguments.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerValueShape {
    /// A single raw integer argument.
    Integer,
    /// A single boolean argument.
    Bool,
    /// Fixed-point newtype encoded as a struct containing `bits`.
    FixedBits,
    /// The `Yuma` or `Null` epoch consensus variant.
    EpochConsensus,
    /// Replace one side of the stored alpha pair, retaining the other.
    AlphaPair {
        /// Position of the parameter in the stored pair.
        index: u8,
    },
}
/// How a transaction cooldown gets its block count.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerRateKind {
    /// Named storage multiplier times the subnet's current tempo.
    TempoMultiplier(&'static str),
    /// Named metadata constant containing a fixed block count.
    FixedConstant(&'static str),
    /// Authenticated production and fast-block runtime defaults.
    RuntimeDefault(crate::rt::rate_limits::RateLimit),
}
/// Runtime storage key and rate rule used by an owner setter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerCooldown {
    /// `LastRateLimitedBlock[OwnerHyperparamUpdate(netuid, variant)]`, with
    /// `OwnerHyperparamRateLimit` multiplied by the current subnet tempo.
    OwnerHyperparameter(&'static str),
    /// `TransactionKeyLastBlock[(owner, netuid, transaction_type_code)]`.
    Transaction(u16, OwnerRateKind),
    /// The setter has no owner cooldown; the admin freeze window still applies.
    None,
}
/// An authenticated allowlisted owner hyperparameter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnerHyperparam {
    /// Stable Python intent parameter name.
    pub name: &'static str,
    /// AdminUtils call name.
    pub setter: &'static str,
    /// Value argument names in call order, excluding `netuid`.
    pub params: &'static [&'static str],
    /// Storage containing the current value, or the entire alpha pair.
    pub current: OwnerStorage,
    /// Setter argument representation.
    pub shape: OwnerValueShape,
    /// Owner cooldown rule.
    pub cooldown: OwnerCooldown,
}
/// Cooldown rule for an owner mechanism or pruning operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnerOperationRule {
    /// AdminUtils call name.
    pub setter: &'static str,
    /// Value argument names in call order, excluding `netuid`.
    pub params: &'static [&'static str],
    /// Owner cooldown rule for a new operation.
    pub cooldown: OwnerCooldown,
    /// A stored target matching the supplied target bypasses only the cooldown.
    pub continuation_storage: Option<OwnerStorage>,
}
/// All 34 allowlisted names over 33 owner setters, checked against runtime source.
pub const OWNER_HYPERPARAMETERS: &[OwnerHyperparam] = &[
"#,
    );
    for row in document["values"]["hyperparameters"]
        .as_array()
        .context("owner rows missing")?
    {
        let shape = match row["shape"]["kind"]
            .as_str()
            .context("value shape missing")?
        {
            "integer" => "OwnerValueShape::Integer".into(),
            "bool" => "OwnerValueShape::Bool".into(),
            "fixed_bits" => "OwnerValueShape::FixedBits".into(),
            "epoch_consensus" => "OwnerValueShape::EpochConsensus".into(),
            "alpha_pair" => format!(
                "OwnerValueShape::AlphaPair {{index:{}}}",
                row["shape"]["index"]
            ),
            _ => bail!("unsupported owner value shape"),
        };
        out.push_str(&format!("OwnerHyperparam {{name:{},setter:{},params:{},current:{},shape:{shape},cooldown:{}}},\n",rust_string(row["name"].as_str().context("owner name missing")?),rust_string(row["setter"].as_str().context("owner setter missing")?),render_params(row)?,render_storage(&row["current"])?,render_cooldown(&row["cooldown"])?));
    }
    out.push_str("];\n/// Authenticated mechanism and pruning setter rules.\npub const OWNER_OPERATION_RULES: &[OwnerOperationRule] = &[\n");
    for row in document["values"]["operations"]
        .as_array()
        .context("operation rows missing")?
    {
        let continuation = if row["continuation_storage"].is_null() {
            "None".into()
        } else {
            format!("Some({})", render_storage(&row["continuation_storage"])?)
        };
        out.push_str(&format!("OwnerOperationRule {{setter:{},params:{},cooldown:{},continuation_storage:{continuation}}},\n",rust_string(row["setter"].as_str().context("operation setter missing")?),render_params(row)?,render_cooldown(&row["cooldown"])?));
    }
    out.push_str("];\n");
    out.push_str(&format!("/// Production slot duration decoded from the authenticated pinned Aura metadata.\npub const PRODUCTION_SLOT_DURATION_MS: u64 = {};\n",document["values"]["production_slot_duration_ms"]));
    rustfmt(out.into_bytes())
}

pub(super) fn outputs(
    root: &Path,
    pins: &DocumentMut,
    tables: &Value,
) -> Result<Vec<(&'static str, Vec<u8>)>> {
    let document = document(root, pins, tables)?;
    Ok(vec![
        ("crates/rtao-catalog/src/owner_map.rs", render(&document)?),
        (
            "vendor/upstream/extracted/owner_map.json",
            pretty(&document)?,
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Result<(Resolver<'static>, Value)> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root missing")?;
        let tables: Value = serde_json::from_slice(&fs::read(
            root.join("vendor/upstream/extracted/named_tables.json"),
        )?)?;
        Ok((Resolver::new(root)?, tables["tables"].clone()))
    }

    #[test]
    fn exact_runtime_variants_and_recording_cover_every_owner_setter() -> Result<()> {
        let (resolver, tables) = fixture()?;
        let owners = dictionary(&tables[PYTHON], "OWNER_HYPERPARAMETERS")?;
        let source = resolver.source(ADMIN)?;
        let admin = methods(&syn::parse_file(&source)?.items)?;
        let tempo = methods(&syn::parse_file(&resolver.source(TEMPO)?)?.items)?;
        let (codes, variants) = transaction_codes(&resolver.source(RATE)?)?;
        let rates = rate_kinds(&resolver.source(RATE)?)?;
        let mut policies = BTreeMap::new();
        for (name, row) in &owners {
            let setter = row["tuple"][0].as_str().context("setter missing")?;
            let method = &admin[setter];
            let policy = if matches!(setter, "sudo_set_tempo" | "sudo_set_activity_cutoff_factor") {
                delegated_policy(
                    method,
                    &tempo[&format!("do_set_{}", setter.trim_start_matches("sudo_set_"))],
                    &variants,
                )?
            } else {
                direct_policy(method, &variants)?
            };
            policies.insert(name.clone(), complete_policy(policy, &codes, &rates)?);
        }
        assert_eq!(policies.len(), 34);
        for (name, variant) in [
            ("commit_reveal_period", "WeightCommitInterval"),
            ("commit_reveal_weights_enabled", "CommitRevealEnabled"),
            ("registration_allowed", "BurnRegistrationAllowed"),
            ("network_pow_registration_allowed", "PowRegistrationAllowed"),
            ("transfers_enabled", "TransferEnabled"),
            ("owner_immune_neuron_limit", "ImmuneNeuronLimit"),
            ("activity_cutoff_factor", "ActivityCutoffFactorMilli"),
            ("alpha_low", "AlphaValues"),
            ("alpha_high", "AlphaValues"),
        ] {
            assert_eq!(policies[name]["variant"], variant, "{name}");
        }
        assert_eq!(policies["tempo"]["code"], 11);
        assert_eq!(policies["weights_version"]["code"], 4);
        assert_eq!(policies["owner_cut_enabled"]["kind"], "none");
        assert_eq!(policies["owner_cut_auto_lock_enabled"]["kind"], "none");
        let mut changed = admin["sudo_set_commit_reveal_weights_interval"].clone();
        changed.block = syn::parse_str(&tokens(&changed.block).replacen(
            "Hyperparameter :: WeightCommitInterval",
            "Hyperparameter :: CommitRevealEnabled",
            1,
        ))?;
        assert!(direct_policy(&changed, &variants).is_err());
        let mut changed = admin["sudo_set_owner_cut_enabled"].clone();
        changed.block = syn::parse_str(
            &tokens(&changed.block).replace("ensure_admin_window_open", "changed_window"),
        )?;
        assert!(direct_policy(&changed, &variants).is_err());
        Ok(())
    }

    #[test]
    fn transaction_codes_and_rates_are_extracted_from_source() -> Result<()> {
        let (resolver, _) = fixture()?;
        let source = resolver.source(RATE)?;
        let (codes, _) = transaction_codes(&source)?;
        assert_eq!(codes["MechanismCountUpdate"], 7);
        assert_eq!(codes["MechanismEmission"], 8);
        assert_eq!(codes["MaxUidsTrimming"], 9);
        let rates = rate_kinds(&source)?;
        assert_eq!(
            rates["MechanismCountUpdate"]["constant"],
            "MechanismCountSetRateLimit"
        );
        assert_eq!(
            rates["MechanismEmission"]["constant"],
            "MechanismEmissionRateLimit"
        );
        assert_eq!(
            rates["MaxUidsTrimming"]["constant"],
            "MaxUidsTrimmingRateLimit"
        );
        assert_eq!(rates["TempoUpdate"]["constant"], "MinTempo");
        assert_eq!(
            rates["SetWeightsVersionKey"]["storage"],
            "WeightsVersionKeyRateLimit"
        );
        assert!(
            rate_kinds(&source.replace(
                "saturating_mul(WeightsVersionKeyRateLimit",
                "saturating_add(WeightsVersionKeyRateLimit"
            ))
            .is_err()
        );
        assert!(
            rate_kinds(&source.replace(
                "Self::TempoUpdate => MIN_TEMPO as u64",
                "Self::TempoUpdate => 1"
            ))
            .is_err()
        );
        assert!(
            transaction_codes(&source.replace(
                "TransactionType::TempoUpdate => 11",
                "TransactionType::TempoUpdate => 9"
            ))
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn complete_owner_document_authenticates_special_shapes_and_helper_rules() -> Result<()> {
        let (resolver, tables) = fixture()?;
        let pins =
            fs::read_to_string(resolver.root.join("upstream.toml"))?.parse::<DocumentMut>()?;
        let document = document(resolver.root, &pins, &tables)?;
        assert_eq!(document["values"]["production_slot_duration_ms"], 12000);
        assert_eq!(
            document["metadata_provenance"]["sha256"],
            crate::metadata_pin::pinned_metadata(&pins)?["v15_sha256"]
                .as_str()
                .context("metadata hash missing")?
        );
        let rows = document["values"]["hyperparameters"]
            .as_array()
            .context("rows missing")?;
        let row = |name: &str| -> Result<&Value> {
            rows.iter()
                .find(|row| row["name"] == name)
                .context("owner row missing")
        };
        assert_eq!(
            row("epoch_consensus")?["current"]["name"],
            "SubnetEpochConsensus"
        );
        assert_eq!(row("epoch_consensus")?["shape"]["kind"], "epoch_consensus");
        assert_eq!(row("alpha_low")?["shape"]["index"], 0);
        assert_eq!(row("alpha_high")?["shape"]["index"], 1);
        assert_eq!(
            row("alpha_high")?["params"],
            json!(["alpha_low", "alpha_high"])
        );
        assert_eq!(
            row("collateral_drain_ratio")?["params"],
            json!(["drain_ratio"])
        );
        assert_eq!(
            row("collateral_drain_ratio")?["shape"]["kind"],
            "fixed_bits"
        );
        let operations = document["values"]["operations"]
            .as_array()
            .context("operations missing")?;
        assert_eq!(operations.len(), 4);
        assert_eq!(
            operations
                .iter()
                .map(|row| row["cooldown"]["code"].as_u64().context("code missing"))
                .collect::<Result<Vec<_>>>()?,
            [7, 8, 9, 9]
        );
        assert!(operations[2]["continuation_storage"].is_null());
        assert_eq!(
            operations[3]["continuation_storage"]["name"],
            "NullPruningTarget"
        );
        let mut changed = tables.clone();
        let pair = changed[PYTHON]["values"]["_ALPHA_PAIR"]["value"]["dict"]
            .as_array_mut()
            .context("alpha dictionary missing")?;
        pair[0][1] = json!(2);
        assert!(super::document(resolver.root, &pins, &changed).is_err());
        Ok(())
    }

    #[test]
    fn authenticated_freeze_rules_reject_boundary_and_scheduler_drift() -> Result<()> {
        let (resolver, _) = fixture()?;
        let misc_source = resolver.source(MISC)?;
        let coinbase_source = resolver.source(COINBASE)?;
        let misc = methods(&syn::parse_file(&misc_source)?.items)?;
        let coinbase = methods(&syn::parse_file(&coinbase_source)?.items)?;
        freeze_rules(&misc, &coinbase)?;
        for changed in [
            misc_source.replace("remaining < window", "remaining <= window"),
            misc_source.replace("pending > current_block", "pending >= current_block"),
        ] {
            assert!(freeze_rules(&methods(&syn::parse_file(&changed)?.items)?, &coinbase).is_err());
        }
        let changed = coinbase_source.replace(
            "next_auto.saturating_sub(block_number)",
            "next_auto.wrapping_sub(block_number)",
        );
        assert!(freeze_rules(&misc, &methods(&syn::parse_file(&changed)?.items)?).is_err());
        Ok(())
    }
}
