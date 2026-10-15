//! CLI-only frozen basket display data from the authenticated literal corpus.

use anyhow::{Context, Result, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs, path::Path};
use toml_edit::DocumentMut;

const SOURCE: &str = "sdk/python/bittensor/basket_index_data.py";
const OUTPUT: &str = "crates/rtao-cli/src/root_helpers/basket_index_data.rs";

fn tuple(value: &Value, length: usize) -> Result<&[Value]> {
    let values = value["tuple"]
        .as_array()
        .context("basket literal is not a tuple")?;
    ensure!(values.len() == length, "basket tuple arity differs");
    Ok(values)
}

fn integer(value: &Value) -> Result<u64> {
    value.as_u64().context("basket block is not a u64 integer")
}

fn float(value: &Value, positive: bool) -> Result<String> {
    let number = value.as_number().context("basket value is not numeric")?;
    let parsed = number.as_f64().context("basket number exceeds f64")?;
    ensure!(
        parsed.is_finite()
            && if positive {
                parsed > 0.0
            } else {
                parsed >= 0.0
            },
        "basket index, divisor or rate is outside its domain"
    );
    // Preserve the extracted decimal token rather than reprinting a binary
    // float. Rust and Python both round this literal to the nearest binary64.
    Ok(format!("{}_f64", number.as_str()))
}

fn series(values: &Value, name: &str, epoch: u64, generated: u64) -> Result<(String, Vec<u64>)> {
    let points = values[name]["value"]["tuple"]
        .as_array()
        .with_context(|| format!("basket {name} is not a tuple"))?;
    ensure!(!points.is_empty(), "basket {name} is empty");
    let mut body = format!(
        "/// Frozen {name} samples as (block, level).\npub const {name}: &[(u64, f64)] = &[\n"
    );
    let mut blocks = Vec::new();
    for point in points {
        let point = tuple(point, 2)?;
        let block = integer(&point[0])?;
        ensure!(
            (epoch..=generated).contains(&block) && blocks.last().is_none_or(|last| *last < block),
            "basket {name} blocks are outside the frozen range or unordered"
        );
        body.push_str(&format!("({block}, {}),\n", float(&point[1], true)?));
        blocks.push(block);
    }
    ensure!(
        blocks.first() == Some(&epoch) && blocks.last() == Some(&generated),
        "basket {name} endpoints do not match its frozen bounds"
    );
    body.push_str("];\n");
    Ok((body, blocks))
}

fn render(values: &Value) -> Result<String> {
    let epoch = integer(&values["EPOCH_BLOCK"]["value"])?;
    let generated = integer(&values["GENERATED_BLOCK"]["value"])?;
    let step = integer(&values["SAMPLE_STEP"]["value"])?;
    ensure!(
        epoch <= generated && step > 0,
        "basket frozen bounds differ"
    );
    let mut body = format!(
        "//! Frozen basket indexes and fund baselines for the CLI display.\n\
/// First block in the frozen display index.\npub const EPOCH_BLOCK: u64 = {epoch};\n\
/// Block spacing of the frozen samples.\npub const SAMPLE_STEP: u64 = {step};\n\
/// Last block included in the frozen display index.\npub const GENERATED_BLOCK: u64 = {generated};\n"
    );
    let (index, blocks) = series(values, "INDEX", epoch, generated)?;
    let (total_return, total_return_blocks) = series(values, "TR_INDEX", epoch, generated)?;
    ensure!(
        blocks == total_return_blocks,
        "basket indexes use different block grids"
    );
    ensure!(
        blocks.windows(2).all(|pair| pair[1] - pair[0] == step),
        "basket frozen index grid differs from SAMPLE_STEP"
    );
    body.push_str(&index);
    body.push_str(&total_return);
    body.push_str("/// Fund hotkey, display divisor, first observed block and starting BasketRate.\npub const BASELINES: &[(&str, f64, u64, f64)] = &[\n");
    let baselines = values["BASELINES"]["value"]["dict"]
        .as_array()
        .context("basket baselines are not a literal dict")?;
    ensure!(!baselines.is_empty(), "basket baseline table is empty");
    let mut seen = BTreeSet::new();
    for entry in baselines {
        let entry = entry
            .as_array()
            .context("basket baseline entry is not a pair")?;
        ensure!(entry.len() == 2, "basket baseline pair arity differs");
        let key = entry[0].as_str().context("basket hotkey is not a string")?;
        ensure!(
            !key.is_empty() && seen.insert(key),
            "empty or duplicate basket hotkey"
        );
        let record = tuple(&entry[1], 3)?;
        let block = integer(&record[1])?;
        ensure!(
            (epoch..=generated).contains(&block),
            "baseline first block outside frozen range"
        );
        body.push_str(&format!(
            "({key:?}, {}, {block}, {}),\n",
            float(&record[0], true)?,
            float(&record[2], false)?
        ));
    }
    body.push_str("];\n");
    Ok(body)
}

