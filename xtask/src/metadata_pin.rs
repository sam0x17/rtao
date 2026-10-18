//! Immutable, finalized metadata inputs for code generation, recorded from the
//! network that proves the parity runtime (Finney for a published release).

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use frame_metadata::{META_RESERVED, RuntimeMetadata, RuntimeMetadataPrefixed};
use parity_scale_codec::{DecodeAll, Encode};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use toml_edit::{ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value, value};

const MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Blob {
    path: String,
    sha256: String,
    bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    spec: u32,
    code_hash: String,
    genesis: String,
    tag: String,
    commit: String,
    finalized_head: String,
    endpoint: String,
    blobs: [Blob; 2],
}

/// The network whose metadata feeds code generation, with its index location.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Network {
    name: String,
    genesis: String,
}

impl Network {
    fn from_pins(pins: &DocumentMut) -> Result<Self> {
        let name = pinned_network(pins)?;
        let genesis = pins
            .get("networks")
            .and_then(|i| i.get(name))
            .and_then(|i| i.get("genesis"))
            .and_then(Item::as_str)
            .with_context(|| format!("[networks.{name}] genesis is missing"))?;
        Ok(Self {
            name: name.to_owned(),
            genesis: genesis.to_owned(),
        })
    }

    fn directory(&self) -> String {
        format!("vendor/metadata/{}", self.name)
    }

    fn index(&self) -> String {
        format!("vendor/metadata/{}/index.toml", self.name)
    }
}

/// The network whose finalized runtime the parity pin was proven on.
pub(crate) fn pinned_network(pins: &DocumentMut) -> Result<&str> {
    let name = pins
        .get("parity")
        .and_then(|i| i.get("network"))
        .map_or(Some("finney"), Item::as_str)
        .context("parity.network must be a string")?;
    ensure!(
        ["finney", "testnet", "devnet"].contains(&name),
        "unsupported pinned metadata network {name}"
    );
    Ok(name)
}

/// The `[networks.<pinned>.metadata]` table that selects code-generation inputs.
pub(crate) fn pinned_metadata(pins: &DocumentMut) -> Result<&Item> {
    let name = pinned_network(pins)?;
    pins.get("networks")
        .and_then(|i| i.get(name))
        .and_then(|i| i.get("metadata"))
        .with_context(|| format!("[networks.{name}.metadata] is missing"))
}

pub(super) struct Prepared {
    network: Network,
    row: Row,
    index: DocumentMut,
    bytes: [Vec<u8>; 2],
}

