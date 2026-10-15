//! Optional direct replay of hosted artifacts. `xtask oracle --check` also
//! verifies their manifest, complete sample inventory and authenticated H.
#![cfg(all(feature = "weights", feature = "serving", feature = "transfer"))]

#[path = "../../../xtask/src/oracle/replay.rs"]
mod adapter;

use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{collections::BTreeSet, fs, path::Path};

#[tokio::test]
#[ignore = "requires immutable vectors from the designated hosted Python oracle lane"]
async fn hosted_m2_intent_and_read_oracle_replay() -> Result<()> {
    let directory = std::env::var("RTAO_ORACLE_DIR")
        .context("RTAO_ORACLE_DIR must identify artifacts verified by xtask oracle --check")?;
    let directory = Path::new(&directory);
    adapter::timelock(&serde_json::from_slice(&fs::read(
        directory.join("pure/timelock.json"),
    )?)?)
    .await?;
    for (kind, expected) in [("intents", 9), ("reads", 15)] {
        let document: Value =
            serde_json::from_slice(&fs::read(directory.join(format!("{kind}.json")))?)?;
        ensure!(document["scope"] == "M2", "this test requires the M2 scope");
        let mut names = BTreeSet::new();
        for vector in document["vectors"].as_array().context("hosted vectors")? {
            let name = vector["name"].as_str().context("vector name")?;
            names.insert(name);
            let seeds: Value = serde_json::from_slice(&fs::read(
                directory.join("seeds").join(format!("{name}.json")),
            )?)?;
            let seed = seeds["samples"]
                .as_array()
                .context("seed samples")?
                .iter()
                .find(|row| {
                    row["sample"] == vector["sample"]
                        && (kind == "intents" || row["block"] == vector["block"])
                })
                .context("matching sample seed")?;
            if kind == "intents" {
                adapter::intent(vector, &seed["state"]).await?;
            } else {
                adapter::read(vector, &seed["state"]).await?;
            }
        }
        ensure!(
            names.len() == expected,
            "full native {kind} coverage is required"
        );
    }
    Ok(())
}
