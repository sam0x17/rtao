//! Reconcile the chain's ProxyType codec contract with metadata and Python.
//!
//! This checks the enum and both numeric conversion implementations, rather
//! than attributing independently generated metadata models to Python source.

use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{Context, Result, bail, ensure};
use parity_scale_codec::DecodeAll;
use serde_json::{Value, json};
use syn::{Expr, ImplItem, Item, ItemImpl, Pat};
use toml_edit::DocumentMut;

use super::{Resolver, document, tokens};

const SOURCE: &str = "common/src/proxy.rs";
const PYTHON: &str = "sdk/python/bittensor/intents/proxy.py";

fn implementation<'a>(
    file: &'a syn::File,
    trait_name: &str,
    self_type: &str,
) -> Result<&'a ItemImpl> {
    let expected_trait: syn::Path = syn::parse_str(trait_name)?;
    let expected_type: syn::Type = syn::parse_str(self_type)?;
    let mut matches = file.items.iter().filter_map(|item| match item {
        Item::Impl(item)
            if item
                .trait_
                .as_ref()
                .is_some_and(|(_, path, _)| tokens(path) == tokens(&expected_trait))
                && tokens(&item.self_ty) == tokens(&expected_type) =>
        {
            Some(item)
        }
        _ => None,
    });
    let result = matches
        .next()
        .with_context(|| format!("proxy {trait_name} implementation missing"))?;
    ensure!(
        matches.next().is_none() && result.attrs.is_empty(),
        "proxy codec implementation is conditional or duplicated"
    );
    Ok(result)
}

fn matching<'a>(
    implementation: &'a ItemImpl,
    signature: &str,
    argument: &str,
) -> Result<&'a syn::ExprMatch> {
    let expected: syn::Signature = syn::parse_str(signature)?;
    let methods = implementation
        .items
        .iter()
        .filter_map(|item| match item {
            ImplItem::Fn(method) if method.sig.ident == expected.ident => Some(method),
            _ => None,
        })
        .collect::<Vec<_>>();
    ensure!(
        methods.len() == 1
            && methods[0].attrs.is_empty()
            && tokens(&methods[0].sig) == tokens(&expected),
        "proxy conversion method changed"
    );
    let Expr::Match(matching) = super::tail(&methods[0].block)? else {
        bail!("proxy conversion must be a direct match")
    };
    ensure!(
        super::expression_path_is(&matching.expr, &[argument]) && matching.attrs.is_empty(),
        "proxy conversion input changed"
    );
    Ok(matching)
}

fn single_variant(expression: &Expr, prefix: &str) -> Result<String> {
    let Expr::Path(path) = expression else {
        bail!("proxy conversion variant must be a path")
    };
    ensure!(
        path.qself.is_none()
            && path.path.segments.len() == 2
            && path.path.segments[0].ident == prefix,
        "proxy conversion variant path changed"
    );
    let name = path.path.segments[1].ident.to_string();
    ensure!(
        super::path_is(&path.path, &[prefix, &name]),
        "proxy variant has unsupported path arguments"
    );
    Ok(name)
}

