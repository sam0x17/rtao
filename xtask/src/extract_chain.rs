//! Authenticated runtime tables and the chain rules mirrored by the framework.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};
use quote::ToTokens;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use syn::{
    Token,
    parse::{Parse, ParseStream, Parser},
    punctuated::Punctuated,
};
use toml_edit::DocumentMut;

use super::{Resolver, header, pretty, rust_string, rustfmt};

#[path = "extract_fee_contract.rs"]
mod fee_contract;
#[path = "extract_proxy_contract.rs"]
mod proxy_contract;
#[path = "extract_reveal_contract.rs"]
mod reveal_contract;

const TRANSACTION_ERRORS: &str = "common/src/transaction_error.rs";
const FEE_FILTERS: &str = "runtime/src/fee_filters.rs";
const MORTALITY: &str = "runtime/src/check_mortality.rs";
const PALLET: &str = "pallets/subtensor/src/lib.rs";
const RATE_LIMITING: &str = "pallets/subtensor/src/utils/rate_limiting.rs";
const ERROR_MAP: &str = "sdk/python/bittensor/error_map.py";
const PY_FEE_FILTERS: &str = "sdk/python/bittensor/fee_filters.py";

fn pallet_source(root: &Path, pins: &DocumentMut) -> Result<String> {
    // lib.rs is an intentional EXTRA_INPUTS entry in import-upstream: its
    // constants are copied and hash-pinned, while it is excluded from tree.txt.
    let artifact = &pins["artifacts"][PALLET];
    let destination = format!("vendor/upstream/src/{PALLET}");
    ensure!(
        artifact["to"].as_str() == Some(destination.as_str()),
        "pallet extraction input destination differs from the artifact pin"
    );
    let expected = artifact["sha256"]
        .as_str()
        .context("pallet extraction artifact hash missing")?;
    let bytes = fs::read(root.join(destination))?;
    ensure!(
        bytes.len() <= super::MAX_SOURCE_BYTES,
        "pallet source exceeds extraction byte limit"
    );
    ensure!(
        format!("{:x}", Sha256::digest(&bytes)) == expected,
        "pallet extraction input differs from its artifact SHA256 pin"
    );
    String::from_utf8(bytes).context("pallet extraction input is not UTF-8")
}

fn tokens(value: &impl ToTokens) -> String {
    value.to_token_stream().to_string()
}

fn path_is(path: &syn::Path, expected: &[&str]) -> bool {
    path.leading_colon.is_none()
        && path.segments.len() == expected.len()
        && path.segments.iter().zip(expected).all(|(part, expected)| {
            part.ident == *expected && matches!(part.arguments, syn::PathArguments::None)
        })
}

fn type_is(ty: &syn::Type, expected: &str) -> bool {
    matches!(ty, syn::Type::Path(ty) if ty.qself.is_none() && path_is(&ty.path, &[expected]))
}

fn expression_path_is(expr: &syn::Expr, expected: &[&str]) -> bool {
    matches!(expr, syn::Expr::Path(expr) if expr.qself.is_none() && path_is(&expr.path, expected))
}

fn document(resolver: &Resolver<'_>, pins: &DocumentMut, path: &str, values: Value) -> Value {
    json!({"format":1,"provenance":{"path":path,"blob":resolver.tree.get(path),"tag":pins["parity"]["tag"].as_str(),"commit":pins["parity"]["commit"].as_str()},"values":values})
}

fn tail(block: &syn::Block) -> Result<&syn::Expr> {
    let [syn::Stmt::Expr(expr, None)] = block.stmts.as_slice() else {
        bail!("expected a single tail expression");
    };
    Ok(expr)
}

fn number(expr: &syn::Expr, depth: usize, ty: &str) -> Result<u64> {
    ensure!(
        depth < 32,
        "runtime constant exceeds expression depth limit"
    );
    let maximum = match ty {
        "u8" => u64::from(u8::MAX),
        "u16" => u64::from(u16::MAX),
        "u32" => u64::from(u32::MAX),
        "u64" => u64::MAX,
        _ => bail!("unsupported runtime integer type {ty}"),
    };
    let result = match expr {
        syn::Expr::Lit(syn::ExprLit {
            attrs,
            lit: syn::Lit::Int(value),
        }) if attrs.is_empty() => {
            ensure!(
                value.suffix().is_empty() || value.suffix() == ty,
                "runtime integer suffix differs from {ty} context"
            );
            Ok(value.base10_parse()?)
        }
        syn::Expr::Paren(expr) if expr.attrs.is_empty() => number(&expr.expr, depth + 1, ty),
        syn::Expr::Binary(expr) if expr.attrs.is_empty() => {
            let left = number(&expr.left, depth + 1, ty)?;
            let right = number(&expr.right, depth + 1, ty)?;
            match expr.op {
                syn::BinOp::Add(_) => left.checked_add(right),
                syn::BinOp::Mul(_) => left.checked_mul(right),
                _ => bail!("unsupported runtime constant operator"),
            }
            .context("runtime integer constant overflows u64")
        }
        _ => bail!("runtime constant is not bounded literal arithmetic"),
    }?;
    ensure!(result <= maximum, "runtime integer expression exceeds {ty}");
    Ok(result)
}

