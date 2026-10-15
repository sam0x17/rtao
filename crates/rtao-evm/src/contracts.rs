// Derived from RaoFoundation/subtensor sdk/python/bittensor/evm/contracts.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Bounded user-contract artifact loading and constructor calldata.

use std::path::{Path, PathBuf};

use alloy_dyn_abi::JsonAbiExt;
use alloy_json_abi::{AbiItem, Constructor, Function};
use serde_json::Value;

use crate::{Error, Result, precompiles};

/// Compiled ABI and optional init bytecode from a user-supplied artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContractArtifact {
    /// Solidity ABI entries in source order, when the artifact contains one.
    ///
    /// A list retains interleaved overloads and repeated entries, matching
    /// Python's artifact representation instead of grouping by function name.
    pub abi: Option<Vec<AbiItem<'static>>>,
    /// Validated, lowercase 0x-hex init bytecode, when supplied.
    pub bytecode: Option<String>,
    /// Source path used to identify errors.
    pub source: String,
}

impl ContractArtifact {
    /// All functions, including overloads and duplicates, in source ABI order.
    pub fn functions(&self) -> Vec<&Function> {
        self.abi
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter_map(|entry| match entry {
                AbiItem::Function(function) => Some(function.as_ref()),
                _ => None,
            })
            .collect()
    }

    /// Resolve a name or an exact canonical function signature.
    pub fn function(&self, name: &str) -> Result<&Function> {
        if self.abi.is_none() {
            return Err(Error::Validation(format!("{} carries no ABI", self.source)));
        }
        precompiles::find_function(self.functions().into_iter(), name, &self.source)
    }

    /// Constructor ABI, when explicitly present.
    pub fn constructor(&self) -> Option<&Constructor> {
        self.abi.as_deref()?.iter().find_map(|entry| match entry {
            AbiItem::Constructor(constructor) => Some(constructor.as_ref()),
            _ => None,
        })
    }

    /// Parse artifact text already acquired by a caller.
    ///
    /// Supported shapes are Hardhat/Foundry objects, bare ABI arrays and raw
    /// hex bytecode. Invalid hex and unresolved library placeholders fail here.
    pub fn from_text(text: &str, source: impl Into<String>) -> Result<Self> {
        if text.len() > precompiles::MAX_ABI_BYTES {
            return Err(Error::Validation(
                "contract artifact exceeds 16 MiB limit".into(),
            ));
        }
        let source = source.into();
        let text = text.trim();
        if !text.starts_with(['{', '[']) {
            let code = text.replace('\n', "");
            return Ok(Self {
                abi: None,
                bytecode: Some(normalize_bytecode(&code)?),
                source,
            });
        }
        let payload: Value = serde_json::from_str(text).map_err(Error::Json)?;
        if payload.is_array() {
            return Ok(Self {
                abi: Some(serde_json::from_value(payload).map_err(Error::Json)?),
                bytecode: None,
                source,
            });
        }
        let object = payload
            .as_object()
            .ok_or_else(|| Error::Validation("expected an artifact object or ABI array".into()))?;
        let abi = object
            .get("abi")
            .filter(|value| !value.is_null())
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(Error::Json)?;
        let bytecode = object
            .get("bytecode")
            .and_then(extract_bytecode)
            .map(normalize_bytecode)
            .transpose()?;
        if abi.is_none() && bytecode.is_none() {
            return Err(Error::Validation(format!(
                "{source} is JSON but has neither an 'abi' nor a 'bytecode' field — expected a Hardhat/Foundry artifact, an ABI array, or a .bin file"
            )));
        }
        Ok(Self {
            abi,
            bytecode,
            source,
        })
    }
}

/// Read a contract artifact with the shared 16 MiB regular-file reader.
///
/// A leading `~/` is expanded. Regular symlinks work; special files and files
/// growing beyond the bound are refused before parsing. No file is modified.
pub fn load_artifact(path: impl AsRef<Path>) -> Result<ContractArtifact> {
    let path = expand_home(path.as_ref())?;
    let bytes = rtao_client::config::read_bounded_json_file(&path).map_err(|source| Error::Io {
        path: path.clone(),
        source,
    })?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|error| Error::Validation(format!("{} is not UTF-8: {error}", path.display())))?;
    ContractArtifact::from_text(text, path.display().to_string())
}

