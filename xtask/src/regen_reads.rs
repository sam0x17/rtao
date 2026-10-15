//! Catalog-derived read names, typed parameters, defaults and exhaustive dispatch.

use anyhow::{Context, Result, bail, ensure};
use rtao_catalog::{ParamSpec, ReadSpec};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};
use toml_edit::DocumentMut;

fn pascal(name: &str) -> String {
    name.split('_')
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or(String::new(), |first| {
                first.to_uppercase().collect::<String>() + chars.as_str()
            })
        })
        .collect()
}
fn setting<'a>(config: &'a DocumentMut, section: &str, key: &str) -> Option<&'a str> {
    config
        .get(section)
        .and_then(|v| v.get(key))
        .and_then(|v| v.as_str())
}
fn param_type(spec: &ReadSpec, field: &ParamSpec, config: &DocumentMut) -> Result<String> {
    if let Some(ty) = setting(config, "fields", &format!("{}.{}", spec.name, field.name)) {
        return Ok(ty.to_owned());
    }
    let annotation = field.annotation.context("read annotation missing")?;
    let optional = annotation.starts_with("Optional[") || annotation.ends_with(" | None");
    let annotation = annotation
        .strip_prefix("Optional[")
        .and_then(|a| a.strip_suffix(']'))
        .or_else(|| annotation.strip_suffix(" | None"))
        .unwrap_or(annotation);
    let ty = if field.name.ends_with("_ss58") {
        "rtao_primitives::AccountId"
    } else if field.name.ends_with("_ss58s") {
        "Vec<rtao_primitives::AccountId>"
    } else if field.name == "netuid" || field.name.ends_with("_netuid") {
        "rtao_primitives::NetUid"
    } else if field.name == "mechid" {
        "rtao_primitives::MechId"
    } else if field.name == "uid" {
        "rtao_primitives::Uid"
    } else {
        match annotation {
            "int" => "i64",
            "float" => "f64",
            "str" => "String",
            "bool" => "bool",
            "list[int]" => "Vec<i64>",
            "list[str]" => "Vec<String>",
            _ => bail!(
                "unmapped read {}.{} annotation {annotation}",
                spec.name,
                field.name
            ),
        }
    };
    Ok(if optional {
        format!("Option<{ty}>")
    } else {
        ty.to_owned()
    })
}
fn default_expr(ty: &str, value: &Value) -> Result<String> {
    if value.is_null() {
        ensure!(ty.starts_with("Option<"), "null default for {ty}");
        return Ok("None".into());
    }
    if let Some(inner) = ty.strip_prefix("Option<").and_then(|s| s.strip_suffix('>')) {
        return Ok(format!("Some({})", default_expr(inner, value)?));
    }
    if ty == "String" {
        return Ok(format!(
            "{:?}.to_owned()",
            value.as_str().context("read string default")?
        ));
    }
    if ty == "f64" {
        return Ok(format!(
            "{:?}",
            value.as_f64().context("read float default")?
        ));
    }
    if ty.starts_with("rtao_primitives::") {
        return Ok(format!("{ty}({value})"));
    }
    if let Some(inner) = ty.strip_prefix("Vec<").and_then(|s| s.strip_suffix('>')) {
        return Ok(format!(
            "vec![{}]",
            value
                .as_array()
                .context("read array default")?
                .iter()
                .map(|v| default_expr(inner, v))
                .collect::<Result<Vec<_>>>()?
                .join(",")
        ));
    }
    Ok(value.to_string())
}

fn hit(value: &Value) -> Result<(String, String, String)> {
    let string = |key: &str| {
        value[key]
            .as_str()
            .map(str::to_owned)
            .with_context(|| format!("hit {key} missing"))
    };
    Ok((string("kind")?, string("container")?, string("name")?))
}

fn render_hit(hit: &(String, String, String)) -> Result<String> {
    let kind = match hit.0.as_str() {
        "storage" => "Storage",
        "runtime" => "RuntimeApi",
        "constant" => "Constant",
        kind => bail!("unknown replacement hit kind {kind}"),
    };
    Ok(format!(
        "Hit{{kind:HitKind::{kind},container:{:?},name:{:?}}}",
        hit.1, hit.2
    ))
}

