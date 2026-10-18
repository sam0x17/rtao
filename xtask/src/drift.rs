//! Classify runtime metadata changes and record immutable, block-pinned inputs.

// Use the production runtime hasher, mapper and descriptor emitter.
use crate::regen::{emit, shape, types};

use anyhow::{Context, Result, bail, ensure};
use frame_metadata::{RuntimeMetadata, RuntimeMetadataPrefixed};
use parity_scale_codec::{DecodeAll, Encode};
use rtao_core::{
    ir::MetadataIr,
    runtime::{Runtime, TypeSpec},
};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use shape::{OuterEnums, ShapeHasher};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use toml_edit::{DocumentMut, Item, value};

const LIMIT: usize = 16 * 1024 * 1024;
const NETWORKS: [&str; 4] = ["finney", "testnet", "devnet", "localnet-main"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Class {
    Unchanged,
    Additive,
    Behavioural,
    Breaking,
}
impl Class {
    fn name(self) -> &'static str {
        match self {
            Self::Unchanged => "surface-unchanged",
            Self::Additive => "additive",
            Self::Behavioural => "behavioural",
            Self::Breaking => "breaking",
        }
    }
}
struct Surface {
    items: BTreeMap<String, Json>,
    extensions: Vec<Json>,
    versions: Json,
    system: Json,
    v15: bool,
}
struct Report {
    class: Class,
    changes: Vec<Json>,
}
impl Report {
    fn new() -> Self {
        Self {
            class: Class::Unchanged,
            changes: Vec::new(),
        }
    }
    fn change(&mut self, class: Class, key: &str, reason: &str, before: &Json, after: &Json) {
        self.class = self.class.max(class);
        self.changes.push(json!({"class":class.name(),"surface":key,"reason":reason,"before":before,"after":after}));
    }
    fn json(&self) -> Json {
        json!({"schema":1,"classification":self.class.name(),"changes":self.changes})
    }
    fn markdown(&self) -> String {
        let mut text = format!("Runtime metadata drift: **{}**\n\n", self.class.name());
        for change in &self.changes {
            text.push_str(&format!(
                "- `{}`: {} ({})\n",
                change["surface"].as_str().unwrap_or("?"),
                change["reason"].as_str().unwrap_or("?"),
                change["class"].as_str().unwrap_or("?")
            ));
        }
        if self.changes.is_empty() {
            text.push_str("No semantic metadata changes.\n");
        }
        text
    }
}
fn root() -> Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_owned)
        .context("workspace root missing")
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn unhex(text: &str) -> Result<Vec<u8>> {
    let text = text.strip_prefix("0x").context("hex has no 0x prefix")?;
    ensure!(
        text.len() % 2 == 0 && text.len() <= (LIMIT + 8) * 2,
        "hex length outside bound"
    );
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| Ok(u8::from_str_radix(std::str::from_utf8(pair)?, 16)?))
        .collect()
}
fn metadata(bytes: &[u8]) -> Result<RuntimeMetadataPrefixed> {
    ensure!(
        (5..=LIMIT).contains(&bytes.len()),
        "metadata length outside bound"
    );
    let value = RuntimeMetadataPrefixed::decode_all(&mut &*bytes)
        .context("metadata is invalid or has trailing bytes")?;
    ensure!(
        value.0 == frame_metadata::META_RESERVED && value.encode() == bytes,
        "metadata is not canonical"
    );
    Ok(value)
}
fn read_metadata(path: &Path) -> Result<Vec<u8>> {
    ensure!(
        fs::symlink_metadata(path)?.is_file(),
        "metadata path must be a regular file"
    );
    ensure!(
        fs::metadata(path)?.len() <= LIMIT as u64,
        "metadata exceeds size bound"
    );
    let bytes = fs::read(path)?;
    metadata(&bytes)?;
    Ok(bytes)
}
fn zero_sized(
    registry: &scale_info::PortableRegistry,
    id: u32,
    seen: &mut BTreeSet<u32>,
) -> Result<bool> {
    use scale_info::TypeDef;
    ensure!(seen.len() < 128, "extension type exceeds recursion limit");
    if !seen.insert(id) {
        return Ok(false);
    }
    let result = match &registry
        .resolve(id)
        .context("extension type absent")?
        .type_def
    {
        TypeDef::Composite(value) => value
            .fields
            .iter()
            .map(|field| zero_sized(registry, field.ty.id, seen))
            .collect::<Result<Vec<_>>>()?
            .iter()
            .all(|zero| *zero),
        TypeDef::Tuple(value) => value
            .fields
            .iter()
            .map(|field| zero_sized(registry, field.id, seen))
            .collect::<Result<Vec<_>>>()?
            .iter()
            .all(|zero| *zero),
        TypeDef::Array(value) => value.len == 0 || zero_sized(registry, value.type_param.id, seen)?,
        _ => false,
    };
    seen.remove(&id);
    Ok(result)
}
fn insert(items: &mut BTreeMap<String, Json>, key: String, value: Json) -> Result<()> {
    ensure!(
        items.insert(key.clone(), value).is_none(),
        "duplicate metadata surface {key}"
    );
    Ok(())
}
fn build(root: &Path, bytes: &[u8], v16: Option<&[u8]>) -> Result<Surface> {
    let parsed = metadata(bytes)?;
    let RuntimeMetadata::V15(m15) = parsed.1 else {
        return Ok(Surface {
            items: BTreeMap::new(),
            extensions: Vec::new(),
            versions: Json::Null,
            system: Json::Null,
            v15: false,
        });
    };
    let runtime = Runtime::parse(bytes, 0, 0, 42)?;
    let mut ir = MetadataIr::from_runtime(&runtime)?;
    let hasher = ShapeHasher::new(
        &m15.types,
        OuterEnums {
            call: m15.outer_enums.call_enum_ty.id,
            event: m15.outer_enums.event_enum_ty.id,
            error: m15.outer_enums.error_enum_ty.id,
        },
    );
    let hash = |id| -> Result<String> { Ok(hex(&hasher.exact(id)?.0)) };
    let mut deprecations = BTreeMap::new();
    let mut versions = json!({"v15":m15.extrinsic.version});
    if let Some(v16) = v16 {
        let RuntimeMetadata::V16(m16) = metadata(v16)?.1 else {
            bail!("V16 input is not V16")
        };
        ir.set_v16_api_versions(&m16)?;
        let h16 = ShapeHasher::new(
            &m16.types,
            OuterEnums {
                call: m16.outer_enums.call_enum_ty.id,
                event: m16.outer_enums.event_enum_ty.id,
                error: m16.outer_enums.error_enum_ty.id,
            },
        );
        let mut by_version = BTreeMap::new();
        for (version, indices) in &m16.extrinsic.transaction_extensions_by_version {
            let mut list = Vec::new();
            for index in indices {
                let extension = m16
                    .extrinsic
                    .transaction_extensions
                    .get(index.0 as usize)
                    .context("V16 extension index outside metadata")?;
                list.push(json!({"identifier":extension.identifier,"ty":hex(&h16.exact(extension.ty.id)?.0),"implicit":hex(&h16.exact(extension.implicit.id)?.0),"empty":zero_sized(&m16.types,extension.ty.id,&mut BTreeSet::new())? && zero_sized(&m16.types,extension.implicit.id,&mut BTreeSet::new())?}));
            }
            by_version.insert(version.to_string(), list);
        }
        versions["v16"] = json!(m16.extrinsic.versions);
        versions["extensions_by_version"] = json!(by_version);
        for pallet in &m16.pallets {
            if let Some(calls) = &pallet.calls {
                for (index, info) in &calls.deprecation_info.0 {
                    deprecations.insert((pallet.name.clone(), *index), format!("{info:?}"));
                }
            }
        }
    }
    let mut items = BTreeMap::new();
    let mut system = Json::Null;
    let mut pallet_indices = BTreeSet::new();
    for pallet in &ir.pallets {
        ensure!(
            pallet_indices.insert(pallet.index),
            "ambiguous pallet index"
        );
        for indices in [
            pallet
                .calls
                .iter()
                .map(|call| call.index)
                .collect::<Vec<_>>(),
            pallet.events.iter().map(|event| event.index).collect(),
            pallet.errors.iter().map(|error| error.index).collect(),
        ] {
            ensure!(
                indices.iter().collect::<BTreeSet<_>>().len() == indices.len(),
                "ambiguous call, event or error index in {}",
                pallet.name
            );
        }
        insert(
            &mut items,
            format!("pallet/{}", pallet.name),
            json!({"index":pallet.index}),
        )?;
        for call in &pallet.calls {
            let fields = call
                .args
                .iter()
                .map(|field| Ok(json!({"name":field.name,"shape":hash(field.type_id)?})))
                .collect::<Result<Vec<_>>>()?;
            insert(
                &mut items,
                format!("call/{}/{}", pallet.name, call.name),
                json!({"index":call.index,"fields":fields,"deprecated":deprecations.get(&(pallet.name.clone(),call.index))}),
            )?;
        }
        for event in &pallet.events {
            let fields = event
                .fields
                .iter()
                .map(|field| Ok(json!({"name":field.name,"shape":hash(field.type_id)?})))
                .collect::<Result<Vec<_>>>()?;
            insert(
                &mut items,
                format!("event/{}/{}", pallet.name, event.name),
                json!({"index":event.index,"fields":fields}),
            )?;
        }
        for error in &pallet.errors {
            insert(
                &mut items,
                format!("error/{}/{}", pallet.name, error.name),
                json!({"index":error.index}),
            )?;
        }
        for storage in &pallet.storage {
            let keys = storage
                .key_types
                .iter()
                .map(|id| hash(*id))
                .collect::<Result<Vec<_>>>()?;
            insert(
                &mut items,
                format!("storage/{}/{}", pallet.name, storage.name),
                json!({"prefix":storage.prefix,"hashers":storage.hashers,"keys":keys,"value":hash(storage.value_type)?,"modifier":storage.modifier,"default":hex(&storage.default_bytes)}),
            )?;
        }
        for constant in &pallet.constant_details {
            if pallet.name == "System" && constant.name == "Version" {
                system = rtao_core::codec::value::to_corpus_json(&runtime.decode_spec(
                    &TypeSpec::Id(constant.type_id),
                    &constant.value,
                    true,
                )?);
            } else {
                insert(
                    &mut items,
                    format!("constant/{}/{}", pallet.name, constant.name),
                    json!({"shape":hash(constant.type_id)?,"value":hex(&constant.value)}),
                )?;
            }
        }
    }
    ensure!(system.is_object(), "System.Version must decode to a record");
    for api in &ir.runtime_apis {
        insert(
            &mut items,
            format!("api/{}", api.name),
            json!({"version":api.version}),
        )?;
        for method in &api.method_signatures {
            let inputs = method
                .inputs
                .iter()
                .map(|field| Ok(json!({"name":field.name,"shape":hash(field.type_id)?})))
                .collect::<Result<Vec<_>>>()?;
            insert(
                &mut items,
                format!("method/{}/{}", api.name, method.name),
                json!({"inputs":inputs,"output":hash(method.output)?}),
            )?;
        }
    }
    let mut extensions = Vec::new();
    let mut extension_names = BTreeSet::new();
    for extension in &ir.extrinsic.signed_extensions {
        ensure!(
            extension_names.insert(&extension.identifier),
            "duplicate extension identifier"
        );
        extensions.push(json!({"identifier":extension.identifier,"ty":hash(extension.type_id)?,"implicit":hash(extension.additional_signed)?,"empty":zero_sized(&runtime.types,extension.type_id,&mut BTreeSet::new())? && zero_sized(&runtime.types,extension.additional_signed,&mut BTreeSet::new())?}));
    }
    let load = |name| -> Result<DocumentMut> { Ok(fs::read_to_string(root.join(name))?.parse()?) };
    let mapper = match types::TypeMapper::new(
        &runtime,
        &load("codegen/types.toml")?,
        &load("codegen/models.toml")?,
        &load("codegen/overrides.toml")?,
    ) {
        Ok(mapper) => {
            insert(
                &mut items,
                "model/configuration".to_owned(),
                json!({"compatible":true}),
            )?;
            mapper
        }
        Err(error) => {
            insert(
                &mut items,
                "model/configuration".to_owned(),
                json!({"incompatible":error.to_string()}),
            )?;
            return Ok(Surface {
                items,
                extensions,
                versions,
                system,
                v15: true,
            });
        }
    };
    // Missing or incompatible models stay in the report; a by-name field append
    // leaves the configured projection unchanged.
    for name in mapper.models.keys().chain(mapper.enums.keys()) {
        let matches = runtime
            .types
            .types
            .iter()
            .filter(|ty| {
                ty.ty.path.segments.join("::") == *name || ty.ty.path.segments.last() == Some(name)
            })
            .collect::<Vec<_>>();
        let projection = (|| -> Result<String> {
            ensure!(matches.len() == 1, "model must resolve once");
            let ty = matches[0];
            let projection = mapper
                .output_projection(ty.id)?
                .context("model projection absent")?;
            Ok(hex(&hasher.projection(ty.id, &projection)?.0))
        })();
        insert(
            &mut items,
            format!("model/{name}"),
            match projection {
                Ok(shape) => json!({"projection":shape}),
                Err(error) => json!({"incompatible":error.to_string()}),
            },
        )?;
    }
    Ok(Surface {
        items,
        extensions,
        versions,
        system,
        v15: true,
    })
}
fn extension_class(old: &[Json], new: &[Json]) -> Class {
    if old == new {
        return Class::Unchanged;
    }
    let old_ids = old
        .iter()
        .map(|extension| &extension["identifier"])
        .collect::<Vec<_>>();
    let retained = new
        .iter()
        .filter(|extension| old_ids.contains(&&extension["identifier"]))
        .cloned()
        .collect::<Vec<_>>();
    let empty = new
        .iter()
        .filter(|extension| !old_ids.contains(&&extension["identifier"]))
        .all(|extension| extension["empty"] == true);
    if retained == old && empty {
        Class::Additive
    } else {
        Class::Breaking
    }
}
fn compare(old: &Surface, new: &Surface) -> Report {
    let mut report = Report::new();
    if !new.v15 {
        report.change(
            Class::Breaking,
            "metadata/V15",
            "V15 is no longer offered",
            &json!(old.v15),
            &json!(new.v15),
        );
        return report;
    }
    for key in old
        .items
        .keys()
        .chain(new.items.keys())
        .collect::<BTreeSet<_>>()
    {
        let before = old.items.get(key).unwrap_or(&Json::Null);
        let after = new.items.get(key).unwrap_or(&Json::Null);
        if before == after {
            continue;
        }
        let kind = key.split('/').next().unwrap_or("");
        let (class, reason) = if before.is_null() {
            (Class::Additive, "added surface")
        } else if after.is_null() {
            (
                if matches!(kind, "pallet" | "call" | "error" | "storage") {
                    Class::Breaking
                } else {
                    Class::Behavioural
                },
                "removed surface",
            )
        } else if matches!(kind, "pallet" | "error")
            || (kind == "call"
                && (before["index"] != after["index"] || before["fields"] != after["fields"]))
        {
            (
                Class::Breaking,
                "index or signing argument contract changed",
            )
        } else {
            (Class::Behavioural, "metadata contract changed")
        };
        report.change(class, key, reason, before, after);
    }
    if old.extensions != new.extensions {
        report.change(
            extension_class(&old.extensions, &new.extensions),
            "extrinsic/extensions",
            "extension order or payload changed",
            &json!(old.extensions),
            &json!(new.extensions),
        );
    }
    if old.versions["v15"] != new.versions["v15"] || old.versions["v16"] != new.versions["v16"] {
        report.change(
            Class::Breaking,
            "extrinsic/versions",
            "advertised extrinsic versions changed",
            &old.versions,
            &new.versions,
        );
    }
    if old.versions["extensions_by_version"] != new.versions["extensions_by_version"] {
        let mut class = Class::Unchanged;
        let old_map = old.versions["extensions_by_version"].as_object();
        let new_map = new.versions["extensions_by_version"].as_object();
        if let (Some(old_map), Some(new_map)) = (old_map, new_map) {
            for version in old_map
                .keys()
                .chain(new_map.keys())
                .collect::<BTreeSet<_>>()
            {
                class = class.max(
                    match (
                        old_map.get(version).and_then(Json::as_array),
                        new_map.get(version).and_then(Json::as_array),
                    ) {
                        (Some(before), Some(after)) => extension_class(before, after),
                        _ => Class::Breaking,
                    },
                );
            }
        } else {
            class = Class::Breaking;
        }
        report.change(
            class,
            "extrinsic/extensions-by-version",
            "V16 extension order or payload mapping changed",
            &old.versions["extensions_by_version"],
            &new.versions["extensions_by_version"],
        );
    }
    if old.system != new.system {
        let breaking = old.system["spec_name"] != new.system["spec_name"]
            || new.system["spec_name"] != "node-subtensor"
            || old.system["transaction_version"] != new.system["transaction_version"];
        report.change(
            if breaking {
                Class::Breaking
            } else if old.system["apis"] != new.system["apis"] {
                Class::Behavioural
            } else {
                Class::Unchanged
            },
            "System.Version",
            if breaking {
                "spec name or transaction version changed"
            } else {
                "runtime version changed"
            },
            &old.system,
            &new.system,
        );
    }
    report
}
fn write_report(out: &Path, report: &Report) -> Result<()> {
    fs::create_dir_all(out)?;
    for (name, bytes) in [
        (
            "report.json",
            format!("{}\n", serde_json::to_string_pretty(&report.json())?),
        ),
        ("report.md", report.markdown()),
        ("class", format!("{}\n", report.class.name())),
    ] {
        fs::write(out.join(name), bytes)?;
    }
    Ok(())
}