fn hash(text: &str, prefix: &str, length: usize) -> bool {
    text.strip_prefix(prefix).is_some_and(|hex| {
        hex.len() == length
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn field<'a>(table: &'a dyn toml_edit::TableLike, key: &str) -> Result<&'a str> {
    table
        .get(key)
        .and_then(Item::as_str)
        .with_context(|| format!("metadata field {key} must be a string"))
}

fn metadata_table(pins: &DocumentMut) -> Result<&dyn toml_edit::TableLike> {
    pinned_metadata(pins)?
        .as_table_like()
        .context("pinned network metadata must be a table")
}

fn spec(table: &dyn toml_edit::TableLike) -> Result<u32> {
    let number = table
        .get("spec")
        .and_then(Item::as_integer)
        .context("metadata spec must be an integer")?;
    let spec = u32::try_from(number).context("metadata spec is outside u32")?;
    ensure!(spec > 0, "metadata spec must be positive");
    Ok(spec)
}

fn parse_row(table: &Table, network: &Network) -> Result<Row> {
    let spec = spec(table)?;
    let read_blob = |version: u8| -> Result<Blob> {
        let blob = table
            .get(&format!("v{version}"))
            .and_then(Item::as_table_like)
            .context("metadata blob row is missing")?;
        let bytes = blob
            .get("bytes")
            .and_then(Item::as_integer)
            .context("metadata blob byte length is missing")?;
        let blob = Blob {
            path: field(blob, "path")?.to_owned(),
            sha256: field(blob, "sha256")?.to_owned(),
            bytes: usize::try_from(bytes).context("metadata blob length is invalid")?,
        };
        ensure!(
            blob.path == format!("{spec}.v{version}.scale"),
            "metadata blob path is not canonical"
        );
        ensure!(
            hash(&blob.sha256, "", 64),
            "metadata blob SHA256 is invalid"
        );
        ensure!(
            (5..=MAX_BYTES).contains(&blob.bytes),
            "metadata blob length is outside limits"
        );
        Ok(blob)
    };
    let row = Row {
        spec,
        code_hash: field(table, "code_hash")?.to_owned(),
        genesis: field(table, "genesis")?.to_owned(),
        tag: field(table, "tag")?.to_owned(),
        commit: field(table, "commit")?.to_owned(),
        finalized_head: field(table, "finalized_head")?.to_owned(),
        endpoint: field(table, "endpoint")?.to_owned(),
        blobs: [read_blob(15)?, read_blob(16)?],
    };
    ensure!(
        row.genesis == network.genesis,
        "metadata index is not {}",
        network.name
    );
    ensure!(
        row.tag == format!("v{spec}"),
        "metadata tag differs from spec"
    );
    ensure!(hash(&row.commit, "", 40), "metadata commit is invalid");
    ensure!(
        hash(&row.code_hash, "0x", 64),
        "metadata code hash is invalid"
    );
    ensure!(
        hash(&row.finalized_head, "0x", 64),
        "metadata finalized head is invalid"
    );
    ensure!(
        row.endpoint.starts_with("https://"),
        "metadata endpoint must use HTTPS"
    );
    Ok(row)
}

fn row_table(row: &Row) -> Table {
    let mut table = Table::new();
    table["spec"] = value(i64::from(row.spec));
    for (key, text) in [
        ("code_hash", &row.code_hash),
        ("genesis", &row.genesis),
        ("tag", &row.tag),
        ("commit", &row.commit),
        ("finalized_head", &row.finalized_head),
        ("endpoint", &row.endpoint),
    ] {
        table[key] = value(text);
    }
    for (version, blob) in [15, 16].into_iter().zip(&row.blobs) {
        let mut item = InlineTable::new();
        item.insert("path", Value::from(blob.path.clone()));
        item.insert("sha256", Value::from(blob.sha256.clone()));
        item.insert("bytes", Value::from(blob.bytes as i64));
        table[&format!("v{version}")] = Item::Value(Value::InlineTable(item));
    }
    table
}

fn decode_hex(hex: &str) -> Result<Vec<u8>> {
    let hex = hex
        .strip_prefix("0x")
        .context("metadata RPC result has no 0x prefix")?;
    ensure!(
        hex.len() % 2 == 0 && hex.len() <= (MAX_BYTES + 8) * 2,
        "metadata RPC result has an invalid length"
    );
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let byte = std::str::from_utf8(pair)?;
            u8::from_str_radix(byte, 16).context("metadata RPC result contains invalid hex")
        })
        .collect()
}

fn unwrap_metadata(bytes: &[u8]) -> Result<Vec<u8>> {
    let option = Option::<Vec<u8>>::decode_all(&mut &*bytes)
        .context("invalid Option<OpaqueMetadata> SCALE envelope")?;
    ensure!(
        option.encode() == bytes,
        "metadata SCALE envelope is not canonical"
    );
    let raw = option.context("runtime returned None for required metadata version")?;
    ensure!(
        (5..=MAX_BYTES).contains(&raw.len()),
        "metadata length is outside limits"
    );
    Ok(raw)
}