/// Append ABI-encoded constructor arguments to validated init bytecode.
///
/// A constructor selector is never added. Generic Solidity integers keep their
/// full declared widths, independently of native chain balance limits.
pub fn encode_deploy(artifact: &ContractArtifact, args: &[Value]) -> Result<String> {
    let bytecode = artifact
        .bytecode
        .as_deref()
        .filter(|code| *code != "0x")
        .ok_or_else(|| {
            Error::Validation(format!(
                "{} carries no bytecode — pass the compiler artifact",
                artifact.source
            ))
        })?;
    // Validate even a manually constructed public artifact before encoding.
    let mut bytecode = normalize_bytecode(bytecode)?;
    let constructor = artifact.constructor();
    let inputs = constructor
        .map(|constructor| constructor.inputs.as_slice())
        .unwrap_or_default();
    let values = precompiles::coerce_inputs(inputs, args)?;
    if (bytecode.len() - 2) / 2 + precompiles::encoded_arguments_size(&values)?
        > precompiles::MAX_ABI_BYTES
    {
        return Err(Error::Validation(
            "deployment calldata exceeds 16 MiB limit".into(),
        ));
    }
    if let Some(constructor) = constructor {
        let encoded = constructor
            .abi_encode_input(&values)
            .map_err(|error| Error::Validation(format!("invalid constructor ABI: {error}")))?;
        if (bytecode.len() - 2) / 2 + encoded.len() > precompiles::MAX_ABI_BYTES {
            return Err(Error::Validation(
                "deployment calldata exceeds 16 MiB limit".into(),
            ));
        }
        bytecode.push_str(&hex::encode(encoded));
    }
    Ok(bytecode)
}

fn extract_bytecode(value: &Value) -> Option<&str> {
    match value {
        Value::String(text) if !text.is_empty() && text != "0x" => Some(text),
        Value::Object(object) => object.get("object").and_then(extract_bytecode),
        _ => None,
    }
}

fn normalize_bytecode(code: &str) -> Result<String> {
    let code = code.strip_prefix("0x").unwrap_or(code);
    if code.len() > precompiles::MAX_ABI_BYTES * 2
        || !code.len().is_multiple_of(2)
        || !code.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(Error::Validation("invalid or oversized contract bytecode; link all library placeholders before deployment".into()));
    }
    Ok(format!("0x{}", code.to_ascii_lowercase()))
}