fn from_implementation<'a>(
    file: &'a syn::File,
    source: &str,
    target: &str,
) -> Result<&'a syn::ImplItemFn> {
    let mut matches = file.items.iter().filter_map(|item| {
        let syn::Item::Impl(item) = item else { return None; };
        let (_, trait_path, _) = item.trait_.as_ref()?;
        let segment = trait_path.segments.first()?;
        let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else { return None; };
        if trait_path.segments.len() != 1 || segment.ident != "From" || arguments.args.len() != 1 || !type_is(&item.self_ty, target) || !matches!(arguments.args.first(), Some(syn::GenericArgument::Type(ty)) if type_is(ty, source)) {
            return None;
        }
        Some(item)
    });
    let implementation = matches.next().context("runtime conversion impl missing")?;
    ensure!(
        matches.next().is_none()
            && implementation
                .attrs
                .iter()
                .all(|attr| attr.path().is_ident("doc"))
            && implementation.generics.params.is_empty()
            && implementation.generics.where_clause.is_none(),
        "runtime conversion must be a single unconditional impl"
    );
    let [syn::ImplItem::Fn(method)] = implementation.items.as_slice() else {
        bail!("runtime conversion must contain only from");
    };
    ensure!(
        method.sig.ident == "from" && method.attrs.iter().all(|attr| attr.path().is_ident("doc")),
        "runtime conversion method changed"
    );
    Ok(method)
}

fn custom_codes(source: &str) -> Result<BTreeMap<u8, String>> {
    let file = syn::parse_file(source)?;
    let mut enums = file.items.iter().filter_map(|item| match item {
        syn::Item::Enum(item) if item.ident == "CustomTransactionError" => Some(item),
        _ => None,
    });
    let enumeration = enums
        .next()
        .context("CustomTransactionError enum missing")?;
    ensure!(
        enums.next().is_none()
            && enumeration
                .attrs
                .iter()
                .all(|attr| attr.path().is_ident("derive")),
        "custom error enum is conditional or duplicated"
    );
    let mut names = BTreeSet::new();
    for variant in &enumeration.variants {
        ensure!(
            matches!(variant.fields, syn::Fields::Unit)
                && variant.discriminant.is_none()
                && variant
                    .attrs
                    .iter()
                    .all(|attr| attr.path().is_ident("doc") || attr.path().is_ident("deprecated")),
            "custom error variants must be unconditional unit variants"
        );
        ensure!(
            names.insert(variant.ident.to_string()),
            "duplicate custom error variant"
        );
    }
    let method = from_implementation(&file, "CustomTransactionError", "u8")?;
    ensure!(
        tokens(&method.sig) == "fn from (variant : CustomTransactionError) -> u8",
        "custom error conversion signature changed"
    );
    let syn::Expr::Match(mapping) = tail(&method.block)? else {
        bail!("custom error conversion must be a match");
    };
    ensure!(
        mapping.attrs.is_empty() && expression_path_is(&mapping.expr, &["variant"]),
        "custom error conversion match changed"
    );
    let mut mapped = BTreeSet::new();
    let mut codes = BTreeMap::new();
    for arm in &mapping.arms {
        ensure!(
            arm.guard.is_none()
                && arm
                    .attrs
                    .iter()
                    .all(|attr| tokens(attr) == "# [allow (deprecated)]"),
            "custom error conversion has guarded or conditional arms"
        );
        let syn::Pat::Path(pattern) = &arm.pat else {
            bail!("custom error conversion pattern is not a variant");
        };
        ensure!(
            pattern.qself.is_none()
                && pattern.path.segments.len() == 2
                && pattern.path.segments[0].ident == "CustomTransactionError"
                && pattern
                    .path
                    .segments
                    .iter()
                    .all(|part| matches!(part.arguments, syn::PathArguments::None)),
            "custom error conversion variant path changed"
        );
        let name = pattern.path.segments[1].ident.to_string();
        ensure!(
            mapped.insert(name.clone()),
            "custom error variant is mapped twice"
        );
        let code = u8::try_from(number(&arm.body, 0, "u8")?)
            .context("custom transaction code exceeds u8")?;
        ensure!(
            codes.insert(code, name).is_none(),
            "duplicate custom transaction code {code}"
        );
    }
    ensure!(
        mapped == names,
        "custom error conversion must cover every enum variant exactly once"
    );
    Ok(codes)
}