fn proxy_types(source: &str) -> Result<Vec<(u8, String)>> {
    let file = syn::parse_file(source)?;
    let enums = file
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Enum(item) if item.ident == "ProxyType" => Some(item),
            _ => None,
        })
        .collect::<Vec<_>>();
    ensure!(enums.len() == 1, "chain ProxyType missing or duplicated");
    ensure!(
        !enums[0]
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident("cfg") || attr.path().is_ident("cfg_attr")),
        "conditional ProxyType unsupported"
    );
    let derives = enums[0]
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
        "ProxyType must retain derived SCALE Encode and Decode"
    );
    let variants = enums[0]
        .variants
        .iter()
        .enumerate()
        .map(|(index, variant)| {
            ensure!(
                variant.fields.is_empty()
                    && variant.discriminant.is_none()
                    && variant.attrs.is_empty(),
                "proxy variant no longer unconditional implicit unit variant"
            );
            Ok((u8::try_from(index)?, variant.ident.to_string()))
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(!variants.is_empty(), "empty ProxyType");
    let forward = matching(
        implementation(&file, "TryFrom<u8>", "ProxyType")?,
        "fn try_from(value: u8) -> Result<Self, Self::Error>",
        "value",
    )?;
    let mut decoded = BTreeMap::new();
    let mut wildcard = 0;
    for arm in &forward.arms {
        ensure!(
            arm.guard.is_none() && arm.attrs.is_empty(),
            "conditional proxy conversion arm"
        );
        if matches!(arm.pat, Pat::Wild(_)) {
            let expected: Expr = syn::parse_str("Err(())")?;
            ensure!(
                tokens(&arm.body) == tokens(&expected),
                "unknown proxy index must return Err(())"
            );
            wildcard += 1;
            continue;
        }
        let Pat::Lit(syn::ExprLit {
            lit: syn::Lit::Int(index),
            ..
        }) = &arm.pat
        else {
            bail!("proxy index must be an integer")
        };
        let Expr::Call(ok) = arm.body.as_ref() else {
            bail!("proxy decode must return Ok variant")
        };
        ensure!(
            super::expression_path_is(&ok.func, &["Ok"]) && ok.args.len() == 1,
            "proxy decode must return one Ok variant"
        );
        let name = single_variant(&ok.args[0], "Self")?;
        ensure!(
            decoded.insert(index.base10_parse::<u8>()?, name).is_none(),
            "duplicate proxy decode index"
        );
    }
    ensure!(
        wildcard == 1 && decoded == variants.iter().cloned().collect(),
        "proxy TryFrom indices differ from SCALE enum order"
    );
    // A wildcard before a literal arm could shadow a correct-looking table.
    ensure!(
        forward
            .arms
            .last()
            .is_some_and(|arm| matches!(arm.pat, Pat::Wild(_))),
        "proxy unknown-index arm must be last"
    );
    let reverse = matching(
        implementation(&file, "From<ProxyType>", "u8")?,
        "fn from(proxy_type: ProxyType) -> Self",
        "proxy_type",
    )?;
    let mut encoded = BTreeMap::new();
    for arm in &reverse.arms {
        ensure!(
            arm.guard.is_none() && arm.attrs.is_empty(),
            "conditional proxy encode arm"
        );
        let Pat::Path(path) = &arm.pat else {
            bail!("proxy encode pattern must name variant")
        };
        let name = single_variant(&Expr::Path(path.clone()), "ProxyType")?;
        let Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Int(index),
            ..
        }) = arm.body.as_ref()
        else {
            bail!("proxy encode must return integer")
        };
        ensure!(
            encoded.insert(index.base10_parse::<u8>()?, name).is_none(),
            "duplicate proxy encode index"
        );
    }
    ensure!(
        encoded == decoded,
        "proxy From conversion differs from TryFrom/enum"
    );
    Ok(variants)
}