fn second_opinion(old: &Path, new: &Path, out: &Path) -> Result<()> {
    let tool = std::env::var_os("RTAO_SUBXT").unwrap_or_else(|| "subxt".into());
    let result = Command::new(tool)
        .arg("diff")
        .arg(old)
        .arg(new)
        .output()
        .context("drift second opinion requires subxt-cli 0.51.1")?;
    fs::write(out.join("subxt-diff.txt"), &result.stdout)?;
    fs::write(out.join("subxt-diff.stderr.txt"), &result.stderr)?;
    ensure!(
        result.status.success(),
        "independent subxt diff failed; see subxt-diff.stderr.txt"
    );
    Ok(())
}

fn descriptor_preview(
    root: &Path,
    v15: &[u8],
    v16: &[u8],
    out: &Path,
) -> Result<BTreeMap<String, String>> {
    let RuntimeMetadata::V15(m15) = metadata(v15)?.1 else {
        bail!("preview requires V15")
    };
    let RuntimeMetadata::V16(m16) = metadata(v16)?.1 else {
        bail!("preview requires V16")
    };
    let runtime = Runtime::parse(v15, 0, 0, 42)?;
    let mut ir = MetadataIr::from_runtime(&runtime)?;
    ir.set_v16_api_versions(&m16)?;
    let load = |name| -> Result<DocumentMut> { Ok(fs::read_to_string(root.join(name))?.parse()?) };
    let mapper = types::TypeMapper::new(
        &runtime,
        &load("codegen/types.toml")?,
        &load("codegen/models.toml")?,
        &load("codegen/overrides.toml")?,
    )?;
    let scope = load("codegen/scope.toml")?;
    let mut names = BTreeSet::new();
    for key in ["covered_pallets", "extra_pallets"] {
        for name in scope[key]
            .as_array()
            .context("codegen scope array absent")?
        {
            names.insert(
                name.as_str()
                    .context("scope name is not a string")?
                    .to_owned(),
            );
        }
    }
    let mut deprecated = BTreeMap::new();
    for pallet in &m16.pallets {
        if let Some(calls) = &pallet.calls {
            for (index, info) in &calls.deprecation_info.0 {
                let note = match info {
                    frame_metadata::v16::VariantDeprecationInfo::DeprecatedWithoutNote => {
                        "Deprecated by runtime metadata".to_owned()
                    }
                    frame_metadata::v16::VariantDeprecationInfo::Deprecated { note, since } => {
                        format!(
                            "{note}{}",
                            since
                                .as_ref()
                                .map(|since| format!(" (since {since})"))
                                .unwrap_or_default()
                        )
                    }
                };
                deprecated.insert((pallet.name.clone(), *index), note);
            }
        }
    }
    let bodies = emit::generate(
        &runtime,
        &ir,
        &mapper,
        &names,
        OuterEnums {
            call: m15.outer_enums.call_enum_ty.id,
            event: m15.outer_enums.event_enum_ty.id,
            error: m15.outer_enums.error_enum_ty.id,
        },
        &deprecated,
    )?;
    fs::create_dir_all(out)?;
    let mut hashes = BTreeMap::new();
    for (name, body) in bodies {
        let mut formatter = Command::new("rustfmt")
            .args(["--edition", "2024", "--emit", "stdout"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        formatter
            .stdin
            .take()
            .context("rustfmt stdin absent")?
            .write_all(body.as_bytes())?;
        let result = formatter.wait_with_output()?;
        ensure!(
            result.status.success(),
            "descriptor preview is invalid Rust: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        hashes.insert(
            name.clone(),
            format!("{:x}", Sha256::digest(&result.stdout)),
        );
        fs::write(out.join(format!("{name}.rs")), result.stdout)?;
    }
    fs::write(
        out.join("sha256.json"),
        format!("{}\n", serde_json::to_string_pretty(&hashes)?),
    )?;
    Ok(hashes)
}

pub(super) fn run(args: Vec<String>) -> Result<()> {
    if args.first().is_some_and(|arg| arg == "diff") {
        return diff(&args[1..]);
    }
    live(&args)
}
fn diff(args: &[String]) -> Result<()> {
    ensure!(
        args.len() >= 2,
        "usage: cargo xtask drift diff old.v15.scale new.v15.scale [--old-v16 path] [--new-v16 path] [--out directory]"
    );
    let mut old16 = None;
    let mut new16 = None;
    let mut out = None;
    let mut i = 2;
    while i < args.len() {
        let next = args.get(i + 1).context("missing drift diff option value")?;
        match args[i].as_str() {
            "--old-v16" => old16 = Some(read_metadata(Path::new(next))?),
            "--new-v16" => new16 = Some(read_metadata(Path::new(next))?),
            "--out" => out = Some(PathBuf::from(next)),
            option => bail!("unknown drift diff option {option}"),
        };
        i += 2;
    }
    ensure!(
        old16.is_some() == new16.is_some(),
        "supply both V16 inputs or neither"
    );
    let root = root()?;
    let old = build(
        &root,
        &read_metadata(Path::new(&args[0]))?,
        old16.as_deref(),
    )?;
    let new = build(
        &root,
        &read_metadata(Path::new(&args[1]))?,
        new16.as_deref(),
    )?;
    let report = compare(&old, &new);
    if let Some(out) = out {
        write_report(&out, &report)?;
        second_opinion(Path::new(&args[0]), Path::new(&args[1]), &out)?;
    }
    println!("{}", serde_json::to_string_pretty(&report.json())?);
    Ok(())
}

fn rpc(endpoint: &str, method: &str, params: Json) -> Result<Json> {
    ensure!(
        endpoint.starts_with("https://")
            || endpoint.starts_with("http://127.0.0.1:")
            || endpoint.starts_with("http://localhost:"),
        "drift RPC requires HTTPS or loopback HTTP"
    );
    let mut child = Command::new("curl")
        .args([
            "--fail-with-body",
            "--silent",
            "--show-error",
            "--max-time",
            "90",
            "--max-filesize",
            "33554432",
            "--header",
            "Content-Type: application/json",
            "--data-binary",
            "@-",
            endpoint,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child.stdin.take().context("curl stdin absent")?.write_all(
        json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})
            .to_string()
            .as_bytes(),
    )?;
    let response = child.wait_with_output()?;
    ensure!(
        response.status.success(),
        "RPC {method} failed: {}",
        String::from_utf8_lossy(&response.stderr)
    );
    ensure!(
        response.stdout.len() <= 32 * 1024 * 1024,
        "RPC response exceeds size bound"
    );
    let body: Json = serde_json::from_slice(&response.stdout)?;
    ensure!(
        body.get("error").is_none(),
        "RPC {method}: {}",
        body["error"]
    );
    body.get("result").cloned().context("RPC result absent")
}
fn text<'a>(table: &'a dyn toml_edit::TableLike, field: &str) -> Result<&'a str> {
    table
        .get(field)
        .and_then(Item::as_str)
        .with_context(|| format!("metadata index {field} absent"))
}
fn hash_text(text: &str) -> bool {
    text.strip_prefix("0x").is_some_and(|text| {
        text.len() == 64
            && text
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}
fn index(root: &Path, network: &str) -> Result<DocumentMut> {
    let directory = root.join("vendor/metadata").join(network);
    let path = directory.join("index.toml");
    if !path.try_exists()? {
        let mut document = DocumentMut::new();
        document["schema"] = value(1);
        document["network"] = value(network);
        document["runtimes"] = Item::ArrayOfTables(toml_edit::ArrayOfTables::new());
        return Ok(document);
    }
    ensure!(
        fs::symlink_metadata(&path)?.is_file(),
        "index must be a regular file"
    );
    let document: DocumentMut = fs::read_to_string(path)?.parse()?;
    ensure!(
        document["schema"].as_integer() == Some(1) && document["network"].as_str() == Some(network),
        "index schema or network differs"
    );
    let mut identities = BTreeSet::new();
    for row in document["runtimes"]
        .as_array_of_tables()
        .context("index runtimes absent")?
    {
        let spec = u32::try_from(row["spec"].as_integer().context("index spec absent")?)?;
        ensure!(
            identities.insert((spec, text(row, "code_hash")?)),
            "duplicate runtime identity"
        );
        ensure!(
            hash_text(text(row, "code_hash")?)
                && hash_text(text(row, "genesis")?)
                && hash_text(text(row, "finalized_head")?),
            "invalid runtime identity hash"
        );
        for version in [15, 16] {
            let blob = row
                .get(&format!("v{version}"))
                .and_then(Item::as_table_like)
                .context("required metadata blob absent")?;
            let name = text(blob, "path")?;
            ensure!(
                name == format!("{spec}.v{version}.scale"),
                "metadata blob path is not canonical"
            );
            let bytes = read_metadata(&directory.join(name))?;
            ensure!(
                bytes[4] == version
                    && blob.get("bytes").and_then(Item::as_integer)
                        == i64::try_from(bytes.len()).ok(),
                "metadata length or version differs"
            );
            ensure!(
                text(blob, "sha256")? == format!("{:x}", Sha256::digest(&bytes)),
                "metadata SHA256 differs"
            );
        }
    }
    Ok(document)
}
fn recorded(document: &DocumentMut, spec: u32, code: &str) -> bool {
    document
        .get("runtimes")
        .and_then(Item::as_array_of_tables)
        .is_some_and(|rows| {
            rows.iter().any(|row| {
                row["spec"].as_integer() == Some(i64::from(spec))
                    && row["code_hash"].as_str() == Some(code)
            })
        })
}
fn pending_pr(network: &str, spec: u32, code: &str) -> Result<Option<String>> {
    let output = Command::new("gh")
        .args([
            "pr",
            "list",
            "--state",
            "open",
            "--json",
            "number,headRefName,headRefOid,url",
            "--limit",
            "100",
        ])
        .output()?;
    ensure!(output.status.success(), "cannot inspect open pin-bump PRs");
    let prs: Json = serde_json::from_slice(&output.stdout)?;
    for pr in prs.as_array().context("GitHub PR list is not array")? {
        let branch = pr["headRefName"].as_str().context("PR branch absent")?;
        if !branch.starts_with("bot/pin-")
            && !branch.starts_with("bot/drift-")
            && !branch.starts_with("bot/pin-bump")
        {
            continue;
        }
        let oid = pr["headRefOid"].as_str().context("PR head absent")?;
        ensure!(
            oid.len() == 40 && oid.bytes().all(|b| b.is_ascii_hexdigit()),
            "PR head is not immutable SHA"
        );
        let output = Command::new("gh").args(["api","--header","Accept: application/vnd.github.raw+json",&format!("repos/{{owner}}/{{repo}}/contents/vendor/metadata/{network}/index.toml?ref={oid}")]).output()?;
        // A pin-bump branch can predate that network's first baseline.
        if !output.status.success() {
            ensure!(
                String::from_utf8_lossy(&output.stderr).contains("404"),
                "could not fetch PR index"
            );
            continue;
        }
        let document: DocumentMut = String::from_utf8(output.stdout)?.parse()?;
        if recorded(&document, spec, code) {
            return Ok(Some(
                pr["url"].as_str().context("PR URL absent")?.to_owned(),
            ));
        }
    }
    Ok(None)
}
fn fetch_version(endpoint: &str, head: &str, version: u8) -> Result<Option<Vec<u8>>> {
    let response = rpc(
        endpoint,
        "state_call",
        json!([
            "Metadata_metadata_at_version",
            format!("0x{version:02x}000000"),
            head
        ]),
    )?;
    let envelope = unhex(
        response
            .as_str()
            .context("metadata RPC result is not hex")?,
    )?;
    let option = Option::<Vec<u8>>::decode_all(&mut &*envelope)?;
    ensure!(
        option.encode() == envelope,
        "metadata envelope not canonical"
    );
    let Some(raw) = option else {
        return Ok(None);
    };
    ensure!(
        Some(raw.clone()).encode() == envelope,
        "metadata envelope not canonical"
    );
    metadata(&raw)?;
    ensure!(raw[4] == version, "metadata version differs from request");
    Ok(Some(raw))
}
fn ordinary_parents(root: &Path, relative: &Path) -> Result<()> {
    let mut path = root.to_owned();
    for component in relative.components() {
        ensure!(
            matches!(component, std::path::Component::Normal(_)),
            "destination escapes repository"
        );
        path.push(component);
        if !path.try_exists()? {
            fs::create_dir(&path)?;
        }
        ensure!(
            fs::symlink_metadata(&path)?.is_dir(),
            "destination directory is not ordinary"
        );
    }
    Ok(())
}
fn run_checked(root: &Path, arguments: &[&str]) -> Result<()> {
    ensure!(
        Command::new("cargo")
            .args(arguments)
            .current_dir(root)
            .status()?
            .success(),
        "cargo {} failed",
        arguments.join(" ")
    );
    Ok(())
}
/// The default finalized-state endpoint for a drift network.
pub(super) fn public_endpoint(network: &str) -> &'static str {
    match network {
        "finney" => "https://entrypoint-finney.opentensor.ai:443",
        "testnet" => "https://test.finney.opentensor.ai:443",
        "devnet" => "https://dev.chain.opentensor.ai:443",
        _ => "http://127.0.0.1:9944",
    }
}

/// A live finalized runtime with both required metadata versions.
pub(super) struct Finalized {
    pub(super) genesis: String,
    pub(super) head: String,
    pub(super) spec: u32,
    pub(super) code_hash: String,
    pub(super) v15: Vec<u8>,
    pub(super) v16: Vec<u8>,
}

/// Read the finalized runtime identity and its V15/V16 metadata at one block.
pub(super) fn finalized(endpoint: &str) -> Result<Finalized> {
    let genesis = rpc(endpoint, "chain_getBlockHash", json!([0]))?
        .as_str()
        .context("genesis absent")?
        .to_owned();
    let head = rpc(endpoint, "chain_getFinalizedHead", json!([]))?
        .as_str()
        .context("finalized head absent")?
        .to_owned();
    let version = rpc(endpoint, "state_getRuntimeVersion", json!([head]))?;
    ensure!(
        version["specName"] == "node-subtensor",
        "finalized runtime spec_name differs from node-subtensor"
    );
    let spec = u32::try_from(
        version["specVersion"]
            .as_u64()
            .context("runtime spec absent")?,
    )?;
    let code_hash = rpc(
        endpoint,
        "state_getStorageHash",
        json!(["0x3a636f6465", head]),
    )?
    .as_str()
    .context(":code hash absent")?
    .to_owned();
    ensure!(
        hash_text(&genesis) && hash_text(&head) && hash_text(&code_hash),
        "runtime identity invalid"
    );
    let required = |version| {
        fetch_version(endpoint, &head, version)?
            .with_context(|| format!("finalized runtime no longer offers metadata V{version}"))
    };
    let v15 = required(15)?;
    let v16 = required(16)?;
    Ok(Finalized {
        genesis,
        head,
        spec,
        code_hash,
        v15,
        v16,
    })
}

/// Classify the surface change between two V15/V16 metadata pairs.
pub(super) fn classify(
    old15: &[u8],
    old16: &[u8],
    new15: &[u8],
    new16: &[u8],
) -> Result<&'static str> {
    let root = root()?;
    let old = build(&root, old15, Some(old16))?;
    let new = build(&root, new15, Some(new16))?;
    Ok(compare(&old, &new).class.name())
}