fn extracted<'a>(tables: &'a Value, path: &str, name: &str) -> Result<&'a Value> {
    tables
        .get(path)
        .and_then(|table| table.get("values"))
        .and_then(|values| values.get(name))
        .and_then(|value| value.get("value"))
        .with_context(|| format!("missing extracted {path}::{name}"))
}

fn custom_error_rows(codes: &BTreeMap<u8, String>, python: &Value) -> Result<Value> {
    // The Python table deliberately projects private runtime variants onto public
    // dispatch/semantic names. These six aliases are the checked parity rule;
    // numeric codes always come from the Rust From implementation.
    let aliases = BTreeMap::from([
        ("ColdkeyInSwapSchedule", "ColdkeySwapAnnounced"),
        ("BalanceTooLow", "NotEnoughBalanceToStake"),
        ("HotkeyAccountDoesntExist", "HotKeyAccountNotExists"),
        ("CommitNotFound", "NoWeightsCommitFound"),
        ("CommitBlockNotInRevealRange", "RevealTooEarly"),
        ("UidNotFound", "HotKeyNotRegisteredInSubNet"),
    ]);
    let entries = python["dict"]
        .as_array()
        .context("Python custom transaction table is not a dictionary")?;
    let mut python_codes = BTreeMap::new();
    for entry in entries {
        let pair = entry
            .as_array()
            .context("Python custom error entry is not a pair")?;
        ensure!(pair.len() == 2, "Python custom error entry is not a pair");
        let code = u8::try_from(
            pair[0]
                .as_u64()
                .context("Python custom code is not unsigned")?,
        )?;
        let name = pair[1]
            .as_str()
            .context("Python custom error name is not text")?;
        ensure!(
            python_codes.insert(code, name).is_none(),
            "duplicate Python custom error code"
        );
    }
    ensure!(
        python_codes.len() == codes.len(),
        "Rust/Python custom error code counts differ"
    );
    let mut rows = Vec::new();
    for (code, variant) in codes {
        let expected = aliases.get(variant.as_str()).copied().unwrap_or(variant);
        ensure!(
            python_codes.get(code).copied() == Some(expected),
            "Rust custom transaction {variant}={code} differs from the Python projection {expected}"
        );
        rows.push(json!({"code":code,"runtime_variant":variant,"python_name":expected,"alias":expected!=variant}));
    }
    ensure!(
        aliases
            .keys()
            .all(|name| codes.values().any(|variant| variant == name)),
        "custom error alias rule refers to a removed runtime variant"
    );
    Ok(Value::Array(rows))
}

struct FeeGroup {
    name: syn::Ident,
    calls: syn::ExprArray,
}

impl Parse for FeeGroup {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let name = input.parse()?;
        input.parse::<Token![,]>()?;
        let calls = input.parse()?;
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        }
        ensure_empty(input)?;
        Ok(Self { name, calls })
    }
}

fn ensure_empty(input: ParseStream<'_>) -> syn::Result<()> {
    if input.is_empty() {
        Ok(())
    } else {
        Err(input.error("unexpected trailing macro tokens"))
    }
}

