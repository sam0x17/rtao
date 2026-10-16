//! Embed shared offline tests and compiled operator examples in the guides.

use std::{collections::BTreeMap, fs, path::Path};

use anyhow::Result;

// Proc-macro file reads alone do not establish Cargo input dependencies.
// Track each excerpt and template so a cached xtask cannot retain old examples.
const _: &str = include_str!("../../../crates/rtao/examples/validator.rs");
const _: &str = include_str!("../../../crates/rtao/examples/miner.rs");
const _: &str = include_str!("../../../crates/rtao/examples/owner.rs");
const _: &str = include_str!("../../../crates/rtao/tests/documentation_examples.rs");
const _: &str = include_str!("../../../README.docify.md");

const README: &str = docify::compile_markdown!("../README.docify.md");
const VALIDATOR: &str = docify::embed!(
    "../crates/rtao/examples/validator.rs",
    validator_implementation
);
const MINER: &str = docify::embed!("../crates/rtao/examples/miner.rs", miner_implementation);
const HANDLER: &str = docify::embed!("../crates/rtao/examples/miner.rs", miner_handler);
const OWNER: &str = docify::embed!("../crates/rtao/examples/owner.rs", owner_configuration);
const NATIVE_BALANCES: &str = docify::embed_run!(
    "../crates/rtao/tests/documentation_examples.rs",
    checked_native_balances
);
const NUMBER_FORMATTING: &str = docify::embed_run!(
    "../crates/rtao/tests/documentation_examples.rs",
    python_compatible_number_formatting
);

const BTAUTH_VECTOR: &str = docify::embed_run!(
    "../crates/rtao/tests/documentation_examples.rs",
    btauth_payload_matches_committed_vector
);
const FAKECHAIN_READ: &str = docify::embed_run!(
    "../crates/rtao/tests/documentation_examples.rs",
    fakechain_balance_read_stays_on_one_snapshot
);
const INTENT_PLAN: &str = docify::embed_run!(
    "../crates/rtao/tests/documentation_examples.rs",
    transfer_plan_uses_public_keys_without_signing
);

pub(super) fn pages(root: &Path) -> Result<BTreeMap<String, String>> {
    let mut pages = BTreeMap::from([("README.md".into(), README.into())]);
    for (name, excerpts) in [
        ("validator", vec![VALIDATOR]),
        ("miner", vec![MINER, HANDLER]),
        ("owner", vec![OWNER]),
    ] {
        let mut text = fs::read_to_string(root.join(format!("book/src/{name}.md")))?;
        text.push_str("\n## Example implementation\n\nThese excerpts come from the source example named above, which CI compiles with its required features. They share the imports and setup in that complete example.\n\n");
        for excerpt in excerpts {
            text.push_str(excerpt);
            text.push_str("\n\n");
        }
        pages.insert(format!("{name}.md"), text);
    }
    let mut getting_started = fs::read_to_string(root.join("book/src/getting-started.md"))?;
    getting_started.push_str("\n## Offline examples\n\nThese examples are ordinary Rust tests in `crates/rtao/tests/documentation_examples.rs`. CI runs the same functions shown here.\n\n");
    for (heading, feature, excerpt) in [
        ("Checked native balances", None, NATIVE_BALANCES),
        (
            "Python-compatible number formatting",
            None,
            NUMBER_FORMATTING,
        ),
        ("Btauth payload vector", Some("btauth"), BTAUTH_VECTOR),
        (
            "A balance read through FakeChain",
            Some("reads"),
            FAKECHAIN_READ,
        ),
        ("A public transfer plan", Some("intents-all"), INTENT_PLAN),
    ] {
        getting_started.push_str(&format!("### {heading}\n\n"));
        if let Some(feature) = feature {
            getting_started.push_str(&format!("Requires the `{feature}` facade feature.\n\n"));
        }
        getting_started.push_str(excerpt);
        getting_started.push_str("\n\n");
    }
    pages.insert("getting-started.md".into(), getting_started);
    Ok(pages)
}