fn validate_metadata(raw: &[u8], expected: u8) -> Result<()> {
    ensure!(
        (5..=MAX_BYTES).contains(&raw.len()),
        "metadata length is outside limits"
    );
    ensure!(
        raw[..4] == META_RESERVED.to_le_bytes(),
        "metadata magic is not meta"
    );
    ensure!(
        raw[4] == expected,
        "metadata version {} differs from requested {expected}",
        raw[4]
    );
    let metadata = RuntimeMetadataPrefixed::decode_all(&mut &*raw)
        .context("metadata is not complete valid SCALE")?;
    ensure!(
        matches!(
            (&metadata.1, expected),
            (RuntimeMetadata::V15(_), 15) | (RuntimeMetadata::V16(_), 16)
        ),
        "metadata decoder returned a different version"
    );
    ensure!(
        metadata.encode() == raw,
        "metadata SCALE encoding is not canonical"
    );
    Ok(())
}

fn regular_file(root: &Path, relative: &str) -> Result<PathBuf> {
    let mut path = root.to_owned();
    for component in Path::new(relative).components() {
        ensure!(
            matches!(component, std::path::Component::Normal(_)),
            "metadata path escapes repository"
        );
        path.push(component);
        let info = fs::symlink_metadata(&path)
            .with_context(|| format!("missing metadata input {}", path.display()))?;
        ensure!(
            !info.file_type().is_symlink(),
            "metadata input {} is a symlink",
            path.display()
        );
    }
    ensure!(
        fs::symlink_metadata(&path)?.is_file(),
        "metadata input is not a regular file"
    );
    Ok(path)
}

fn path_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

fn verify_blob(root: &Path, network: &Network, blob: &Blob, version: u8) -> Result<()> {
    let path = regular_file(root, &format!("{}/{}", network.directory(), blob.path))?;
    ensure!(
        fs::metadata(&path)?.len() == blob.bytes as u64,
        "{} byte length differs from index",
        path.display()
    );
    let raw = fs::read(&path)?;
    ensure!(
        format!("{:x}", Sha256::digest(&raw)) == blob.sha256,
        "{} SHA256 differs from index",
        path.display()
    );
    validate_metadata(&raw, version).with_context(|| format!("validate {}", path.display()))
}

fn read_index(root: &Path, network: &Network) -> Result<(DocumentMut, Vec<Row>)> {
    let index = crate::pin::read_document(&regular_file(root, &network.index())?)?;
    ensure!(
        index.get("schema").and_then(Item::as_integer) == Some(1),
        "unsupported metadata index schema"
    );
    ensure!(
        index.get("network").and_then(Item::as_str) == Some(network.name.as_str()),
        "incorrect metadata index network"
    );
    let entries = index
        .get("runtimes")
        .and_then(Item::as_array_of_tables)
        .context("metadata index runtimes are missing")?;
    let mut rows = Vec::new();
    let mut specs = BTreeSet::new();
    for entry in entries {
        let row = parse_row(entry, network)?;
        ensure!(
            specs.insert(row.spec),
            "duplicate metadata index spec {}",
            row.spec
        );
        for (version, blob) in [15, 16].into_iter().zip(&row.blobs) {
            verify_blob(root, network, blob, version)?;
        }
        rows.push(row);
    }
    ensure!(!rows.is_empty(), "metadata index has no runtimes");
    Ok((index, rows))
}