fn fee_calls(source: &str) -> Result<BTreeSet<(String, String)>> {
    let file = syn::parse_file(source)?;
    ensure!(file.items.iter().filter(|item| matches!(item, syn::Item::Use(item) if tokens(&item.tree) == "subtensor_macros :: call_filter_group")).count() == 1, "call_filter_group must be imported from subtensor_macros");
    let mut groups = file.items.iter().filter_map(|item| match item {
        syn::Item::Macro(item) if item.mac.path.is_ident("call_filter_group") => Some(item),
        _ => None,
    });
    let group = groups.next().context("fee call_filter_group missing")?;
    ensure!(
        groups.next().is_none() && group.attrs.is_empty(),
        "fee filter must be a single unconditional macro"
    );
    let parsed = syn::parse2::<FeeGroup>(group.mac.tokens.clone())?;
    ensure!(
        parsed.name == "ColdkeyPaysFeeCalls" && parsed.calls.attrs.is_empty(),
        "fee filter group changed"
    );
    let mut calls = BTreeSet::new();
    for call in &parsed.calls.elems {
        let syn::Expr::Call(call) = call else {
            bail!("fee filter must name a RuntimeCall variant");
        };
        let syn::Expr::Path(outer) = &*call.func else {
            bail!("fee filter RuntimeCall constructor must be a path");
        };
        ensure!(
            outer.qself.is_none()
                && outer.path.segments.len() == 2
                && outer.path.segments[0].ident == "RuntimeCall"
                && outer
                    .path
                    .segments
                    .iter()
                    .all(|part| matches!(part.arguments, syn::PathArguments::None))
                && call.attrs.is_empty()
                && call.args.len() == 1,
            "fee filter RuntimeCall shape changed"
        );
        let syn::Expr::Path(inner) = &call.args[0] else {
            bail!("fee filter must name a pallet Call variant");
        };
        ensure!(
            inner.qself.is_none()
                && inner.path.segments.len() == 2
                && inner
                    .path
                    .segments
                    .iter()
                    .all(|part| matches!(part.arguments, syn::PathArguments::None)),
            "fee filter pallet Call shape changed"
        );
        let pallet = outer.path.segments[1].ident.to_string();
        let alias = inner.path.segments[0].ident.to_string();
        let expected_import = match (pallet.as_str(), alias.as_str()) {
            ("SubtensorModule", "SubtensorCall") => "pallet_subtensor :: Call as SubtensorCall",
            ("Commitments", "CommitmentsCall") => "pallet_commitments :: Call as CommitmentsCall",
            _ => bail!("unsupported fee filter pallet/Call alias {pallet}/{alias}"),
        };
        ensure!(file.items.iter().filter(|item| matches!(item, syn::Item::Use(item) if tokens(&item.tree) == expected_import)).count() == 1, "fee filter alias source changed");
        let function = inner.path.segments[1].ident.to_string();
        ensure!(
            calls.insert((pallet, function)),
            "duplicate runtime fee filter call"
        );
    }
    let mut implementations = file.items.iter().filter_map(|item| match item {
        syn::Item::Impl(item)
            if item.trait_.as_ref().is_some_and(|(_, path, _)| {
                path.segments
                    .last()
                    .is_some_and(|part| part.ident == "ColdkeyFeeCallFilter")
            }) =>
        {
            Some(item)
        }
        _ => None,
    });
    let implementation = implementations
        .next()
        .context("coldkey fee filter implementation missing")?;
    ensure!(
        implementations.next().is_none()
            && implementation.attrs.is_empty()
            && type_is(&implementation.self_ty, "Runtime"),
        "coldkey fee filter impl changed"
    );
    let [syn::ImplItem::Fn(method)] = implementation.items.as_slice() else {
        bail!("coldkey fee filter must contain only charges_coldkey");
    };
    ensure!(
        tokens(&method.sig) == "fn charges_coldkey (call : & RuntimeCall) -> bool"
            && method.attrs.is_empty()
            && tokens(tail(&method.block)?) == "ColdkeyPaysFeeCalls :: contains (call)",
        "coldkey fee routing no longer uses the extracted filter"
    );
    Ok(calls)
}

fn check_fee_mirror(calls: &BTreeSet<(String, String)>, python: &Value) -> Result<()> {
    let entries = python["frozenset"]
        .as_array()
        .context("Python fee filter is not a frozenset")?;
    let mut mirror = BTreeSet::new();
    for entry in entries {
        let pair = entry["tuple"]
            .as_array()
            .context("Python fee filter entry is not a tuple")?;
        ensure!(pair.len() == 2, "Python fee filter entry must be a pair");
        ensure!(
            mirror.insert((
                pair[0]
                    .as_str()
                    .context("Python fee pallet is not text")?
                    .to_owned(),
                pair[1]
                    .as_str()
                    .context("Python fee call is not text")?
                    .to_owned()
            )),
            "duplicate Python fee filter call"
        );
    }
    ensure!(
        calls == &mirror,
        "runtime fee filter differs from the Python mirror"
    );
    Ok(())
}

fn numeric_constant(items: &[syn::Item], name: &str, ty: &str) -> Result<Value> {
    let mut matching = items.iter().filter_map(|item| match item {
        syn::Item::Const(item) if item.ident == name => Some(item),
        _ => None,
    });
    let item = matching
        .next()
        .with_context(|| format!("runtime constant {name} missing"))?;
    ensure!(
        matching.next().is_none()
            && type_is(&item.ty, ty)
            && item.attrs.iter().all(|attr| attr.path().is_ident("doc")),
        "runtime constant {name} changed type or became conditional"
    );
    let value = number(&item.expr, 0, ty)?;
    ensure!(
        match ty {
            "u16" => value <= u64::from(u16::MAX),
            "u32" => value <= u64::from(u32::MAX),
            "u64" => true,
            _ => false,
        },
        "runtime constant {name} exceeds declared width"
    );
    Ok(json!({"type":ty,"value":value,"expression":tokens(&item.expr)}))
}

fn pallet_items(file: &syn::File) -> Result<&[syn::Item]> {
    let mut modules = file.items.iter().filter_map(|item| match item {
        syn::Item::Mod(item) if item.ident == "pallet" => Some(item),
        _ => None,
    });
    let module = modules.next().context("subtensor pallet module missing")?;
    ensure!(
        modules.next().is_none()
            && module.attrs.iter().all(|attr| {
                path_is(attr.path(), &["frame_support", "pallet"])
                    || attr.path().is_ident("import_section")
                    || attr.path().is_ident("allow")
                    || attr.path().is_ident("deny")
            }),
        "subtensor pallet module became conditional"
    );
    Ok(&module
        .content
        .as_ref()
        .context("subtensor pallet module is not inline")?
        .1)
}