pub(super) fn run(root: &Path, pins: &DocumentMut, check: bool) -> Result<()> {
    // Recompute the Rust-only extraction first: provenance strings alone do
    // not bind named_tables.json values to their authenticated Python AST.
    crate::extract::check_at(root)?;
    let source = root.join("vendor/upstream/src").join(SOURCE);
    ensure!(
        fs::symlink_metadata(&source)?.file_type().is_file(),
        "basket source must be regular"
    );
    let digest = hex::encode(Sha256::digest(fs::read(&source)?));
    ensure!(
        pins["artifacts"][SOURCE]["sha256"].as_str() == Some(digest.as_str()),
        "basket source differs from upstream pin"
    );
    let extracted = root.join("vendor/upstream/extracted/named_tables.json");
    ensure!(
        fs::symlink_metadata(&extracted)?.file_type().is_file(),
        "basket extraction must be regular"
    );
    let tables: Value = serde_json::from_slice(&fs::read(extracted)?)?;
    ensure!(tables["format"] == 1, "named-table format differs");
    let module = &tables["tables"][SOURCE];
    let commit = pins["parity"]["commit"]
        .as_str()
        .context("parity commit missing")?;
    ensure!(
        module["provenance"]["path"].as_str() == Some(SOURCE)
            && module["provenance"]["commit"].as_str() == Some(commit)
            && module["provenance"]["tag"].as_str() == pins["parity"]["tag"].as_str(),
        "basket extraction provenance differs from pin"
    );
    let header = format!(
        "// @generated by `cargo xtask regen`; do not edit.\n\
// Derived from RaoFoundation/subtensor {SOURCE} @ {commit} (Apache-2.0); modified by rtao.\n\
// Python source SHA256 {digest}.\n"
    );
    let output = format!(
        "{header}{}",
        super::format_rust(&render(&module["values"])?)?
    );
    let path = root.join(OUTPUT);
    let directory = path.parent().context("basket output parent")?;
    if !check {
        fs::create_dir_all(directory)?;
    }
    ensure!(
        fs::symlink_metadata(directory)?.file_type().is_dir(),
        "basket output parent must be a regular directory"
    );
    if path.exists() {
        ensure!(
            fs::symlink_metadata(&path)?.file_type().is_file(),
            "basket output must be regular"
        );
    }
    if fs::read_to_string(&path).ok().as_deref() != Some(output.as_str()) {
        ensure!(!check, "{OUTPUT} is stale; run cargo xtask regen");
        fs::write(path, output)?;
    }
    println!(
        "CLI basket indexes {}",
        if check {
            "match extracted tuples"
        } else {
            "regenerated"
        }
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn values() -> Value {
        json!({
            "EPOCH_BLOCK":{"value":10}, "GENERATED_BLOCK":{"value":20}, "SAMPLE_STEP":{"value":10},
            "INDEX":{"value":{"tuple":[{"tuple":[10,1.0]},{"tuple":[20,1.02]}]}},
            "TR_INDEX":{"value":{"tuple":[{"tuple":[10,1.0]},{"tuple":[20,1.001]}]}},
            "BASELINES":{"value":{"dict":[["fund",{"tuple":[1.02,10,0.0]}]]}}
        })
    }

    #[test]
    fn refuses_inconsistent_or_malformed_frozen_data() -> Result<()> {
        ensure!(render(&values())?.contains("pub const BASELINES"));
        for (pointer, replacement) in [
            ("/INDEX/value/tuple/1/tuple/0", json!(10)),
            ("/TR_INDEX/value/tuple/1/tuple/0", json!(19)),
            ("/SAMPLE_STEP/value", json!(5)),
            ("/BASELINES/value/dict/0/1/tuple/0", json!(0.0)),
            ("/BASELINES/value/dict/0/1/tuple/1", json!(21)),
            ("/BASELINES/value/dict/0/1/tuple/2", json!(-0.1)),
            ("/BASELINES/value/dict/0/1", json!({"tuple":[1,10]})),
        ] {
            let mut changed = values();
            *changed.pointer_mut(pointer).context("fixture pointer")? = replacement;
            ensure!(render(&changed).is_err(), "accepted malformed {pointer}");
        }
        let mut changed = values();
        let duplicate = changed["BASELINES"]["value"]["dict"][0].clone();
        changed["BASELINES"]["value"]["dict"]
            .as_array_mut()
            .context("fixture dict")?
            .push(duplicate);
        ensure!(render(&changed).is_err(), "accepted duplicate fund");
        Ok(())
    }
}