fn specifications(root: &Path) -> Result<String> {
    let extracted: Value = serde_json::from_slice(&fs::read(
        root.join("vendor/upstream/extracted/read_hits.json"),
    )?)?;
    let extracted: BTreeMap<_, _> = extracted["reads"]
        .as_array()
        .context("extracted read hits")?
        .iter()
        .map(|row| {
            Ok((
                row["name"].as_str().context("hit read name")?.to_owned(),
                row.clone(),
            ))
        })
        .collect::<Result<_>>()?;
    ensure!(
        extracted.len() == 102,
        "read hit coverage must include102reads"
    );
    let extras: DocumentMut =
        fs::read_to_string(root.join("parity/read_hits_extra.toml"))?.parse()?;
    let mut out = String::from(
        "//! Authenticated read metadata and complete reviewed descriptor coverage.\n\
/// Descriptor family in a read's complete hit set.\n#[derive(Clone,Copy,Debug,PartialEq,Eq,PartialOrd,Ord)]\npub enum HitKind {\n/// Storage value or map scan.\nStorage,\n/// Runtime API invocation.\nRuntimeApi,\n/// Runtime metadata constant.\nConstant,}\n\
/// A descriptor included in the static or reviewed out-of-module coverage.\n#[derive(Clone,Copy,Debug,PartialEq,Eq,PartialOrd,Ord)]\npub struct Hit{\n/// Descriptor family.\npub kind:HitKind,\n/// Pallet or runtime API name.\npub container:&'static str,\n/// Item or method name.\npub name:&'static str,}\n\
impl Hit {\n/// Match an access observed by the client at any block.\npub fn matches(&self,touch:&rtao_client::ReadTouch)->bool{let kind=match self.kind{HitKind::Storage=>rtao_client::ReadKind::Storage,HitKind::RuntimeApi=>rtao_client::ReadKind::RuntimeApi,HitKind::Constant=>rtao_client::ReadKind::Constant};kind==touch.kind && self.container==touch.pallet_or_api && self.name==touch.item}}\n\
/// JSON parameter kind authenticated by the upstream catalog.\n#[derive(Clone,Copy,Debug,PartialEq,Eq)]\npub enum JsonType {\n/// An integral token.\nInteger,\n/// A numeric token.\nNumber,\n/// A text value.\nString,\n/// A true or false value.\nBoolean,\n/// A sequence of values.\nArray,}\n\
/// Signature defaults and CLI metadata for one read argument.\n#[derive(Clone,Copy,Debug,PartialEq,Eq)]\npub struct ParamSpec {\n/// Python argument name.\npub name:&'static str,\n/// Catalog JSON kind.\npub json_type:JsonType,\n/// Catalog argument documentation.\npub doc:&'static str,\n/// Whether the query CLI requires an explicit value.\npub required:bool,\n/// Exact Python signature default as a JSON literal.\npub default:Option<&'static str>,\n/// Whether the CLI may obtain the argument from the selected wallet.\npub own_key:bool,}\n\
/// State identity policy required by the read.\n#[derive(Clone,Copy,Debug,PartialEq,Eq)]\npub enum Pinning {\n/// One descriptor access, with the underlying view's identity policy.\nSingle,\n/// Multiple state accesses or a map scan use one immutable identity.\nPinned,}\n\
/// Catalog rendering shape extracted from the Python read registration.\n#[derive(Clone,Copy,Debug,PartialEq,Eq)]\npub enum RenderHint{\n/// Rows grouped by one record field.\nGrouped{\n/// Grouping field.\nkey:&'static str},\n/// Sparse rows indexed by a matrix's two coordinates.\nMatrix{\n/// Row label.\nrow:&'static str,\n/// Column label.\ncol:&'static str,\n/// Numeric value label.\nvalue:&'static str},}\n\
/// The fourteen public read categories.\n#[derive(Clone,Copy,Debug,PartialEq,Eq)]\npub enum Category {\n",
    );
    let categories: BTreeMap<_, _> = rtao_catalog::READS
        .iter()
        .map(|spec| (spec.namespace, spec.category))
        .collect();
    ensure!(categories.len() == 14, "expected14read categories");
    for (namespace, category) in &categories {
        out.push_str(&format!("#[doc={category:?}]\n{},\n", pascal(namespace)));
    }
    out.push_str("}\nimpl Category{\n/// Pinned Python category name.\npub const fn as_str(self)->&'static str{match self{\n");
    for (namespace, category) in &categories {
        out.push_str(&format!("Self::{}=>{category:?},\n", pascal(namespace)));
    }
    out.push_str("}}}\n\
/// Complete catalog, signature, rendering and coverage contract for one read.\n#[derive(Clone,Copy,Debug,PartialEq,Eq)]\npub struct ReadSpec{\n/// Stable read name.\npub name:&'static str,\n/// Catalog summary.\npub summary:&'static str,\n/// Full catalog description.\npub description:&'static str,\n/// Arguments in authenticated Python signature order.\npub params:&'static [ParamSpec],\n/// Public category.\npub category:Category,\n/// Namespace accessor.\npub namespace:&'static str,\n/// Upstream query documentation.\npub docs_url:&'static str,\n/// Source hits plus reviewed accesses and exact divergence-backed replacements.\npub hits:&'static [Hit],\n/// Optional presentation shape.\npub render:Option<RenderHint>,\n/// Required state identity policy.\npub pinning:Pinning,}\n/// Every read's complete authenticated metadata, in catalog order.\npub static READ_SPECS:[ReadSpec;102]=[\n");
    let mut replacement_source = Vec::new();
    for spec in rtao_catalog::READS {
        let schema: Value = serde_json::from_str(spec.catalog_json)?;
        let row = extracted
            .get(spec.name)
            .with_context(|| format!("missing read hit extraction {}", spec.name))?;
        let mut hits = row["hits"]
            .as_array()
            .context("read hit array")?
            .iter()
            .map(hit)
            .collect::<Result<BTreeSet<_>>>()?;
        if let Some(extra) = extras
            .get("read")
            .and_then(|v| v.get(spec.name))
            .and_then(|v| v.get("hits"))
            .and_then(|v| v.as_array())
        {
            for value in extra {
                let value = value
                    .as_inline_table()
                    .context("extra hit must be inline table")?;
                let field = |key: &str| {
                    value
                        .get(key)
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.trim().is_empty())
                        .with_context(|| format!("{} extra hit requires {key}", spec.name))
                };
                let _ = field("reason")?;
                let _ = field("python")?;
                hits.insert((
                    field("kind")?.into(),
                    field("container")?.into(),
                    field("name")?.into(),
                ));
            }
        }
        let catalog_hits = schema["hits"]
            .as_array()
            .into_iter()
            .flatten()
            .map(hit)
            .collect::<Result<BTreeSet<_>>>()?;
        let (effective_catalog, witnesses) = crate::read_hit_replacements::apply(
            root,
            spec.name,
            row,
            &catalog_hits,
            &mut hits,
            extras.get("read").and_then(|read| read.get(spec.name)),
        )?;
        ensure!(
            effective_catalog.is_subset(&hits),
            "{} catalog coverage missing after replacements",
            spec.name
        );
        for witness in witnesses {
            replacement_source.push(format!(
                "ReadHitReplacement {{read:{:?},divergence:{:?},from:{},to:{}}}",
                spec.name,
                witness.divergence,
                render_hit(&witness.from)?,
                render_hit(&witness.to)?,
            ));
        }
        let hits_source = hits
            .iter()
            .map(|(kind, container, name)| -> Result<String> {
                let kind = match kind.as_str() {
                    "storage" => "Storage",
                    "runtime" => "RuntimeApi",
                    "constant" => "Constant",
                    _ => bail!("unknown hit kind {kind}"),
                };
                Ok(format!(
                    "Hit{{kind:HitKind::{kind},container:{container:?},name:{name:?}}}"
                ))
            })
            .collect::<Result<Vec<_>>>()?
            .join(",");
        let params=spec.params.iter().map(|field|->Result<String>{let ty=match field.json_type.context("read JSON kind")?{"integer"=>"Integer","number"=>"Number","string"=>"String","boolean"=>"Boolean","array"=>"Array",kind=>bail!("unknown JSON kind {kind}")};let doc=schema["param_docs"][field.name].as_str().unwrap_or(field.name);Ok(format!("ParamSpec{{name:{:?},json_type:JsonType::{ty},doc:{doc:?},required:{},default:{:?},own_key:{}}}",field.name,field.required,field.default_json,field.own_key))}).collect::<Result<Vec<_>>>()?.join(",");
        let render = if row["render"].is_null() {
            "None".into()
        } else {
            let labels = row["render"]["labels"]
                .as_array()
                .context("render labels")?;
            match row["render"]["kind"].as_str() {
                Some("Grouped") => {
                    ensure!(labels.len() == 1, "Grouped label count");
                    format!(
                        "Some(RenderHint::Grouped{{key:{:?}}})",
                        labels[0].as_str().context("group label")?
                    )
                }
                Some("Matrix") => {
                    ensure!(labels.len() == 3, "Matrix label count");
                    format!(
                        "Some(RenderHint::Matrix{{row:{:?},col:{:?},value:{:?}}})",
                        labels[0].as_str().context("row label")?,
                        labels[1].as_str().context("column label")?,
                        labels[2].as_str().context("value label")?
                    )
                }
                _ => bail!("unknown render kind"),
            }
        };
        let extra_read = extras.get("read").and_then(|v| v.get(spec.name));
        let reviewed_pinning = extra_read
            .and_then(|v| v.get("pinning"))
            .and_then(|v| v.as_str());
        if let Some(pinning) = reviewed_pinning {
            ensure!(
                pinning == "pinned",
                "reviewed pinning cannot weaken the source policy"
            );
            for key in ["pinning_reason", "pinning_python"] {
                ensure!(
                    extra_read
                        .and_then(|v| v.get(key))
                        .and_then(|v| v.as_str())
                        .is_some_and(|v| !v.trim().is_empty()),
                    "reviewed pinning needs {key}"
                );
            }
        }
        let pinning = if row["pinning"].as_str() == Some("pinned")
            || hits.len() > 1
            || reviewed_pinning.is_some()
        {
            "Pinned"
        } else {
            "Single"
        };
        let docs = schema["docs_url"].as_str().context("read docs URL")?;
        out.push_str(&format!("ReadSpec{{name:{:?},summary:{:?},description:{:?},params:&[{params}],category:Category::{},namespace:{:?},docs_url:{docs:?},hits:&[{hits_source}],render:{render},pinning:Pinning::{pinning}}},\n",spec.name,spec.summary,spec.description,pascal(spec.namespace),spec.namespace));
    }
    out.push_str("];\n");
    out.push_str("/// One exact descriptor correction admitted by the active divergence checks.\n#[derive(Clone,Copy,Debug,PartialEq,Eq)]\npub struct ReadHitReplacement {\n/// Read whose effective coverage is corrected.\npub read:&'static str,\n/// Active divergence authorizing this exact mapping.\npub divergence:&'static str,\n/// Authenticated upstream descriptor retained as source evidence.\npub from:Hit,\n/// Corrected descriptor in the effective hit set.\npub to:Hit,}\n/// Complete validated descriptor replacement witnesses.\npub static READ_HIT_REPLACEMENTS:&[ReadHitReplacement]=&[\n");
    out.push_str(&replacement_source.join(",\n"));
    out.push_str("];\n");
    if let Some(reads) = extras.get("read").and_then(|v| v.as_table_like()) {
        for (name, _) in reads.iter() {
            ensure!(
                extracted.contains_key(name),
                "unknown reviewed extra read {name}"
            );
        }
    }
    Ok(out)
}