fn type_value(function: &syn::ItemFn, ty: &str) -> Result<()> {
    ensure!(
        matches!(&function.sig.output, syn::ReturnType::Type(_, output) if type_is(output, ty))
            && function.sig.inputs.is_empty()
            && function.sig.constness.is_none()
            && function.sig.asyncness.is_none()
            && function.sig.unsafety.is_none()
            && function.sig.abi.is_none()
            && function.sig.variadic.is_none()
            && function.attrs.iter().all(|attr| attr.path().is_ident("doc")
                || path_is(attr.path(), &["pallet", "type_value"]))
            && function
                .attrs
                .iter()
                .filter(|attr| path_is(attr.path(), &["pallet", "type_value"]))
                .count()
                == 1,
        "{}: runtime type_value signature/attributes changed",
        function.sig.ident
    );
    Ok(())
}

fn rate_pairs(items: &[syn::Item]) -> Result<Value> {
    let required = BTreeSet::from([
        "MaxUidsTrimmingRateLimit",
        "MechanismCountSetRateLimit",
        "MechanismEmissionRateLimit",
    ]);
    let mut pairs = serde_json::Map::new();
    for item in items {
        let syn::Item::Fn(function) = item else {
            continue;
        };
        let is_required = required.contains(function.sig.ident.to_string().as_str());
        let expression = tail(&function.block);
        let is_macro = matches!(&expression, Ok(syn::Expr::Macro(expr)) if expr.mac.path.is_ident("prod_or_fast"));
        if !is_required && !is_macro {
            continue;
        }
        type_value(function, "u64")?;
        let syn::Expr::Macro(expression) = expression? else {
            bail!("{}: rate limit must use prod_or_fast!", function.sig.ident);
        };
        ensure!(
            expression.mac.path.is_ident("prod_or_fast") && expression.attrs.is_empty(),
            "rate limit macro changed"
        );
        let arguments = Punctuated::<syn::Expr, Token![,]>::parse_terminated
            .parse2(expression.mac.tokens.clone())?;
        ensure!(
            arguments.len() == 2,
            "prod_or_fast! must have two arguments"
        );
        let name = function.sig.ident.to_string();
        ensure!(pairs.insert(name, json!({"type":"u64","production":number(&arguments[0],0,"u64")?,"fast":number(&arguments[1],0,"u64")?,"production_expression":tokens(&arguments[0]),"fast_expression":tokens(&arguments[1]),"signature":tokens(&function.sig)})).is_none(), "duplicate rate-limit type_value");
    }
    ensure!(
        required.iter().all(|name| pairs.contains_key(*name)),
        "required runtime rate-limit pair missing"
    );
    Ok(Value::Object(pairs))
}