fn expand_home(path: &Path) -> Result<PathBuf> {
    let text = path.to_string_lossy();
    if text == "~" || text.starts_with("~/") {
        let home = std::env::var_os("HOME")
            .ok_or_else(|| Error::Validation("cannot expand artifact path without HOME".into()))?;
        return Ok(if text == "~" {
            PathBuf::from(home)
        } else {
            PathBuf::from(home).join(&text[2..])
        });
    }
    Ok(path.to_path_buf())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hardhat_foundry_abi_array_and_bin_artifacts_share_constructor_semantics() {
        let abi = json!([{"type":"constructor","inputs":[{"name":"count","type":"uint256"}],"stateMutability":"nonpayable"},{"type":"function","name":"count","inputs":[],"outputs":[{"name":"","type":"uint256"}],"stateMutability":"view"}]);
        for bytecode in [json!("60006000"), json!({"object":"0x60006000"})] {
            let artifact = ContractArtifact::from_text(
                &json!({"abi":abi,"bytecode":bytecode}).to_string(),
                "fixture.json",
            )
            .unwrap();
            assert_eq!(artifact.bytecode.as_deref(), Some("0x60006000"));
            assert_eq!(artifact.functions().len(), 1);
            assert_eq!(artifact.function("count").unwrap().signature(), "count()");
            // Four init-code bytes followed by one 32-byte constructor word.
            assert_eq!(
                encode_deploy(&artifact, &[json!(42)]).unwrap(),
                format!("0x60006000{:064x}", 42)
            );
            assert!(encode_deploy(&artifact, &[]).is_err());
        }
        let bare_abi = ContractArtifact::from_text(&abi.to_string(), "abi.json").unwrap();
        assert!(bare_abi.constructor().is_some());
        assert!(encode_deploy(&bare_abi, &[json!(42)]).is_err());
        let bin = ContractArtifact::from_text("6000\n6000\n", "contract.bin").unwrap();
        assert_eq!(encode_deploy(&bin, &[]).unwrap(), "0x60006000");
        assert!(encode_deploy(&bin, &[json!(1)]).is_err());
    }

    #[test]
    fn function_enumeration_preserves_source_order_interleaved_overloads_and_duplicates() {
        let abi = json!([
            {"type":"constructor","inputs":[],"stateMutability":"nonpayable"},
            {"type":"function","name":"zebra","inputs":[{"name":"n","type":"uint256"}],"outputs":[],"stateMutability":"nonpayable"},
            {"type":"event","name":"Changed","inputs":[],"anonymous":false},
            {"type":"function","name":"alpha","inputs":[],"outputs":[],"stateMutability":"view"},
            {"type":"fallback","stateMutability":"nonpayable"},
            {"type":"function","name":"zebra","inputs":[{"name":"who","type":"address"}],"outputs":[],"stateMutability":"view"},
            {"type":"error","name":"Denied","inputs":[]},
            {"type":"function","name":"zebra","inputs":[{"name":"n","type":"uint256"}],"outputs":[],"stateMutability":"nonpayable"}
        ]);
        for payload in [abi.clone(), json!({"abi":abi,"bytecode":"0x6000"})] {
            let mut artifact =
                ContractArtifact::from_text(&payload.to_string(), "ordered.json").unwrap();
            assert_eq!(
                artifact
                    .functions()
                    .iter()
                    .map(|function| function.signature())
                    .collect::<Vec<_>>(),
                [
                    "zebra(uint256)",
                    "alpha()",
                    "zebra(address)",
                    "zebra(uint256)"
                ]
            );
            assert!(artifact.constructor().is_some());
            assert_eq!(
                artifact.function("zebra").unwrap().signature(),
                "zebra(uint256)"
            );
            assert_eq!(
                artifact.function("zebra(address)").unwrap().signature(),
                "zebra(address)"
            );
            let error = artifact.function("missing").unwrap_err().to_string();
            assert!(error.contains("Available: alpha, zebra, zebra, zebra"));

            // The public ABI list remains authoritative after caller edits;
            // function enumeration must not rely on a stale parallel cache.
            artifact.abi.as_mut().unwrap().reverse();
            assert_eq!(
                artifact
                    .functions()
                    .iter()
                    .map(|function| function.signature())
                    .collect::<Vec<_>>(),
                [
                    "zebra(uint256)",
                    "zebra(address)",
                    "alpha()",
                    "zebra(uint256)"
                ]
            );
        }
    }

    #[test]
    fn bytecode_only_and_abi_only_artifacts_keep_missing_fields_explicit() {
        let bytecode =
            ContractArtifact::from_text(r#"{"bytecode":"0x60AA"}"#, "code.json").unwrap();
        assert_eq!(bytecode.bytecode.as_deref(), Some("0x60aa"));
        assert!(bytecode.abi.is_none());
        assert!(bytecode.function("missing").is_err());
        assert_eq!(encode_deploy(&bytecode, &[]).unwrap(), "0x60aa");
        let abi = ContractArtifact::from_text("[]", "empty-abi.json").unwrap();
        assert_eq!(abi.functions().len(), 0);
        assert!(abi.function("missing").is_err());
        assert!(ContractArtifact::from_text(r#"{"bytecode":"0x"}"#, "empty.json").is_err());
        assert!(ContractArtifact::from_text("{}", "bad.json").is_err());
    }

    #[test]
    fn malformed_unlinked_and_oversized_artifacts_fail_before_deploy() {
        for text in [
            "0x1",
            "not hex",
            "0x__$123$__",
            "{",
            r#"{"abi":42}"#,
            r#"{"bytecode":{"object":"bad!"}}"#,
        ] {
            assert!(ContractArtifact::from_text(text, "bad").is_err(), "{text}");
        }
        let enormous = "0".repeat(precompiles::MAX_ABI_BYTES + 1);
        assert!(ContractArtifact::from_text(&enormous, "large").is_err());
        let artifact = ContractArtifact {
            abi: None,
            bytecode: Some("0xnothex".into()),
            source: "manual".into(),
        };
        assert!(encode_deploy(&artifact, &[]).is_err());
    }

    #[test]
    fn constructor_dynamic_tuple_array_uses_abi_offsets_without_a_selector() {
        let artifact = ContractArtifact::from_text(r#"{"bytecode":"0x6000","abi":[{"type":"constructor","inputs":[{"name":"items","type":"tuple[]","components":[{"name":"n","type":"uint256"},{"name":"ok","type":"bool"}]}],"stateMutability":"nonpayable"}]}"#, "tuple.json").unwrap();
        // Array tail at 32; one element; static tuple (7,true), each one word.
        let expected = format!("0x6000{:064x}{:064x}{:064x}{:064x}", 32, 1, 7, 1);
        assert_eq!(
            encode_deploy(&artifact, &[json!([[7, true]])]).unwrap(),
            expected
        );
    }

    #[test]
    fn file_loader_uses_the_shared_bounded_reader_for_a_real_vendored_abi() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../vendor/upstream/evm/abi/stakingV2.json");
        let artifact = load_artifact(&path).unwrap();
        assert!(artifact.bytecode.is_none());
        assert_eq!(artifact.source, path.display().to_string());
        assert_eq!(
            artifact.function("addStake").unwrap().signature(),
            "addStake(bytes32,uint256,uint256)"
        );
    }
}