/// Regenerate every read interface from the authenticated catalog and parameters.
pub(super) fn run(root: &Path, check: bool) -> Result<()> {
    let config: DocumentMut = fs::read_to_string(root.join("codegen/read_types.toml"))?.parse()?;
    let pending: DocumentMut = fs::read_to_string(root.join("parity/pending.toml"))?.parse()?;
    let pending: BTreeSet<_> = pending
        .get("reads")
        .and_then(|v| v.as_array())
        .context("pending reads array")?
        .iter()
        .map(|v| v.as_str().map(str::to_owned).context("pending read string"))
        .collect::<Result<_>>()?;
    let known: BTreeSet<_> = rtao_catalog::READS
        .iter()
        .map(|s| s.name.to_owned())
        .collect();
    ensure!(pending.is_subset(&known), "unknown pending read");
    let mut names = String::from(
        "//! Complete pinned catalog of read names.\n\
/// Actual generated interface types and implementation availability for a read.\n\
#[derive(Clone,Copy,Debug,PartialEq,Eq)]\npub struct ReadInterfaceMetadata {\n\
/// Whether dispatch has real behavior rather than a pending stub.\npub implemented:bool,\n\
/// Actual generated parameter record type.\npub params_type_name:&'static str,\n\
/// Actual generated read output type.\npub output_type_name:&'static str,\n}\n\
/// A read selected from the pinned Python catalog.\n#[derive(Clone,Copy,Debug,PartialEq,Eq,PartialOrd,Ord,Hash)]\npub enum ReadName {\n",
    );
    let mut availability = String::new();
    let mut type_metadata = String::new();
    let mut interfaces = String::from("//! Typed inputs and entry points for all catalog reads.\n");
    let mut defaults =
        String::from("//! Defaults extracted from authenticated Python signatures.\n");
    let mut namespace_methods =
        String::from("//! Catalog methods on the generated namespace accessors.\n");
    let mut blocking_namespaces =
        String::from("//! Blocking namespace adapters over the asynchronous catalog reads.\n");
    for namespace in rtao_catalog::READS
        .iter()
        .map(|spec| spec.namespace)
        .collect::<BTreeSet<_>>()
    {
        let class = format!("Sync{}", pascal(namespace));
        let accessor = super::emit::ident(namespace);
        blocking_namespaces.push_str(&format!(
            "/// Blocking reads in the `{namespace}` namespace.\n#[derive(Clone,Copy)]\npub struct {class}<'a>{{pub(super) view:&'a super::SyncView}}\n"
        ));
        for owner in ["SyncClient", "SyncSnapshot"] {
            blocking_namespaces.push_str(&format!(
                "impl super::{owner} {{\n/// Bind the `{namespace}` namespace without connecting.\npub fn {accessor}(&self) -> {class}<'_> {{ {class} {{ view: &self.view }} }}\n}}\n"
            ));
        }
    }
    let mut dispatch = String::from(
        "//! Exhaustive catalog dispatch with extracted signature defaults.\nuse rtao_client::DynView;\nuse serde_json::{Map,Value};\nuse crate::{ReadError,ReadName};\n/// Fill only absent fields with authenticated Python signature defaults.\npub fn fill_defaults(name:ReadName,params:&Map<String,Value>)->Result<Map<String,Value>,ReadError>{\nlet mut params=params.clone();\nfor field in name.catalog_spec().params {if !params.contains_key(field.name) && let Some(default)=field.default_json {params.insert(field.name.into(),serde_json::from_str(default).map_err(|error| ReadError::SurfaceChanged{descriptor:\"read default\",detail:error.to_string()})?);}}\nOk(params)\n}\n/// Run one catalog read with exact extracted defaults and typed validation.\npub async fn dispatch(view:&dyn DynView,name:ReadName,params:&Map<String,Value>)->Result<Value,ReadError>{\nlet params=fill_defaults(name,params)?;\nmatch name {\n",
    );
    for spec in &rtao_catalog::READS {
        let class = pascal(spec.name);
        names.push_str(&format!("#[doc={:?}]\n{class},\n", spec.summary));
        let output = setting(&config, "outputs", spec.name).unwrap_or("serde_json::Value");
        let implementation = setting(&config, "implementations", spec.name);
        let scalar = setting(&config, "scalars", spec.name);
        ensure!(
            implementation.is_none() || scalar.is_none(),
            "read has two implementations: {}",
            spec.name
        );
        ensure!(
            pending.contains(spec.name) == (implementation.is_none() && scalar.is_none()),
            "read implementation/pending mismatch: {}",
            spec.name
        );
        let implemented = implementation.is_some() || scalar.is_some();
        availability.push_str(&format!("Self::{class}=>{implemented},\n"));
        type_metadata.push_str(&format!("Self::{class}=>ReadInterfaceMetadata{{implemented:{implemented},params_type_name:std::any::type_name::<super::interfaces::{class}Params>(),output_type_name:std::any::type_name::<{output}>()}},\n"));
        interfaces.push_str(&format!("#[doc={:?}]\n#[derive(Clone,Debug,PartialEq,serde::Serialize,serde::Deserialize)]\n#[serde(deny_unknown_fields)]\npub struct {class}Params {{\n",format!("Typed parameters for `{}`.",spec.name)));
        defaults.push_str(&format!(
            "/// Extracted defaults for `{}`.\npub mod {} {{\n",
            spec.name, spec.name
        ));
        let schema: Value = serde_json::from_str(spec.catalog_json)?;
        let mut arguments = Vec::new();
        let mut call_args = Vec::new();
        for field in spec.params {
            let ty = param_type(spec, field, &config)?;
            let ident = super::emit::ident(field.name);
            let doc = schema["param_docs"][field.name]
                .as_str()
                .unwrap_or(field.name);
            interfaces.push_str(&format!("#[doc={doc:?}]\n"));
            if let Some(default) = field.default_json {
                let expression = default_expr(&ty, &serde_json::from_str(default)?)?;
                interfaces.push_str(&format!(
                    "#[serde(default={:?})]\n",
                    format!("super::defaults::{}::{ident}", spec.name)
                ));
                defaults.push_str(&format!(
                    "/// Authenticated default for `{}`.\npub fn {ident}()->{ty}{{{expression}}}\n",
                    field.name
                ));
            }
            interfaces.push_str(&format!("pub {ident}:{ty},\n"));
            arguments.push(format!("{ident}:{ty}"));
            call_args.push(ident);
        }
        interfaces.push_str("}\n");
        defaults.push_str("}\n");
        let args = arguments.join(",");
        let separator = if args.is_empty() { "" } else { "," };
        let implementation_calls = spec.params.iter().zip(&call_args).map(|(field,arg)| -> Result<String> {
            let ty=param_type(spec,field,&config)?;
            Ok(if field.required && ty.starts_with("Option<") {format!("{arg}.ok_or_else(||crate::ReadError::InvalidParam{{name:{:?}.into(),reason:\"required chain key is null\".into()}})?",field.name)}else{arg.clone()})
        }).collect::<Result<Vec<_>>>()?.join(",");
        let expr = if let Some(descriptor) = scalar {
            ensure!(
                spec.params.len() == 1 && spec.params[0].name == "netuid",
                "scalar signature requires explicit reviewed keys"
            );
            format!(
                "Ok(rtao_client::ViewExt::storage(view,&{descriptor},({implementation_calls},)).await?)"
            )
        } else {
            implementation.map_or_else(
                || format!("Err(crate::ReadError::NotYetPorted(super::names::ReadName::{class}))"),
                |implementation| {
                    format!("{implementation}(view{separator}{implementation_calls}).await")
                },
            )
        };
        // Pending implementations deliberately expose the typed signature while doing no I/O.
        let (view_name, args) = if pending.contains(spec.name) {
            (
                "_view",
                arguments
                    .iter()
                    .map(|arg| format!("_{arg}"))
                    .collect::<Vec<_>>()
                    .join(","),
            )
        } else {
            ("view", args)
        };
        interfaces.push_str(&format!("#[doc={:?}]\n#[doc(alias={:?})]\npub async fn {}<V:rtao_client::View+?Sized>({view_name}:&V{separator}{args})->Result<{output},crate::ReadError>{{{expr}}}\n",spec.description,format!("bittensor.reads.{}",spec.name),super::emit::ident(spec.name)));
        dispatch.push_str(&format!("ReadName::{class}=>{{let args:super::interfaces::{class}Params=serde_json::from_value(Value::Object(params)).map_err(|error|ReadError::InvalidParam{{name:name.as_str().into(),reason:error.to_string()}})?;let _=&args;let value=super::interfaces::{}(view{}{}).await?;serde_json::to_value(value).map_err(|error|ReadError::SurfaceChanged{{descriptor:\"read result\",detail:error.to_string()}})}},\n",super::emit::ident(spec.name),if call_args.is_empty(){""}else{","},call_args.iter().map(|name|format!("args.{name}")).collect::<Vec<_>>().join(",")));
        if schema
            .get("python")
            .and_then(Value::as_str)
            .is_some_and(|expr| !expr.is_empty())
        {
            let namespace = pascal(spec.namespace);
            let namespace_args = spec
                .params
                .iter()
                .zip(&call_args)
                .map(|(field, arg)| -> Result<(String, String)> {
                    let ty = param_type(spec, field, &config)?;
                    if let Some(namespace_ty) = setting(
                        &config,
                        "namespace_fields",
                        &format!("{}.{}", spec.name, field.name),
                    ) {
                        return Ok((format!("{arg}:{namespace_ty}"), format!("{arg}.into()")));
                    }
                    Ok(if field.required && ty.starts_with("Option<") {
                        (
                            format!("{arg}:{}", &ty[7..ty.len() - 1]),
                            format!("Some({arg})"),
                        )
                    } else {
                        (format!("{arg}:{ty}"), arg.clone())
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            namespace_methods.push_str(&format!("impl<V:rtao_client::View+?Sized> super::namespaces::{namespace}<'_,V>{{#[doc={:?}]\npub async fn {}(&self{separator}{})->Result<{output},crate::ReadError>{{super::interfaces::{}(self.view{separator}{}).await}}}}\n",spec.description,super::emit::ident(spec.name),namespace_args.iter().map(|(arg,_)|arg.as_str()).collect::<Vec<_>>().join(","),super::emit::ident(spec.name),namespace_args.iter().map(|(_,arg)|arg.as_str()).collect::<Vec<_>>().join(",")));
            let blocking_output = output.replace("crate::", "rtao_reads::");
            let blocking_args = namespace_args
                .iter()
                .map(|(arg, _)| arg.replace("crate::", "rtao_reads::"))
                .collect::<Vec<_>>()
                .join(",");
            let forwarded = namespace_args
                .iter()
                .map(|(_, arg)| arg.as_str())
                .collect::<Vec<_>>()
                .join(",");
            let method = super::emit::ident(spec.name);
            blocking_namespaces.push_str(&format!(
                "impl Sync{namespace}<'_>{{#[doc={:?}]\npub fn {method}(&self{separator}{blocking_args})->Result<{blocking_output},super::Error>{{self.view.with_view(move |view|async move{{rtao_reads::generated::interfaces::{method}(&view{separator}{forwarded}).await}})}}}}\n",
                spec.description,
            ));
        }
    }
    names.push_str("}\nimpl ReadName {\n/// Every catalog read in stable name order.\npub const ALL:[Self;102]=[\n");
    for spec in rtao_catalog::READS {
        names.push_str(&format!("Self::{},\n", pascal(spec.name)));
    }
    names.push_str("];\n/// Stable Python catalog name.\npub const fn as_str(self)->&'static str {match self {\n");
    for spec in rtao_catalog::READS {
        names.push_str(&format!("Self::{}=>{:?},\n", pascal(spec.name), spec.name));
    }
    names.push_str("}}\n/// Authenticated catalog and signature metadata.\npub const fn catalog_spec(self)->&'static rtao_catalog::ReadSpec {match self {\n");
    for (index, spec) in rtao_catalog::READS.iter().enumerate() {
        names.push_str(&format!(
            "Self::{}=>&rtao_catalog::READS[{index}],\n",
            pascal(spec.name)
        ));
    }
    names.push_str("}}\n/// Complete descriptor, presentation and signature contract.\npub const fn spec(self)->&'static super::specs::ReadSpec {match self {\n");
    for (index, spec) in rtao_catalog::READS.iter().enumerate() {
        names.push_str(&format!(
            "Self::{}=>&super::specs::READ_SPECS[{index}],\n",
            pascal(spec.name)
        ));
    }
    names.push_str("}}\n/// Whether this read's actual generated dispatch has implemented behavior.\npub const fn implemented(self)->bool{match self{\n");
    names.push_str(&availability);
    names.push_str("}}\n/// Actual generated input/output types and behavior availability.\npub fn interface_metadata(self)->ReadInterfaceMetadata{match self{\n");
    names.push_str(&type_metadata);
    names.push_str("}}\n}\nimpl std::fmt::Display for ReadName{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.write_str(self.as_str())}}\nimpl std::str::FromStr for ReadName{type Err=crate::ReadError;fn from_str(name:&str)->Result<Self,Self::Err>{match name{\n");
    for spec in rtao_catalog::READS {
        names.push_str(&format!(
            "{:?}=>Ok(Self::{}),\n",
            spec.name,
            pascal(spec.name)
        ));
    }
    names.push_str("_=>Err(crate::ReadError::InvalidParam{name:\"read\".into(),reason:format!(\"unknown read: {name}\")}),}}}\n");
    dispatch.push_str("}}\n");
    let modules = "//! Complete catalog-derived read interfaces.\n/// Client and snapshot namespace accessors.\npub mod namespaces;\n/// Catalog names and stable metadata identities.\npub mod names;\n/// Typed parameters and behavior entry points.\npub mod interfaces;\n/// Authenticated signature defaults.\npub mod defaults;\n/// Complete rendering, pinning and descriptor coverage contracts.\npub mod specs;\nmod namespace_methods;\n/// Dynamic dispatch and default filling.\npub mod dispatch;\npub use names::{ReadInterfaceMetadata,ReadName};\npub use dispatch::{dispatch,fill_defaults};\npub use specs::{Category,Hit,HitKind,JsonType,ParamSpec,Pinning,ReadSpec,RenderHint,ReadHitReplacement,READ_HIT_REPLACEMENTS,READ_SPECS};\n";
    let dir = root.join("crates/rtao-reads/src/generated");
    if !check {
        fs::create_dir_all(&dir)?;
    }
    for (name, body) in [
        ("names", names),
        ("interfaces", interfaces),
        ("defaults", defaults),
        ("namespace_methods", namespace_methods),
        ("specs", specifications(root)?),
        ("dispatch", dispatch),
        ("mod", modules.to_owned()),
    ] {
        let output = format!(
            "// @generated by cargo xtask regen; do not edit.\n{}",
            super::format_rust(&body)?
        );
        let path = dir.join(format!("{name}.rs"));
        if check {
            ensure!(
                fs::read_to_string(&path).ok().as_deref() == Some(&output),
                "generated read drift: {name}"
            );
        } else {
            crate::pin::atomic_write(&path, output.as_bytes())?;
        }
    }
    let blocking_path = root.join("crates/rtao/src/generated/blocking_namespaces.rs");
    let blocking_output = format!(
        "// @generated by cargo xtask regen; do not edit.\n{}",
        super::format_rust(&blocking_namespaces)?
    );
    if check {
        ensure!(
            fs::read_to_string(&blocking_path).ok().as_deref() == Some(&blocking_output),
            "generated blocking namespace drift"
        );
    } else {
        fs::create_dir_all(
            blocking_path
                .parent()
                .context("blocking output directory")?,
        )?;
        crate::pin::atomic_write(&blocking_path, blocking_output.as_bytes())?;
    }
    println!(
        "102 read interfaces and {} pending behaviors {}",
        pending.len(),
        if check { "match" } else { "regenerated" }
    );
    Ok(())
}