pub(super) fn verify(root: &Path, pins: &DocumentMut) -> Result<()> {
    let network = Network::from_pins(pins)?;
    let (_, rows) = read_index(root, &network)?;
    let table = metadata_table(pins)?;
    let pinned_spec = spec(table)?;
    ensure!(
        pins.get("parity")
            .and_then(|i| i.get("spec_version"))
            .and_then(Item::as_integer)
            == Some(i64::from(pinned_spec)),
        "metadata spec differs from parity pin"
    );
    let row = rows
        .iter()
        .find(|r| r.spec == pinned_spec)
        .context("pinned metadata is absent from index")?;
    for (key, text) in [
        ("code_hash", &row.code_hash),
        ("finalized_head", &row.finalized_head),
    ] {
        ensure!(
            field(table, key)? == text,
            "metadata {key} differs from index"
        );
    }
    for (section, key, text) in [
        ("parity", "tag", &row.tag),
        ("parity", "commit", &row.commit),
    ] {
        ensure!(
            pins.get(section)
                .and_then(|i| i.get(key))
                .and_then(Item::as_str)
                == Some(text),
            "metadata {key} differs from parity pin"
        );
    }
    ensure!(
        network.genesis == row.genesis,
        "metadata genesis differs from network pin"
    );
    for (version, blob) in [15, 16].into_iter().zip(&row.blobs) {
        ensure!(
            field(table, &format!("v{version}"))?
                == format!("{}/{}", network.directory(), blob.path),
            "pinned metadata path differs from index"
        );
        ensure!(
            field(table, &format!("v{version}_sha256"))? == blob.sha256,
            "pinned metadata SHA256 differs from index"
        );
    }
    Ok(())
}

/// The caller has already verified the immutable release's code hash, genesis,
/// spec and finalized block. Repeat the runtime reads at that exact block before
/// fetching both versions, and verify every previously recorded input offline.
pub(super) fn prepare(
    root: &Path,
    pins: &DocumentMut,
    endpoint: &str,
    mut rpc: impl FnMut(&str, Json) -> Result<Json>,
) -> Result<Prepared> {
    let network = Network::from_pins(pins)?;
    let table = metadata_table(pins)?;
    let spec = spec(table)?;
    let head = field(table, "finalized_head")?;
    let code = field(table, "code_hash")?;
    ensure!(
        rpc("chain_getBlockHash", json!([0]))?.as_str() == Some(network.genesis.as_str()),
        "metadata endpoint is not {}",
        network.name
    );
    ensure!(
        rpc("state_getRuntimeVersion", json!([head]))?["specVersion"].as_u64()
            == Some(u64::from(spec)),
        "metadata finalized spec differs from pin"
    );
    ensure!(
        rpc("state_getStorageHash", json!(["0x3a636f6465", head]))?.as_str() == Some(code),
        "metadata finalized code differs from pin"
    );
    let mut bytes = Vec::new();
    let mut blobs = Vec::new();
    for version in [15u8, 16] {
        let encoded_version = format!("0x{version:02x}000000");
        let result = rpc(
            "state_call",
            json!(["Metadata_metadata_at_version", encoded_version, head]),
        )?;
        let raw = unwrap_metadata(&decode_hex(
            result
                .as_str()
                .context("metadata RPC result is not a string")?,
        )?)?;
        validate_metadata(&raw, version)?;
        blobs.push(Blob {
            path: format!("{spec}.v{version}.scale"),
            sha256: format!("{:x}", Sha256::digest(&raw)),
            bytes: raw.len(),
        });
        bytes.push(raw);
    }
    let mut row = Row {
        spec,
        code_hash: code.to_owned(),
        genesis: network.genesis.clone(),
        tag: pins["parity"]["tag"]
            .as_str()
            .context("parity tag missing")?
            .to_owned(),
        commit: pins["parity"]["commit"]
            .as_str()
            .context("parity commit missing")?
            .to_owned(),
        finalized_head: head.to_owned(),
        endpoint: endpoint.to_owned(),
        blobs: blobs
            .try_into()
            .map_err(|_| anyhow::anyhow!("both metadata versions required"))?,
    };
    parse_row(&row_table(&row), &network)?;
    let mut index = if path_exists(&root.join(network.index()))? {
        let (index, rows) = read_index(root, &network)?;
        if let Some(old) = rows.iter().find(|old| old.spec == row.spec) {
            ensure!(
                old.code_hash == row.code_hash
                    && old.tag == row.tag
                    && old.commit == row.commit
                    && old.blobs == row.blobs,
                "refusing to overwrite immutable metadata spec {}",
                row.spec
            );
            row = old.clone();
        }
        index
    } else {
        let mut index = DocumentMut::new();
        index["schema"] = value(1);
        index["network"] = value(&network.name);
        index["runtimes"] = Item::ArrayOfTables(ArrayOfTables::new());
        index
    };
    let entries = index["runtimes"]
        .as_array_of_tables_mut()
        .context("metadata index runtimes missing")?;
    if !entries
        .iter()
        .any(|entry| entry.get("spec").and_then(Item::as_integer) == Some(i64::from(row.spec)))
    {
        entries.push(row_table(&row));
    }
    Ok(Prepared {
        network,
        row,
        index,
        bytes: bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("both metadata versions required"))?,
    })
}