pub(super) fn output(root: &Path, pins: &DocumentMut, tables: &Value) -> Result<Value> {
    let resolver = Resolver::new(root)?;
    let source = resolver.source(SOURCE)?;
    let variants = proxy_types(&source)?;
    let names = variants
        .iter()
        .map(|(_, name)| name.as_str())
        .collect::<Vec<_>>();
    let python = super::extracted(tables, PYTHON, "PROXY_TYPES")?["tuple"]
        .as_array()
        .context("Python PROXY_TYPES must be a tuple")?
        .iter()
        .map(|name| name.as_str().context("Python proxy name must be text"))
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        names == python,
        "chain ProxyType differs from authenticated Python PROXY_TYPES"
    );
    crate::metadata_pin::verify(root, pins)?;
    let pin = &pins["networks"]["finney"]["metadata"];
    let bytes = fs::read(root.join(pin["v15"].as_str().context("V15 path missing")?))?;
    let frame_metadata::RuntimeMetadata::V15(metadata) =
        frame_metadata::RuntimeMetadataPrefixed::decode_all(&mut &bytes[..])?.1
    else {
        bail!("proxy comparison requires V15 metadata")
    };
    let types = metadata
        .types
        .types
        .iter()
        .filter(|ty| {
            ty.ty
                .path
                .segments
                .last()
                .is_some_and(|name| name == "ProxyType")
        })
        .collect::<Vec<_>>();
    ensure!(types.len() == 1, "metadata ProxyType missing or ambiguous");
    let scale_info::TypeDef::Variant(metadata) = &types[0].ty.type_def else {
        bail!("metadata ProxyType must be variant type")
    };
    let metadata = metadata
        .variants
        .iter()
        .map(|variant| {
            ensure!(
                variant.fields.is_empty(),
                "metadata proxy variant acquired fields"
            );
            Ok((variant.index, variant.name.clone()))
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        variants == metadata,
        "chain ProxyType codec indices differ from authenticated metadata"
    );
    let mut result = document(
        &resolver,
        pins,
        SOURCE,
        json!({"variants":variants.iter().map(|(index,name)| json!({"index":index,"name":name})).collect::<Vec<_>>(), "unknown_index":"Err(())"}),
    );
    result["python_mirror"] =
        json!({"path":PYTHON,"provenance":tables[PYTHON]["provenance"],"table":"PROXY_TYPES"});
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source() -> Result<String> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root missing")?;
        Resolver::new(root)?.source(SOURCE)
    }
    #[test]
    fn proxy_codec_rejects_index_order_conversion_and_unknown_fallback_drift() -> Result<()> {
        let source = source()?;
        let actual = proxy_types(&source)?;
        assert_eq!(actual.len(), 19);
        assert_eq!(actual[18], (18, "BasketTrading".into()));
        for changed in [
            format!(
                "const value: u8 = 0;\n{}",
                source.replace("fn try_from(value: u8)", "fn try_from(_ignored_value: u8)")
            ),
            format!(
                "const proxy_type: ProxyType = ProxyType::Any;\n{}",
                source.replace(
                    "fn from(proxy_type: ProxyType)",
                    "fn from(_ignored_proxy_type: ProxyType)"
                )
            ),
            source.replace("    Encode,\n", ""),
            source.replace(
                "            18 => Ok(Self::BasketTrading)",
                "            19 => Ok(Self::BasketTrading)",
            ),
            source.replace(
                "ProxyType::BasketTrading => 18",
                "ProxyType::BasketTrading => 17",
            ),
            source.replace("            _ => Err(())", "            _ => Ok(Self::Any)"),
            source.replace("    BasketTrading,", "    BasketTrading = 42,"),
            source.replace(
                "    Owner,\n    NonCritical,",
                "    NonCritical,\n    Owner,",
            ),
            source.replace(
                "            0 => Ok(Self::Any)",
                "            0 if value == 0 => Ok(Self::Any)",
            ),
        ] {
            assert_ne!(source, changed, "negative fixture must change actual input");
            assert!(proxy_types(&changed).is_err());
        }
        Ok(())
    }
    #[test]
    fn proxy_output_authenticates_chain_source_python_mirror_and_metadata() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root missing")?;
        let pins = fs::read_to_string(root.join("upstream.toml"))?.parse::<DocumentMut>()?;
        let tables: Value = serde_json::from_slice(&fs::read(
            root.join("vendor/upstream/extracted/named_tables.json"),
        )?)?;
        let tables = &tables["tables"];
        let output = output(root, &pins, tables)?;
        assert_eq!(output["provenance"]["path"], SOURCE);
        assert_eq!(
            output["values"]["variants"]
                .as_array()
                .context("variants missing")?
                .len(),
            19
        );
        let mut changed = tables.clone();
        changed[PYTHON]["values"]["PROXY_TYPES"]["value"]["tuple"][0] = json!("Other");
        assert!(super::output(root, &pins, &changed).is_err());
        Ok(())
    }
}
