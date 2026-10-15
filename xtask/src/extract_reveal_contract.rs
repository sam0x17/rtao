//! Source-backed timelock payload and reveal-epoch projections.
//!
//! These are deliberately narrower than the entire runtime reveal loop: decryption,
//! missing-pulse retries and dispatch acceptance remain runtime-owned behavior.
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use syn::{Expr, ImplItem, Item, Pat, Stmt};
use toml_edit::DocumentMut;

use super::{Resolver, document, tokens};

const SOURCE: &str = "pallets/subtensor/src/coinbase/reveal_commits.rs";
const CORE: &str = "sdk/bittensor-core/src/timelock/mod.rs";

fn payload(file: &syn::File, name: &str) -> Result<Value> {
    let structures = file
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Struct(item) if item.ident == name => Some(item),
            _ => None,
        })
        .collect::<Vec<_>>();
    ensure!(
        structures.len() == 1,
        "{name}: payload declaration missing or ambiguous"
    );
    let structure = structures[0];
    ensure!(
        structure.generics.params.is_empty()
            && !structure
                .attrs
                .iter()
                .any(|attr| attr.path().is_ident("cfg") || attr.path().is_ident("cfg_attr")),
        "conditional/generic timelock payload unsupported"
    );
    let syn::Fields::Named(fields) = &structure.fields else {
        bail!("timelock payload must have named fields")
    };
    let fields = fields
        .named
        .iter()
        .map(|field| {
            ensure!(
                field.attrs.is_empty(),
                "timelock payload field attributes changed"
            );
            Ok((
                field
                    .ident
                    .as_ref()
                    .context("payload field name absent")?
                    .to_string(),
                tokens(&field.ty),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let derives = structure
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("derive"))
        .map(|attr| {
            attr.parse_args_with(
                syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated,
            )
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    ensure!(
        ["Encode", "Decode"]
            .iter()
            .all(|name| derives.iter().flatten().any(|path| path.is_ident(*name))),
        "timelock payload must derive Encode and Decode"
    );
    let freeze = structure
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("freeze_struct"))
        .map(|attr| attr.parse_args::<syn::LitStr>().map(|value| value.value()))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    ensure!(
        freeze.len() == 1,
        "timelock payload freeze_struct hash absent or ambiguous"
    );
    ensure!(
        !freeze[0].is_empty()
            && freeze[0].len() <= 16
            && freeze[0].bytes().all(|byte| byte.is_ascii_hexdigit()),
        "timelock payload freeze_struct label is malformed"
    );
    Ok(
        json!({"fields":fields.iter().map(|(name,ty)| json!({"name":name,"type":ty})).collect::<Vec<_>>(),"freeze_struct":freeze[0]}),
    )
}

fn epoch_prefix(file: &syn::File) -> Result<()> {
    let methods = file
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Impl(item) => Some(item),
            _ => None,
        })
        .flat_map(|implementation| {
            implementation
                .items
                .iter()
                .filter_map(move |item| match item {
                    ImplItem::Fn(method)
                        if method.sig.ident == "reveal_crv3_commits_for_subnet" =>
                    {
                        Some((implementation, method))
                    }
                    _ => None,
                })
        })
        .collect::<Vec<_>>();
    ensure!(methods.len() == 1, "reveal loop missing or duplicated");
    let (implementation, method) = methods[0];
    let expected: syn::ItemImpl = syn::parse_str("impl<T: Config> Pallet<T> {}")?;
    ensure!(
        implementation.trait_.is_none()
            && implementation.unsafety.is_none()
            && implementation.defaultness.is_none()
            && tokens(&implementation.self_ty) == tokens(&expected.self_ty)
            && implementation.generics.where_clause.is_none()
            && tokens(&implementation.generics) == tokens(&expected.generics)
            && !implementation
                .attrs
                .iter()
                .any(|attr| { attr.path().is_ident("cfg") || attr.path().is_ident("cfg_attr") }),
        "reveal method requires the unconditional inherent Pallet<T> implementation"
    );
    let signature: syn::Signature = syn::parse_str(
        "fn reveal_crv3_commits_for_subnet(netuid: NetUid) -> dispatch::DispatchResult",
    )?;
    ensure!(
        tokens(&method.sig) == tokens(&signature)
            && !method
                .attrs
                .iter()
                .any(|attr| attr.path().is_ident("cfg") || attr.path().is_ident("cfg_attr")),
        "conditional reveal method unsupported"
    );
    for (index, (name, expression)) in [
        ("reveal_period", "Self::get_reveal_period(netuid)"),
        ("cur_epoch", "Self::current_epoch_with_lookahead(netuid)"),
        ("reveal_epoch", "cur_epoch.saturating_sub(reveal_period)"),
    ]
    .into_iter()
    .enumerate()
    {
        let Some(Stmt::Local(local)) = method.block.stmts.get(index) else {
            bail!("reveal epoch prefix changed")
        };
        ensure!(
            local.attrs.is_empty()
                && matches!(&local.pat, Pat::Ident(pattern) if pattern.ident == name && pattern.by_ref.is_none() && pattern.mutability.is_none() && pattern.subpat.is_none()),
            "reveal epoch binding changed"
        );
        let init = local
            .init
            .as_ref()
            .context("reveal epoch initializer missing")?;
        let expected: Expr = syn::parse_str(expression)?;
        ensure!(
            init.diverge.is_none() && tokens(&init.expr) == tokens(&expected),
            "reveal epoch expression changed: {name}"
        );
    }
    Ok(())
}

fn values(chain: &str, core: &str) -> Result<Value> {
    let chain = syn::parse_file(chain)?;
    let core = syn::parse_file(core)?;
    let modern = payload(&chain, "WeightsTlockPayload")?;
    let canonical = payload(&core, "WeightsTlockPayload")?;
    let legacy = payload(&chain, "LegacyWeightsTlockPayload")?;
    let expected = |rows: &[(&str, &str)]| {
        rows.iter()
            .map(|(name, ty)| json!({"name":name,"type":ty}))
            .collect::<Vec<_>>()
    };
    ensure!(
        modern["fields"]
            == json!(expected(&[
                ("hotkey", "Vec < u8 >"),
                ("uids", "Vec < u16 >"),
                ("values", "Vec < u16 >"),
                ("version_key", "u64")
            ])),
        "chain canonical timelock payload layout changed"
    );
    ensure!(
        modern["fields"] == canonical["fields"],
        "core timelock payload differs from chain SCALE field order/types"
    );
    ensure!(
        legacy["fields"]
            == json!(expected(&[
                ("uids", "Vec < u16 >"),
                ("values", "Vec < u16 >"),
                ("version_key", "u64")
            ])),
        "legacy timelock payload layout changed"
    );
    epoch_prefix(&chain)?;
    Ok(
        json!({"canonical_payload":modern,"legacy_payload":legacy,"core_payload":canonical,
        "epoch":{"counter":"current_epoch_with_lookahead","operation":"saturating_sub","rhs":"get_reveal_period(netuid)"}}),
    )
}

pub(super) fn output(root: &Path, pins: &DocumentMut) -> Result<Value> {
    let resolver = Resolver::new(root)?;
    let source = resolver.source(SOURCE)?;
    let core = resolver.source(CORE)?;
    let mut result = document(&resolver, pins, SOURCE, values(&source, &core)?);
    result["core_provenance"] = document(&resolver, pins, CORE, Value::Null)["provenance"].clone();
    result["scope"] = json!(
        "SCALE payload fields and first three reveal-epoch bindings only; runtime reveal/decryption/dispatch acceptance is not certified"
    );
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn inputs() -> Result<(String, String)> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root missing")?;
        let resolver = Resolver::new(root)?;
        Ok((resolver.source(SOURCE)?, resolver.source(CORE)?))
    }
    #[test]
    fn reveal_projection_rejects_payload_order_type_and_epoch_rule_drift() -> Result<()> {
        let (chain, core) = inputs()?;
        let actual = values(&chain, &core)?;
        assert_eq!(actual["canonical_payload"]["fields"][0]["name"], "hotkey");
        for changed in [
            chain.replace(
                "impl<T: Config> Pallet<T>",
                "#[cfg(feature = \"alternate-reveal\")]\nimpl<T: Config> Pallet<T>",
            ),
            chain.replace(
                "impl<T: Config> Pallet<T>",
                "impl<T: Config> OtherPallet<T>",
            ),
            chain.replace(
                "impl<T: Config> Pallet<T>",
                "impl<T: Config> Pallet<T> where T: AlternateConfig",
            ),
            format!(
                "const netuid: NetUid = NetUid(0);\n{}",
                chain.replace(
                    "reveal_crv3_commits_for_subnet(netuid: NetUid)",
                    "reveal_crv3_commits_for_subnet(_ignored_netuid: NetUid)"
                )
            ),
            chain.replacen("pub uids: Vec<u16>", "pub uids: Vec<u32>", 1),
            chain.replacen(
                "pub hotkey: Vec<u8>,\n    pub uids: Vec<u16>",
                "pub uids: Vec<u16>,\n    pub hotkey: Vec<u8>",
                1,
            ),
            chain.replace(
                "cur_epoch.saturating_sub(reveal_period)",
                "cur_epoch.saturating_add(reveal_period)",
            ),
            chain.replace(
                "Self::current_epoch_with_lookahead(netuid)",
                "Self::get_epoch_index(netuid, 0)",
            ),
        ] {
            assert_ne!(changed, chain, "negative fixture must mutate source");
            assert!(values(&changed, &core).is_err());
        }
        assert!(
            values(
                &chain,
                &core.replacen("pub values: Vec<u16>", "pub values: Vec<u32>", 1)
            )
            .is_err()
        );
        Ok(())
    }
}