impl Prepared {
    pub(super) fn update_pins(&self, pins: &mut DocumentMut) {
        let directory = self.network.directory();
        let table = &mut pins["networks"][self.network.name.as_str()]["metadata"];
        table["finalized_head"] = value(&self.row.finalized_head);
        for (version, blob) in [15, 16].into_iter().zip(&self.row.blobs) {
            table[&format!("v{version}")] = value(format!("{directory}/{}", blob.path));
            table[&format!("v{version}_sha256")] = value(&blob.sha256);
        }
    }

    /// Publish immutable blobs before the index. The upstream pin is the final
    /// publication marker, written by the caller after this succeeds. Repeating
    /// generation after an interruption recovers matching unpublished blobs.
    pub(super) fn publish(&self, root: &Path) -> Result<()> {
        let temporary = std::env::var_os("RTAO_TEMP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("target"));
        let stage = temporary.join("pin").join(format!(
            "metadata-write-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        ));
        fs::create_dir_all(stage.parent().context("metadata stage parent missing")?)?;
        fs::create_dir(&stage)?;
        let result = self.publish_from_stage(root, &stage);
        // Only this invocation creates these three exact staged files. No live
        // process, configuration, environment or link retains the stage.
        for name in ["v15.scale", "v16.scale", "index.toml"] {
            let path = stage.join(name);
            if path.try_exists()? {
                fs::remove_file(path)?;
            }
        }
        fs::remove_dir(&stage)?;
        result
    }

    fn publish_from_stage(&self, root: &Path, stage: &Path) -> Result<()> {
        let relative_directory = self.network.directory();
        let index = self.network.index();
        let directory = root.join(&relative_directory);
        for relative in ["vendor", "vendor/metadata", relative_directory.as_str()] {
            let path = root.join(relative);
            match fs::create_dir(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error).context("create metadata directory"),
            }
            ensure!(
                fs::symlink_metadata(path)?.is_dir(),
                "metadata destination parent is not an ordinary directory"
            );
        }
        for (version, (blob, raw)) in [15, 16]
            .into_iter()
            .zip(self.row.blobs.iter().zip(&self.bytes))
        {
            let staged = stage.join(format!("v{version}.scale"));
            fs::write(&staged, raw)?;
            fs::File::open(&staged)?.sync_all()?;
            let destination = directory.join(&blob.path);
            match fs::hard_link(&staged, &destination) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    verify_blob(root, &self.network, blob, version)
                        .context("existing immutable metadata differs; refusing replacement")?;
                }
                Err(error) => return Err(error).context("publish immutable metadata blob"),
            }
            fs::remove_file(&staged)?;
        }
        let staged = stage.join("index.toml");
        fs::write(
            &staged,
            format!(
                "# Generated by cargo xtask pin; immutable runtime rows.\n{}",
                self.index
            ),
        )?;
        fs::File::open(&staged)?.sync_all()?;
        // Existing parent components must remain ordinary directories.
        for path in [root.join("vendor"), root.join("vendor/metadata"), directory] {
            ensure!(
                fs::symlink_metadata(path)?.is_dir(),
                "metadata destination parent is not a directory"
            );
        }
        if path_exists(&root.join(&index))? {
            regular_file(root, &index)?;
        }
        fs::rename(staged, root.join(&index)).context("publish metadata index")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GENESIS: &str = "0x2f0555cc76fc2840a25a6ea3b9637146806f1f44b090c175ffde2a7e5ab36c03";

    /// The committed pins' metadata network, directory and index.
    fn committed(repository: &Path) -> Result<(Network, String, String)> {
        let pins = crate::pin::read_document(&repository.join("upstream.toml"))?;
        let network = Network::from_pins(&pins)?;
        let (directory, index) = (network.directory(), network.index());
        Ok((network, directory, index))
    }

    #[test]
    fn opaque_metadata_requires_exact_some_envelope() -> Result<()> {
        let raw = b"meta\x0f".to_vec();
        let encoded = Some(raw.clone()).encode();
        ensure!(unwrap_metadata(&encoded)? == raw);
        for bad in [
            vec![0],
            vec![2],
            vec![1],
            raw,
            [encoded.as_slice(), &[0]].concat(),
        ] {
            ensure!(unwrap_metadata(&bad).is_err());
        }
        // Compact vec length exceeds the remaining input.
        ensure!(unwrap_metadata(&[1, 24, b'm', b'e', b't', b'a', 15]).is_err());
        Ok(())
    }

    #[test]
    fn full_upstream_metadata_rejects_corruption_and_trailing_bytes() -> Result<()> {
        let fixture: Json =
            serde_json::from_str(include_str!("../../vendor/upstream/golden.json"))?;
        let raw = unwrap_metadata(&decode_hex(
            fixture["metadata"]["v15_hex"]
                .as_str()
                .context("missing upstream metadata")?,
        )?)?;
        validate_metadata(&raw, 15)?;
        ensure!(validate_metadata(&raw, 16).is_err());
        ensure!(validate_metadata(&raw[..raw.len() - 1], 15).is_err());
        ensure!(validate_metadata(&[raw.as_slice(), &[0]].concat(), 15).is_err());
        let mut bad = raw.clone();
        bad[0] ^= 1;
        ensure!(validate_metadata(&bad, 15).is_err());
        ensure!(validate_metadata(b"meta\x0f", 15).is_err());
        Ok(())
    }

    #[test]
    fn metadata_hex_is_bounded_and_strict() -> Result<()> {
        ensure!(decode_hex("0x6d6574610f")? == b"meta\x0f");
        for bad in ["meta", "0x1", "0xgg", "0xé"] {
            ensure!(decode_hex(bad).is_err());
        }
        Ok(())
    }

    #[test]
    fn fetch_pins_every_runtime_read_and_fails_closed_on_identity() -> Result<()> {
        let head = format!("0x{}", "11".repeat(32));
        let code = format!("0x{}", "22".repeat(32));
        let pins: DocumentMut = format!(
            "[parity]\ntag='v478'\ncommit='{}'\n[networks.finney]\ngenesis='{GENESIS}'\n[networks.finney.metadata]\nspec=478\nfinalized_head='{head}'\ncode_hash='{code}'\n",
            "33".repeat(20)
        ).parse()?;
        for broken in 0..3 {
            let mut calls = 0;
            let result = prepare(
                Path::new("."),
                &pins,
                "https://finney.example",
                |method, params| {
                    let current = calls;
                    calls += 1;
                    let correct = match current {
                        0 => {
                            ensure!(method == "chain_getBlockHash" && params == json!([0]));
                            json!(GENESIS)
                        }
                        1 => {
                            ensure!(method == "state_getRuntimeVersion" && params == json!([head]));
                            json!({"specVersion": 478})
                        }
                        2 => {
                            ensure!(
                                method == "state_getStorageHash"
                                    && params == json!(["0x3a636f6465", head])
                            );
                            json!(code)
                        }
                        _ => anyhow::bail!("metadata was fetched after failed identity"),
                    };
                    Ok(if current == broken {
                        json!("wrong identity")
                    } else {
                        correct
                    })
                },
            );
            ensure!(
                result.is_err() && calls == broken + 1,
                "identity failure did not stop fetching"
            );
        }
        let fixture: Json =
            serde_json::from_str(include_str!("../../vendor/upstream/golden.json"))?;
        let mut methods = Vec::new();
        let result = prepare(
            Path::new("."),
            &pins,
            "https://finney.example",
            |method, params| {
                methods.push(method.to_owned());
                Ok(match method {
                    "chain_getBlockHash" => json!(GENESIS),
                    "state_getRuntimeVersion" => {
                        ensure!(params == json!([head]));
                        json!({"specVersion": 478})
                    }
                    "state_getStorageHash" => {
                        ensure!(params == json!(["0x3a636f6465", head]));
                        json!(code)
                    }
                    "state_call" => {
                        let version = if methods.len() == 4 { 15 } else { 16 };
                        ensure!(
                            params
                                == json!([
                                    "Metadata_metadata_at_version",
                                    format!("0x{version:02x}000000"),
                                    head
                                ])
                        );
                        if version == 15 {
                            fixture["metadata"]["v15_hex"].clone()
                        } else {
                            json!("0x00")
                        }
                    }
                    _ => anyhow::bail!("unexpected metadata RPC"),
                })
            },
        );
        ensure!(result.is_err());
        ensure!(
            methods
                == [
                    "chain_getBlockHash",
                    "state_getRuntimeVersion",
                    "state_getStorageHash",
                    "state_call",
                    "state_call"
                ]
        );
        Ok(())
    }

    #[test]
    fn offline_index_detects_blob_pin_and_provenance_tampering() -> Result<()> {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace parent missing")?;
        let temporary = std::env::var_os("RTAO_TEMP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| repository.join("target"));
        let parent = temporary.join("metadata-pin");
        fs::create_dir_all(&parent)?;
        let scratch = parent.join(format!(
            "offline-test-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        ));
        fs::create_dir(&scratch)?;
        let (network, metadata_directory, metadata_index) = committed(repository)?;
        let directory = scratch.join(&metadata_directory);
        fs::create_dir_all(&directory)?;
        let mut copied = Vec::new();
        let result = (|| -> Result<()> {
            for entry in fs::read_dir(repository.join(&metadata_directory))? {
                let entry = entry?;
                ensure!(
                    entry.file_type()?.is_file(),
                    "metadata fixture is not a regular file"
                );
                let destination = directory.join(entry.file_name());
                fs::copy(entry.path(), &destination)?;
                copied.push(destination);
            }
            let mut pins = crate::pin::read_document(&repository.join("upstream.toml"))?;
            verify(&scratch, &pins)?;
            let blob = field(metadata_table(&pins)?, "v15")?.to_owned();
            let path = scratch.join(&blob);
            let bytes = fs::read(&path)?;
            let mut corrupted = bytes.clone();
            corrupted[0] ^= 1;
            fs::write(&path, corrupted)?;
            ensure!(
                verify(&scratch, &pins).is_err(),
                "tampered metadata was accepted"
            );
            fs::write(&path, &bytes)?;
            pins["networks"][network.name.as_str()]["metadata"]["v15_sha256"] =
                value("00".repeat(32));
            ensure!(
                verify(&scratch, &pins).is_err(),
                "tampered metadata pin was accepted"
            );
            pins = crate::pin::read_document(&repository.join("upstream.toml"))?;
            let mut index = crate::pin::read_document(&scratch.join(&metadata_index))?;
            let runtime = index["runtimes"]
                .as_array_of_tables_mut()
                .context("fixture index rows missing")?
                .iter_mut()
                .find(|row| row["spec"].as_integer() == pins["parity"]["spec_version"].as_integer())
                .context("fixture runtime missing")?;
            runtime["commit"] = value("00".repeat(20));
            fs::write(scratch.join(&metadata_index), index.to_string())?;
            ensure!(
                verify(&scratch, &pins).is_err(),
                "tampered metadata provenance was accepted"
            );
            fs::copy(
                repository.join(&metadata_index),
                scratch.join(&metadata_index),
            )?;
            #[cfg(unix)]
            {
                fs::remove_file(&path)?;
                std::os::unix::fs::symlink(repository.join(&blob), &path)?;
                ensure!(
                    verify(&scratch, &pins).is_err(),
                    "metadata symlink was accepted"
                );
                fs::remove_file(&path)?;
                fs::write(&path, bytes)?;
            }
            verify(&scratch, &pins)
        })();
        // The test starts no processes, writes no configuration or launchd unit,
        // creates no virtual environment, and retains no handles. Remove only
        // its recorded regular files and symlinks, never their link targets.
        for path in copied {
            if let Ok(info) = fs::symlink_metadata(&path) {
                ensure!(
                    info.is_file() || info.file_type().is_symlink(),
                    "unexpected test fixture dependency"
                );
                fs::remove_file(path)?;
            }
        }
        for path in [
            directory,
            scratch.join("vendor/metadata"),
            scratch.join("vendor"),
            scratch,
        ] {
            fs::remove_dir(path)?;
        }
        result
    }

    #[test]
    fn repeat_fetch_preserves_immutable_bytes_and_original_provenance() -> Result<()> {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace parent missing")?;
        let mut pins = crate::pin::read_document(&repository.join("upstream.toml"))?;
        let (network, metadata_directory, _) = committed(repository)?;
        let (_, rows) = read_index(repository, &network)?;
        let original = rows
            .iter()
            .find(|row| Some(i64::from(row.spec)) == pins["parity"]["spec_version"].as_integer())
            .context("original metadata row missing")?;
        let raw: Vec<_> = original
            .blobs
            .iter()
            .map(|blob| fs::read(repository.join(&metadata_directory).join(&blob.path)))
            .collect::<std::io::Result<_>>()?;
        let newer_head = format!("0x{}", "55".repeat(32));
        pins["networks"][network.name.as_str()]["metadata"]["finalized_head"] = value(&newer_head);
        let fetch = |change: bool| -> Result<Prepared> {
            prepare(
                repository,
                &pins,
                "https://alternate-finney.example",
                |method, params| {
                    Ok(match method {
                        "chain_getBlockHash" => json!(network.genesis),
                        "state_getRuntimeVersion" => json!({"specVersion": original.spec}),
                        "state_getStorageHash" => json!(original.code_hash),
                        "state_call" => {
                            ensure!(params[2].as_str() == Some(newer_head.as_str()));
                            let position = if params[1].as_str() == Some("0x0f000000") {
                                0
                            } else {
                                1
                            };
                            let mut bytes = raw[position].clone();
                            if change && position == 0 {
                                let mut metadata =
                                    RuntimeMetadataPrefixed::decode_all(&mut &*bytes)?;
                                if let RuntimeMetadata::V15(metadata) = &mut metadata.1 {
                                    metadata
                                        .pallets
                                        .first_mut()
                                        .context("fixture pallet missing")?
                                        .docs
                                        .push("changed immutable fixture".to_owned());
                                }
                                bytes = metadata.encode();
                            }
                            let envelope = Some(bytes).encode();
                            json!(format!(
                                "0x{}",
                                envelope
                                    .iter()
                                    .map(|byte| format!("{byte:02x}"))
                                    .collect::<String>()
                            ))
                        }
                        _ => anyhow::bail!("unexpected metadata RPC"),
                    })
                },
            )
        };
        let prepared = fetch(false)?;
        ensure!(
            prepared.row == *original,
            "repeat fetch replaced original provenance"
        );
        ensure!(prepared.bytes.as_slice() == raw.as_slice());
        ensure!(
            fetch(true).is_err(),
            "repeat fetch accepted different immutable metadata"
        );
        Ok(())
    }
}