fn chain_rules(items: &[syn::Item], rate_source: &str) -> Result<Value> {
    let mut constants = serde_json::Map::new();
    for (name, ty) in [
        ("MIN_TEMPO", "u16"),
        ("MAX_TEMPO", "u16"),
        ("MIN_ACTIVITY_CUTOFF_FACTOR_MILLI", "u32"),
        ("MAX_ACTIVITY_CUTOFF_FACTOR_MILLI", "u32"),
    ] {
        constants.insert(name.into(), numeric_constant(items, name, ty)?);
    }
    let mut functions = items.iter().filter_map(|item| match item {
        syn::Item::Fn(function) if function.sig.ident == "DefaultAdminFreezeWindow" => {
            Some(function)
        }
        _ => None,
    });
    let freeze = functions
        .next()
        .context("DefaultAdminFreezeWindow missing")?;
    ensure!(
        functions.next().is_none(),
        "duplicate freeze window type_value"
    );
    type_value(freeze, "u16")?;
    let freeze_value = number(tail(&freeze.block)?, 0, "u16")?;
    ensure!(
        freeze_value <= u64::from(u16::MAX),
        "freeze window exceeds u16"
    );
    constants.insert("DefaultAdminFreezeWindow".into(), json!({"type":"u16","value":freeze_value,"expression":tokens(tail(&freeze.block)?),"signature":tokens(&freeze.sig)}));
    let file = syn::parse_file(rate_source)?;
    let mut methods = serde_json::Map::new();
    for item in &file.items {
        match item {
            syn::Item::Enum(item) if item.ident == "TransactionType" => {
                ensure!(
                    item.attrs.iter().all(|attr| {
                        attr.path().is_ident("doc")
                            || attr.path().is_ident("derive")
                            || attr.path().is_ident("non_exhaustive")
                    }),
                    "TransactionType enum became conditional"
                );
                let variants = item.variants.iter().map(|variant| {
                    ensure!(variant.attrs.iter().all(|attr| attr.path().is_ident("doc")), "TransactionType variant became conditional");
                    Ok(json!({"name":variant.ident.to_string(),"fields":tokens(&variant.fields),"discriminant":variant.discriminant.as_ref().map(|(_,expr)|tokens(expr))}))
                }).collect::<Result<Vec<_>>>()?;
                ensure!(
                    methods
                        .insert("TransactionType".into(), json!({"variants":variants}))
                        .is_none(),
                    "duplicate TransactionType"
                );
            }
            syn::Item::Impl(item)
                if type_is(&item.self_ty, "TransactionType") && item.trait_.is_none() =>
            {
                ensure!(
                    item.attrs.is_empty(),
                    "TransactionType impl became conditional"
                );
                for member in &item.items {
                    let syn::ImplItem::Fn(function) = member else {
                        continue;
                    };
                    let name = function.sig.ident.to_string();
                    if matches!(
                        name.as_str(),
                        "rate_limit"
                            | "rate_limit_on_subnet"
                            | "check_passes_rate_limit"
                            | "last_block"
                            | "last_block_on_subnet"
                            | "set_last_block_on_subnet"
                    ) {
                        ensure!(
                            function
                                .attrs
                                .iter()
                                .all(|attr| attr.path().is_ident("doc")),
                            "TransactionType::{name} became conditional"
                        );
                        ensure!(methods.insert(name, json!({"signature":tokens(&function.sig),"body":tokens(&function.block)})).is_none(), "duplicate TransactionType method");
                    }
                }
            }
            _ => {}
        }
    }
    ensure!(
        [
            "TransactionType",
            "rate_limit",
            "rate_limit_on_subnet",
            "check_passes_rate_limit",
            "last_block",
            "last_block_on_subnet",
            "set_last_block_on_subnet"
        ]
        .iter()
        .all(|name| methods.contains_key(*name)),
        "TransactionType rule missing"
    );
    let conversion = from_implementation(&file, "TransactionType", "u16")?;
    methods.insert(
        "transaction_type_to_u16".into(),
        json!({"signature":tokens(&conversion.sig),"body":tokens(&conversion.block)}),
    );
    Ok(json!({"constants":constants,"transaction_type_rules":methods}))
}

fn constant_name(name: &str) -> String {
    let mut output = String::new();
    for (index, character) in name.chars().enumerate() {
        if index > 0 && character.is_ascii_uppercase() {
            output.push('_');
        }
        output.push(character.to_ascii_uppercase());
    }
    output
}

fn render_runtime(
    mortality: &Value,
    errors: &Value,
    fees: &Value,
    rules: &Value,
) -> Result<Vec<u8>> {
    let mut out = header(mortality)?;
    out.push_str(&format!("/// Maximum mortal era accepted for a shielded transaction.\npub const MAX_SHIELD_ERA_PERIOD: u64 = {};\n", mortality["values"]["MAX_SHIELD_ERA_PERIOD"]["value"]));
    out.push_str(&header(errors)?);
    out.push_str("/// Runtime variants indexed by the custom transaction rejection code.\npub const CUSTOM_TRANSACTION_ERRORS: &[(u8, &str)] = &[\n");
    for row in errors["values"]
        .as_array()
        .context("custom error rows missing")?
    {
        out.push_str(&format!(
            "({}, {}),\n",
            row["code"],
            rust_string(
                row["runtime_variant"]
                    .as_str()
                    .context("runtime variant missing")?
            )
        ));
    }
    out.push_str("];\n");
    out.push_str(&header(fees)?);
    out.push_str("/// Leaf calls whose transaction fee is charged to the hotkey owner's coldkey.\npub const COLDKEY_PAYS_FEE_CALLS: &[(&str, &str)] = &[\n");
    for pair in fees["values"].as_array().context("fee calls missing")? {
        out.push_str(&format!(
            "({}, {}),\n",
            rust_string(pair[0].as_str().context("fee pallet missing")?),
            rust_string(pair[1].as_str().context("fee call missing")?)
        ));
    }
    out.push_str("];\n");
    out.push_str(&header(rules)?);
    out.push_str("/// Runtime type-value defaults for production and fast-block builds.\npub mod rate_limits {\n/// A block-count default selected by the runtime build mode.\n#[derive(Copy, Clone, Debug, Eq, PartialEq)]\npub struct RateLimit {\n/// Number of blocks in production builds.\npub production: u64,\n/// Number of blocks in fast-block builds.\npub fast: u64,\n}\n");
    for (name, pair) in rules["values"]["rate_limits"]
        .as_object()
        .context("rate pairs missing")?
    {
        out.push_str(&format!(
            "/// Block-count defaults from runtime `{name}`.\npub const {}: RateLimit = RateLimit {{ production: {}, fast: {} }};\n",
            constant_name(name),
            pair["production"],
            pair["fast"]
        ));
    }
    out.push_str(
        "}\n/// Chain bounds and defaults mirrored by the framework.\npub mod chain_rules {\n",
    );
    for (name, row) in rules["values"]["constants"]
        .as_object()
        .context("chain constants missing")?
    {
        let name = if name == "DefaultAdminFreezeWindow" {
            "DEFAULT_ADMIN_FREEZE_WINDOW"
        } else {
            name
        };
        out.push_str(&format!(
            "/// Extracted runtime bound or default `{name}`.\npub const {name}: {} = {};\n",
            row["type"]
                .as_str()
                .context("chain constant type missing")?,
            row["value"]
        ));
    }
    out.push_str("}\n");
    Ok(out.into_bytes())
}