fn live(args: &[String]) -> Result<()> {
    let mut network = None;
    let mut endpoint = None;
    let mut out = None;
    let mut container = None;
    let mut apply = false;
    let mut check = false;
    let mut prs = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--apply" => apply = true,
            "--check" => check = true,
            "--check-open-prs" => prs = true,
            option @ ("--network" | "--endpoint" | "--out" | "--localnet-container") => {
                let value = args
                    .get(i + 1)
                    .context("drift option value absent")?
                    .clone();
                match option {
                    "--network" => network = Some(value),
                    "--endpoint" => endpoint = Some(value),
                    "--out" => out = Some(PathBuf::from(value)),
                    _ => container = Some(value),
                };
                i += 1;
            }
            option => bail!("unknown drift option {option}"),
        }
        i += 1;
    }
    let network = network.context("usage: cargo xtask drift --network finney|testnet|devnet|localnet-main [--out directory] [--apply] [--check] [--check-open-prs]")?;
    ensure!(
        NETWORKS.contains(&network.as_str()),
        "unknown drift network"
    );
    ensure!(
        !(apply && check),
        "--apply and --check are mutually exclusive"
    );
    let root = root()?;
    let pins: DocumentMut = fs::read_to_string(root.join("upstream.toml"))?.parse()?;
    let endpoint = endpoint.unwrap_or_else(|| public_endpoint(&network).to_owned());
    let out = out.unwrap_or_else(|| {
        std::env::var_os("RTAO_TEMP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("target"))
            .join("drift")
            .join(&network)
    });
    fs::create_dir_all(&out)?;
    for marker in ["changed", "pending-pr"] {
        let path = out.join(marker);
        if path.try_exists()? {
            ensure!(
                fs::symlink_metadata(&path)?.is_file(),
                "drift marker must be regular file"
            );
            fs::remove_file(path)?;
        }
    }
    let mut index = index(&root, &network)?;
    let image = if network == "localnet-main" {
        let name = container.context(
            "localnet-main requires --localnet-container proving the pinned :main image",
        )?;
        let digest = pins["localnet"]["main"]["digest"]
            .as_str()
            .context(":main image digest absent")?;
        let image = format!(
            "{}@{digest}",
            pins["localnet"]["image"]
                .as_str()
                .context("localnet image absent")?
        );
        let inspected = Command::new("docker").args(["inspect", &name]).output()?;
        ensure!(
            inspected.status.success(),
            "cannot inspect localnet container"
        );
        let inspected: Json = serde_json::from_slice(&inspected.stdout)?;
        ensure!(
            inspected[0]["Config"]["Image"] == image && inspected[0]["State"]["Running"] == true,
            "localnet container is not running the pinned :main image"
        );
        Some(image)
    } else {
        None
    };
    let genesis = rpc(&endpoint, "chain_getBlockHash", json!([0]))?
        .as_str()
        .context("genesis absent")?
        .to_owned();
    if network != "localnet-main" {
        ensure!(
            pins["networks"][&network]["genesis"].as_str() == Some(genesis.as_str()),
            "RPC network genesis differs from pin"
        );
    }
    let head = rpc(&endpoint, "chain_getFinalizedHead", json!([]))?
        .as_str()
        .context("finalized head absent")?
        .to_owned();
    let version = rpc(&endpoint, "state_getRuntimeVersion", json!([head]))?;
    let spec = u32::try_from(
        version["specVersion"]
            .as_u64()
            .context("runtime spec absent")?,
    )?;
    let code = rpc(
        &endpoint,
        "state_getStorageHash",
        json!(["0x3a636f6465", head]),
    )?
    .as_str()
    .context(":code hash absent")?
    .to_owned();
    ensure!(
        hash_text(&genesis) && hash_text(&head) && hash_text(&code),
        "runtime identity invalid"
    );
    let identity = json!({"network":network,"spec":spec,"code_hash":code,"genesis":genesis,"finalized_head":head,"endpoint":endpoint,"runtime_version":version,"source_image":image});
    ensure!(
        version["specName"] == "node-subtensor",
        "finalized runtime spec_name differs from node-subtensor"
    );
    fs::write(
        out.join("identity.json"),
        format!("{}\n", serde_json::to_string_pretty(&identity)?),
    )?;
    fs::write(out.join("spec"), format!("{spec}\n"))?;
    if recorded(&index, spec, &code) {
        write_report(&out, &Report::new())?;
        println!("{network} spec {spec} :code {code} is already indexed");
        return Ok(());
    }
    if prs && let Some(url) = pending_pr(&network, spec, &code)? {
        fs::write(out.join("pending-pr"), &url)?;
        write_report(&out, &Report::new())?;
        println!("runtime already recorded in {url}");
        return Ok(());
    }
    ensure!(
        !check,
        "{network} finalized runtime is absent from immutable index"
    );
    // Refuse to overwrite a spec, even when its :code changed. Such a runtime
    // needs an explicitly reviewed filename/index migration.
    if index["runtimes"]
        .as_array_of_tables()
        .context("index rows absent")?
        .iter()
        .any(|row| row["spec"].as_integer() == Some(i64::from(spec)))
    {
        let mut report = Report::new();
        report.change(
            Class::Breaking,
            "runtime/identity",
            "same spec has a different :code; immutable metadata cannot be overwritten",
            &Json::Null,
            &identity,
        );
        write_report(&out, &report)?;
        bail!("{}", report.markdown());
    }
    let required = |version| -> Result<Vec<u8>> {
        if let Some(raw) = fetch_version(&endpoint, &head, version)? {
            return Ok(raw);
        }
        let mut report = Report::new();
        report.change(
            Class::Breaking,
            &format!("metadata/V{version}"),
            "required metadata version is no longer offered",
            &json!(true),
            &json!(false),
        );
        write_report(&out, &report)?;
        bail!("{}", report.markdown())
    };
    let v15 = required(15)?;
    let v16 = required(16)?;
    fs::write(out.join("new.v15.scale"), &v15)?;
    fs::write(out.join("new.v16.scale"), &v16)?;
    let new = build(&root, &v15, Some(&v16))?;
    ensure!(
        new.system["spec_version"].as_u64() == Some(u64::from(spec))
            && new.system["transaction_version"].as_u64() == version["transactionVersion"].as_u64()
            && new.system["spec_name"].as_str() == version["specName"].as_str(),
        "metadata System.Version differs from finalized runtime identity"
    );
    let mut previous_inputs = None;
    let report = if let Some(previous) = index["runtimes"]
        .as_array_of_tables()
        .context("index rows absent")?
        .iter()
        .max_by_key(|row| row["spec"].as_integer())
    {
        let directory = root.join("vendor/metadata").join(&network);
        let read = |version| -> Result<Vec<u8>> {
            read_metadata(
                &directory.join(text(
                    previous
                        .get(&format!("v{version}"))
                        .and_then(Item::as_table_like)
                        .context("previous blob absent")?,
                    "path",
                )?),
            )
        };
        let previous15 = read(15)?;
        let previous16 = read(16)?;
        let previous_path = directory.join(text(
            previous
                .get("v15")
                .and_then(Item::as_table_like)
                .context("previous V15 blob absent")?,
            "path",
        )?);
        second_opinion(&previous_path, &out.join("new.v15.scale"), &out)?;
        let report = compare(&build(&root, &previous15, Some(&previous16))?, &new);
        previous_inputs = Some((previous15, previous16));
        report
    } else {
        let mut report = Report::new();
        fs::write(
            out.join("subxt-diff.txt"),
            "First immutable baseline; no prior snapshot exists for an independent diff.\n",
        )?;
        report.change(
            Class::Additive,
            "metadata/baseline",
            "first verified runtime baseline",
            &Json::Null,
            &identity,
        );
        report
    };
    write_report(&out, &report)?;
    let preview = descriptor_preview(&root, &v15, &v16, &out.join("descriptor-preview/new"))
        .inspect_err(|error| {
            let _ = fs::write(
                out.join("descriptor-preview-error.txt"),
                format!("{error:#}\n"),
            );
        })?;
    if let Some((previous15, previous16)) = previous_inputs {
        let previous_preview = descriptor_preview(
            &root,
            &previous15,
            &previous16,
            &out.join("descriptor-preview/old"),
        )?;
        if report.class == Class::Unchanged {
            ensure!(
                previous_preview == preview,
                "surface-unchanged drift regenerated changed descriptors"
            );
        }
    }
    fs::write(out.join("changed"), "")?;
    fs::write(
        out.join("title"),
        format!("Record {network} runtime {spec}: {}\n", report.class.name()),
    )?;
    if !apply {
        println!("{}", report.markdown());
        return Ok(());
    }
    // The network that proves the parity runtime feeds code generation; only a
    // completed pin may record or promote its runtime rows.
    let pinned = crate::metadata_pin::pinned_network(&pins)? == network;
    if pinned {
        ensure!(
            pins["parity"]["spec_version"].as_integer() == Some(i64::from(spec))
                && crate::metadata_pin::pinned_metadata(&pins)?["code_hash"].as_str()
                    == Some(code.as_str()),
            "verified {network} preview retained; complete cargo xtask pin --parity v{spec} before recording or promoting code generation"
        );
    }
    let directory = PathBuf::from("vendor/metadata").join(&network);
    ordinary_parents(&root, &directory)?;
    let mut row = toml_edit::Table::new();
    row["spec"] = value(i64::from(spec));
    if pinned {
        row["tag"] = value(
            pins["parity"]["tag"]
                .as_str()
                .context("pinned parity tag missing")?,
        );
        row["commit"] = value(
            pins["parity"]["commit"]
                .as_str()
                .context("pinned parity commit missing")?,
        );
    }
    for (field, text) in [
        ("code_hash", code.as_str()),
        ("genesis", genesis.as_str()),
        ("finalized_head", head.as_str()),
        ("endpoint", endpoint.as_str()),
    ] {
        row[field] = value(text);
    }
    if let Some(image) = image {
        row["source_image"] = value(image);
    }
    for (version, raw) in [(15, &v15), (16, &v16)] {
        let name = format!("{spec}.v{version}.scale");
        let destination = root.join(&directory).join(&name);
        if destination.try_exists()? {
            ensure!(
                read_metadata(&destination)? == *raw,
                "unpublished immutable blob differs"
            );
        } else {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(destination)?;
            file.write_all(raw)?;
            file.sync_all()?;
        }
        let mut blob = toml_edit::InlineTable::new();
        blob.insert("path", name.into());
        blob.insert("sha256", format!("{:x}", Sha256::digest(raw)).into());
        blob.insert("bytes", i64::try_from(raw.len())?.into());
        row[&format!("v{version}")] = Item::Value(toml_edit::Value::InlineTable(blob));
    }
    index["runtimes"]
        .as_array_of_tables_mut()
        .context("index rows absent")?
        .push(row);
    crate::pin::atomic_write(
        &root.join(&directory).join("index.toml"),
        format!("# Generated by cargo xtask drift; immutable block-pinned runtime rows.\n{index}")
            .as_bytes(),
    )?;
    // Live network metadata can be recorded before its source release is pinned.
    // Finney regeneration still requires the normal pin-bump provenance checks.
    run_checked(&root, &["run", "--locked", "-p", "xtask", "--", "regen"])?;
    run_checked(&root, &["test", "--locked", "-p", "xtask", "drift::tests"])?;
    println!("{network} spec {spec} recorded and offline checks passed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use scale_info::{Field, TypeDef, TypeDefPrimitive};
    fn fixture() -> Result<(PathBuf, Vec<u8>, Json)> {
        let root = root()?;
        let fixture: Json = serde_json::from_slice(&fs::read(
            root.join("vendor/metadata/fixtures/mutations.json"),
        )?)?;
        ensure!(fixture["schema"] == 1, "unsupported drift fixture schema");
        let bytes =
            read_metadata(&root.join(fixture["base"].as_str().context("fixture base absent")?))?;
        ensure!(
            format!("{:x}", Sha256::digest(&bytes))
                == fixture["base_sha256"]
                    .as_str()
                    .context("fixture hash absent")?,
            "drift fixture checksum differs"
        );
        Ok((root, bytes, fixture))
    }
    fn mutate(bytes: &[u8], fixture: &Json) -> Result<Vec<u8>> {
        let mut parsed = metadata(bytes)?;
        let RuntimeMetadata::V15(metadata) = &mut parsed.1 else {
            bail!("fixture must be V15")
        };
        let u32_id = metadata
            .types
            .types
            .iter()
            .find(|ty| matches!(&ty.ty.type_def, TypeDef::Primitive(TypeDefPrimitive::U32)))
            .context("fixture u32 absent")?
            .id;
        match fixture["mutation"]
            .as_str()
            .context("fixture mutation absent")?
        {
            "add-call" | "move-call" => {
                let pallet = metadata
                    .pallets
                    .iter()
                    .find(|pallet| Some(pallet.name.as_str()) == fixture["pallet"].as_str())
                    .context("fixture pallet absent")?;
                let id = pallet.calls.as_ref().context("fixture calls absent")?.ty.id;
                let TypeDef::Variant(calls) = &mut metadata
                    .types
                    .types
                    .iter_mut()
                    .find(|ty| ty.id == id)
                    .context("fixture calls type absent")?
                    .ty
                    .type_def
                else {
                    bail!("fixture calls not variants")
                };
                let unused = (0..=255)
                    .find(|index| calls.variants.iter().all(|call| call.index != *index))
                    .context("no unused call index")?;
                if fixture["mutation"] == "add-call" {
                    let mut call = calls
                        .variants
                        .first()
                        .context("fixture has no call")?
                        .clone();
                    call.name = "sdk_synthetic_additive".into();
                    call.index = unused;
                    calls.variants.push(call);
                } else {
                    calls
                        .variants
                        .first_mut()
                        .context("fixture has no call")?
                        .index = unused;
                }
            }
            "storage-default" => {
                let pallet = metadata
                    .pallets
                    .iter_mut()
                    .find(|pallet| Some(pallet.name.as_str()) == fixture["pallet"].as_str())
                    .context("fixture pallet absent")?;
                let entry = pallet
                    .storage
                    .as_mut()
                    .context("fixture storage absent")?
                    .entries
                    .iter_mut()
                    .find(|entry| !entry.default.is_empty())
                    .context("nonempty storage default absent")?;
                entry.default[0] ^= 1;
            }
            "extension-payload" => {
                let mut extension = metadata
                    .extrinsic
                    .signed_extensions
                    .first()
                    .context("fixture extensions absent")?
                    .clone();
                extension.identifier = "SdkNewPayloadExtension".into();
                extension.ty = u32_id.into();
                metadata.extrinsic.signed_extensions.push(extension);
            }
            "append-field" => {
                let ty = metadata
                    .types
                    .types
                    .iter_mut()
                    .find(|ty| {
                        ty.ty.path.segments.last().map(String::as_str) == fixture["model"].as_str()
                    })
                    .context("fixture model absent")?;
                let TypeDef::Composite(composite) = &mut ty.ty.type_def else {
                    bail!("fixture model not composite")
                };
                composite.fields.push(Field {
                    name: Some("sdk_appended_field".into()),
                    ty: u32_id.into(),
                    type_name: Some("u32".into()),
                    docs: vec![],
                });
            }
            mutation => bail!("unknown committed drift fixture {mutation}"),
        }
        Ok(parsed.encode())
    }
    #[test]
    fn committed_finney477_devnet478_surface_is_unchanged() -> Result<()> {
        let (root, old, _) = fixture()?;
        let old16 = read_metadata(&root.join("vendor/metadata/finney/477.v16.scale"))?;
        let new = read_metadata(&root.join("vendor/metadata/devnet/478.v15.scale"))?;
        let new16 = read_metadata(&root.join("vendor/metadata/devnet/478.v16.scale"))?;
        let report = compare(
            &build(&root, &old, Some(&old16))?,
            &build(&root, &new, Some(&new16))?,
        );
        ensure!(
            report.class == Class::Unchanged,
            "477→478: {}",
            report.markdown()
        );
        ensure!(
            report
                .changes
                .iter()
                .all(|change| change["surface"] == "System.Version"),
            "unexpected semantic change"
        );
        Ok(())
    }
    #[test]
    fn committed_synthetic_fixtures_cover_all_classes_and_opaque_wrappers() -> Result<()> {
        let (root, old, fixtures) = fixture()?;
        let original = build(&root, &old, None)?;
        for fixture in fixtures["fixtures"].as_array().context("fixtures absent")? {
            let mutated = mutate(&old, fixture)?;
            let changed = build(&root, &mutated, None)?;
            let report = compare(&original, &changed);
            ensure!(
                Some(report.class.name()) == fixture["class"].as_str(),
                "{}: {}",
                fixture["mutation"],
                report.markdown()
            );
            if fixture["mutation"] == "add-call" {
                ensure!(
                    report.changes.len() == 1
                        && report.changes[0]["surface"]
                            .as_str()
                            .is_some_and(|name| name
                                .starts_with("call/SubtensorModule/sdk_synthetic_additive")),
                    "new call altered opaque wrapper shapes: {}",
                    report.markdown()
                );
            }
            if fixture["mutation"] == "append-field" {
                ensure!(
                    original.items["model/Metagraph"] == changed.items["model/Metagraph"],
                    "appended model field changed by-name projection"
                );
                ensure!(
                    report.class != Class::Breaking,
                    "read model append is breaking"
                );
            }
        }
        Ok(())
    }
    #[test]
    fn spec478_appended_read_field_preserves_projection_and_is_not_breaking() -> Result<()> {
        let root = root()?;
        let old = read_metadata(&root.join("vendor/metadata/devnet/478.v15.scale"))?;
        let changed = mutate(
            &old,
            &json!({"mutation":"append-field","model":"Metagraph"}),
        )?;
        let old = build(&root, &old, None)?;
        let changed = build(&root, &changed, None)?;
        ensure!(
            old.items["model/Metagraph"] == changed.items["model/Metagraph"],
            "478 append changed projected fields"
        );
        ensure!(
            compare(&old, &changed).class == Class::Behavioural,
            "478 append became breaking"
        );
        Ok(())
    }
    #[test]
    fn unchanged_runtime_regenerates_identical_descriptor_lines() -> Result<()> {
        let root = root()?;
        let temporary = std::env::var_os("RTAO_TEMP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("target"));
        let stage = temporary
            .join("tmp")
            .join(format!("drift-preview-test-{}", std::process::id()));
        fs::create_dir(&stage)?;
        let result = (|| -> Result<()> {
            let inputs = |network: &str, spec: u32| -> Result<(Vec<u8>, Vec<u8>)> {
                Ok((
                    read_metadata(
                        &root.join(format!("vendor/metadata/{network}/{spec}.v15.scale")),
                    )?,
                    read_metadata(
                        &root.join(format!("vendor/metadata/{network}/{spec}.v16.scale")),
                    )?,
                ))
            };
            let (old15, old16) = inputs("finney", 477)?;
            let (new15, new16) = inputs("devnet", 478)?;
            let before = descriptor_preview(&root, &old15, &old16, &stage.join("old"))?;
            let after = descriptor_preview(&root, &new15, &new16, &stage.join("new"))?;
            ensure!(
                before == after,
                "477→478 regenerated changed descriptor lines"
            );
            Ok(())
        })();
        // rustfmt children have been reaped; only these test-owned regular
        // outputs exist. No configuration, symlink or environment points here.
        for directory in [stage.join("old"), stage.join("new")] {
            if !directory.try_exists()? {
                continue;
            }
            for file in fs::read_dir(&directory)? {
                let file = file?;
                ensure!(
                    file.file_type()?.is_file(),
                    "unexpected live preview dependency"
                );
                fs::remove_file(file.path())?;
            }
            fs::remove_dir(directory)?;
        }
        fs::remove_dir(stage)?;
        result
    }
    #[test]
    fn extension_reordering_and_shape_changes_break_but_empty_additions_do_not() {
        let make = |extensions| Surface {
            items: BTreeMap::new(),
            extensions,
            versions: json!({"v15":4}),
            system: json!({"spec_name":"node-subtensor","transaction_version":1}),
            v15: true,
        };
        let first = json!({"identifier":"one","ty":"shape","implicit":"unit","empty":true});
        let second = json!({"identifier":"two","ty":"shape","implicit":"unit","empty":true});
        let old = make(vec![first.clone(), second.clone()]);
        assert_eq!(
            compare(&old, &make(vec![second.clone(), first.clone()])).class,
            Class::Breaking
        );
        let mut changed = first.clone();
        changed["implicit"] = json!("new");
        assert_eq!(
            compare(&old, &make(vec![changed, second.clone()])).class,
            Class::Breaking
        );
        let empty = json!({"identifier":"three","ty":"unit","implicit":"unit","empty":true});
        assert_eq!(
            compare(&old, &make(vec![first, second, empty])).class,
            Class::Additive
        );
    }
    #[test]
    fn v16_extension_maps_detect_empty_additions_payloads_and_reordering() {
        let first = json!({"identifier":"first","ty":"unit","implicit":"unit","empty":true});
        let second = json!({"identifier":"second","ty":"unit","implicit":"unit","empty":true});
        let make = |extensions| Surface {
            items: BTreeMap::new(),
            extensions: vec![],
            versions: json!({"v15":4,"v16":[4,5],"extensions_by_version":{"4":extensions}}),
            system: json!({"spec_name":"node-subtensor","transaction_version":1}),
            v15: true,
        };
        let old = make(vec![first.clone()]);
        assert_eq!(
            compare(&old, &make(vec![first.clone(), second.clone()])).class,
            Class::Additive
        );
        let mut payload = second.clone();
        payload["empty"] = json!(false);
        payload["ty"] = json!("u32");
        assert_eq!(
            compare(&old, &make(vec![first.clone(), payload])).class,
            Class::Breaking
        );
        let old = make(vec![first.clone(), second.clone()]);
        assert_eq!(
            compare(&old, &make(vec![second, first])).class,
            Class::Breaking
        );
    }
    #[test]
    fn fixed_fraction_changes_are_visible_in_the_model_projection_report() -> Result<()> {
        let (root, old, _) = fixture()?;
        let mut changed = metadata(&old)?;
        let RuntimeMetadata::V15(metadata) = &mut changed.1 else {
            bail!("fixture must be V15")
        };
        let metagraph = metadata
            .types
            .types
            .iter()
            .find(|ty| {
                ty.ty
                    .path
                    .segments
                    .last()
                    .is_some_and(|last| last == "Metagraph")
            })
            .context("Metagraph absent")?;
        let TypeDef::Composite(composite) = &metagraph.ty.type_def else {
            bail!("Metagraph must be composite")
        };
        let fixed = composite
            .fields
            .iter()
            .find(|field| field.name.as_deref() == Some("moving_price"))
            .context("moving_price absent")?
            .ty
            .id;
        let existing = metadata
            .types
            .resolve(fixed)
            .context("fixed type absent")?
            .type_params[0]
            .ty
            .context("Frac absent")?
            .id;
        let different = metadata
            .types
            .types
            .iter()
            .find(|ty| {
                ty.ty.path.segments == ["substrate_fixed", "FixedI128"]
                    && ty.ty.type_params[0]
                        .ty
                        .is_some_and(|parameter| parameter.id != existing)
            })
            .context("second fixed fraction absent")?
            .ty
            .type_params[0]
            .ty
            .context("fraction absent")?;
        metadata
            .types
            .types
            .iter_mut()
            .find(|ty| ty.id == fixed)
            .context("fixed absent")?
            .ty
            .type_params[0]
            .ty = Some(different);
        let report = compare(
            &build(&root, &old, None)?,
            &build(&root, &changed.encode(), None)?,
        );
        ensure!(
            report
                .changes
                .iter()
                .any(|change| change["surface"] == "model/Metagraph"
                    && change["class"] == "behavioural"),
            "Frac change absent from model report: {}",
            report.markdown()
        );
        Ok(())
    }
    #[test]
    fn version_and_index_guards_are_breaking() {
        let make = || Surface {
            items: BTreeMap::new(),
            extensions: vec![],
            versions: json!({"v15":4,"v16":[4,5]}),
            system: json!({"spec_name":"node-subtensor","spec_version":477,"transaction_version":1}),
            v15: true,
        };
        let old = make();
        let mut new = make();
        new.versions["v16"] = json!([5]);
        assert_eq!(compare(&old, &new).class, Class::Breaking);
        let mut new = make();
        new.versions["v15"] = json!(5);
        assert_eq!(compare(&old, &new).class, Class::Breaking);
        let mut new = make();
        new.system["transaction_version"] = json!(2);
        assert_eq!(compare(&old, &new).class, Class::Breaking);
        let mut new = make();
        new.system["spec_name"] = json!("other");
        assert_eq!(compare(&old, &new).class, Class::Breaking);
        let mut new = make();
        new.v15 = false;
        assert_eq!(compare(&old, &new).class, Class::Breaking);
        let mut old = make();
        old.items
            .insert("event/System/Event".into(), json!({"index":1}));
        let mut new = make();
        new.items
            .insert("event/System/Event".into(), json!({"index":2}));
        assert_eq!(compare(&old, &new).class, Class::Behavioural);
        for kind in ["pallet", "call", "error"] {
            let mut old = make();
            old.items
                .insert(format!("{kind}/Example"), json!({"index":1}));
            let mut new = make();
            new.items
                .insert(format!("{kind}/Example"), json!({"index":2}));
            assert_eq!(compare(&old, &new).class, Class::Breaking);
        }
    }
    #[test]
    fn runtime_index_identity_requires_spec_and_code_pair() -> Result<()> {
        let document: DocumentMut = "[[runtimes]]\nspec = 478\ncode_hash = '0xabc'\n".parse()?;
        assert!(recorded(&document, 478, "0xabc"));
        assert!(!recorded(&document, 477, "0xabc"));
        assert!(!recorded(&document, 478, "0xdef"));
        Ok(())
    }
    #[test]
    fn every_available_network_index_is_verified_offline() -> Result<()> {
        let root = root()?;
        for network in ["finney", "testnet", "devnet"] {
            let rows = index(&root, network)?;
            ensure!(
                !rows["runtimes"]
                    .as_array_of_tables()
                    .context("rows absent")?
                    .is_empty(),
                "network baseline missing"
            );
        }
        Ok(())
    }
    #[test]
    fn noncanonical_and_wrong_metadata_are_rejected() -> Result<()> {
        let (_, bytes, _) = fixture()?;
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(metadata(&trailing).is_err());
        assert!(unhex("0x123").is_err());
        assert!(unhex("0xzz").is_err());
        assert!(unhex("00").is_err());
        Ok(())
    }
}