pub(super) fn outputs(
    root: &Path,
    pins: &DocumentMut,
    tables: &Value,
) -> Result<Vec<(&'static str, Vec<u8>)>> {
    let resolver = Resolver::new(root)?;
    let codes = custom_codes(&resolver.source(TRANSACTION_ERRORS)?)?;
    let mut errors = document(
        &resolver,
        pins,
        TRANSACTION_ERRORS,
        custom_error_rows(
            &codes,
            extracted(tables, ERROR_MAP, "CUSTOM_TRANSACTION_ERRORS")?,
        )?,
    );
    errors["python_mirror"] = json!({"provenance":tables[ERROR_MAP]["provenance"],"table":"CUSTOM_TRANSACTION_ERRORS","source":tables[ERROR_MAP]["values"]["CUSTOM_TRANSACTION_ERRORS"]});
    let calls = fee_calls(&resolver.source(FEE_FILTERS)?)?;
    check_fee_mirror(
        &calls,
        extracted(tables, PY_FEE_FILTERS, "COLDKEY_PAYS_FEE_CALLS")?,
    )?;
    let mut fees = document(&resolver, pins, FEE_FILTERS, json!(calls));
    fees["python_mirror"] = json!({"provenance":tables[PY_FEE_FILTERS]["provenance"],"table":"COLDKEY_PAYS_FEE_CALLS","source":tables[PY_FEE_FILTERS]["values"]["COLDKEY_PAYS_FEE_CALLS"]});
    let mortality_file = syn::parse_file(&resolver.source(MORTALITY)?)?;
    let mortality = document(
        &resolver,
        pins,
        MORTALITY,
        json!({"MAX_SHIELD_ERA_PERIOD":numeric_constant(&mortality_file.items,"MAX_SHIELD_ERA_PERIOD","u64")?}),
    );
    let pallet_source = pallet_source(root, pins)?;
    let pallet_file = syn::parse_file(&pallet_source)?;
    let items = pallet_items(&pallet_file)?;
    let mut rules = chain_rules(items, &resolver.source(RATE_LIMITING)?)?;
    rules["rate_limits"] = rate_pairs(items)?;
    let mut rules = document(&resolver, pins, PALLET, rules);
    rules["provenance"]["sha256"] = json!(pins["artifacts"][PALLET]["sha256"].as_str());
    rules["provenance"]["blob"] = json!(super::super::git_blob(pallet_source.as_bytes()));
    rules["rate_limiting_provenance"] =
        document(&resolver, pins, RATE_LIMITING, Value::Null)["provenance"].clone();
    rules["mortality"] = mortality.clone();
    let proxy_contract = proxy_contract::output(root, pins, tables)?;
    let reveal_contract = reveal_contract::output(root, pins)?;
    rules["fee_contract"] = fee_contract::output(root, pins)?;
    rules["proxy_contract"] = proxy_contract;
    rules["reveal_contract"] = reveal_contract;
    let (pool_rust, pool_outputs) = super::pool_rpc::outputs(root)?;
    let mut runtime = render_runtime(&mortality, &errors, &fees, &rules)?;
    runtime.extend_from_slice(pool_rust.as_bytes());
    let mut outputs = vec![
        (
            "vendor/upstream/extracted/transaction_errors.json",
            pretty(&errors)?,
        ),
        ("vendor/upstream/extracted/fee_filters.json", pretty(&fees)?),
        (
            "vendor/upstream/extracted/chain_rules.json",
            pretty(&rules)?,
        ),
        ("crates/rtao-catalog/src/rt.rs", rustfmt(runtime)?),
    ];
    outputs.extend(pool_outputs);
    Ok(outputs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn source(path: &str) -> Result<String> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root missing")?;
        if path == PALLET {
            let pins = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
            return pallet_source(root, &pins);
        }
        Resolver::new(root)?.source(path)
    }

    fn tables() -> Result<Value> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root missing")?;
        let document: Value = serde_json::from_slice(&fs::read(
            root.join("vendor/upstream/extracted/named_tables.json"),
        )?)?;
        Ok(document["tables"].clone())
    }

    #[test]
    fn pallet_extra_input_requires_exact_destination_and_artifact_hash() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root missing")?;
        let mut pins = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
        let original_hash = pins["artifacts"][PALLET]["sha256"].clone();
        pins["artifacts"][PALLET]["sha256"] = toml_edit::value("0".repeat(64));
        assert!(pallet_source(root, &pins).is_err());
        pins["artifacts"][PALLET]["sha256"] = original_hash;
        pins["artifacts"][PALLET]["to"] =
            toml_edit::value("vendor/upstream/src/runtime/src/fee_filters.rs");
        assert!(pallet_source(root, &pins).is_err());
        Ok(())
    }

    #[test]
    fn custom_codes_check_rust_coverage_and_python_semantic_projection() -> Result<()> {
        let source = source(TRANSACTION_ERRORS)?;
        let codes = custom_codes(&source)?;
        let tables = tables()?;
        let python = extracted(&tables, ERROR_MAP, "CUSTOM_TRANSACTION_ERRORS")?;
        let rows = custom_error_rows(&codes, python)?;
        assert_eq!(rows.as_array().context("rows missing")?.len(), 29);
        assert_eq!(codes[&255], "BadRequest");
        for changed in [
            source.replace("BalanceTooLow => 2", "BalanceTooLow => 3"),
            source.replace("BalanceTooLow => 2", "BalanceTooLow => 256"),
            source.replace("BalanceTooLow => 2", "BalanceTooLow if true => 2"),
        ] {
            assert!(custom_codes(&changed).is_err());
        }
        let changed = custom_codes(&source.replace("BalanceTooLow => 2", "BalanceTooLow => 28"))?;
        assert!(custom_error_rows(&changed, python).is_err());
        let mut changed = python.clone();
        changed["dict"][0][1] = json!("ColdkeyInSwapSchedule");
        assert!(custom_error_rows(&codes, &changed).is_err());
        Ok(())
    }

    #[test]
    fn fee_filter_checks_authoritative_macro_routing_and_python_mirror() -> Result<()> {
        let source = source(FEE_FILTERS)?;
        let calls = fee_calls(&source)?;
        let tables = tables()?;
        let python = extracted(&tables, PY_FEE_FILTERS, "COLDKEY_PAYS_FEE_CALLS")?;
        check_fee_mirror(&calls, python)?;
        assert_eq!(calls.len(), 17);
        let changed = fee_calls(&source.replace(
            "SubtensorCall::serve_axon_tls",
            "SubtensorCall::serve_new_axon",
        ))?;
        assert!(check_fee_mirror(&changed, python).is_err());
        assert!(
            fee_calls(&source.replace("ColdkeyPaysFeeCalls::contains(call)", "false")).is_err()
        );
        assert!(fee_calls(&source.replace("RuntimeCall::Commitments(CommitmentsCall::set_commitment),", "RuntimeCall::Commitments(CommitmentsCall::set_commitment), RuntimeCall::Commitments(CommitmentsCall::set_commitment),")).is_err());
        Ok(())
    }

    #[test]
    fn runtime_bounds_preserve_widths_and_checked_rate_limit_arithmetic() -> Result<()> {
        let pallet_source = source(PALLET)?;
        let file = syn::parse_file(&pallet_source)?;
        let items = pallet_items(&file)?;
        let pairs = rate_pairs(items)?;
        assert_eq!(pairs["MaxUidsTrimmingRateLimit"]["production"], 216000);
        assert_eq!(pairs["MechanismCountSetRateLimit"]["fast"], 1);
        assert_eq!(pairs["DefaultPendingCooldown"]["fast"], 15);
        let rules = chain_rules(items, &source(RATE_LIMITING)?)?;
        assert_eq!(
            rules["constants"]["DefaultAdminFreezeWindow"]["type"],
            "u16"
        );
        assert_eq!(rules["constants"]["MIN_TEMPO"]["value"], 360);
        let changed =
            syn::parse_file(&pallet_source.replace("30 * 7200", "18446744073709551615 * 7200"))?;
        assert!(rate_pairs(pallet_items(&changed)?).is_err());
        let changed = syn::parse_file(&pallet_source.replace("MIN_TEMPO: u16", "MIN_TEMPO: u32"))?;
        assert!(chain_rules(pallet_items(&changed)?, &source(RATE_LIMITING)?).is_err());
        assert!(number(&syn::parse_str("(65535 + 1) * 0")?, 0, "u16").is_err());
        assert!(number(&syn::parse_str("30u8 * 7200")?, 0, "u64").is_err());
        let mortality_file = syn::parse_file(&source(MORTALITY)?)?;
        assert_eq!(
            numeric_constant(&mortality_file.items, "MAX_SHIELD_ERA_PERIOD", "u64")?["value"],
            8
        );
        Ok(())
    }
}
