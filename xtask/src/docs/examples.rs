//! Embed offline tests, operator examples and shared e2e items in the guides.

use std::{collections::BTreeMap, fs, path::Path};

use anyhow::Result;

// Proc-macro file reads alone do not establish Cargo input dependencies.
// Track each excerpt and template so a cached xtask cannot retain old examples.
const _: &str = include_str!("../../../crates/rtao/examples/validator.rs");
const _: &str = include_str!("../../../crates/rtao/examples/miner.rs");
const _: &str = include_str!("../../../crates/rtao/examples/owner.rs");
const _: &str = include_str!("../../../crates/rtao/tests/documentation_examples.rs");
const _: &str = include_str!("../../../README.docify.md");
const _: &str = include_str!("../../../crates/rtao-testkit/tests/e2e/node_examples.rs");
const _: &str = include_str!("../../../crates/rtao-testkit/tests/e2e.rs");

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

const LIVE_NEURONS_DESCRIPTOR: &str = docify::embed!(
    "../crates/rtao-testkit/tests/e2e/node_examples.rs",
    live_neurons_descriptor
);
const LIVE_TYPED_READS: &str = docify::embed!(
    "../crates/rtao-testkit/tests/e2e/node_examples.rs",
    live_typed_reads
);
const LIVE_TYPED_TRANSFER: &str = docify::embed!(
    "../crates/rtao-testkit/tests/e2e/node_examples.rs",
    live_typed_transfer
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
    getting_started.push_str("\n## Node-dependent examples\n\nThese items come from `crates/rtao-testkit/tests/e2e/node_examples.rs`. The existing e2e tests call these same functions. The complete source file supplies their imports; callers supply a connected client or executor and a coldkey source. The caller owns connection cleanup.\n\nOrdinary hosted CI compiles the all-feature e2e target and embeds its source; the ordinary test profile excludes e2e execution. An optional targeted hosted check can run `test_generic_accessors_over_generated_descriptors` or `scenario_17_two_executors_fifty_transfers` against its separately bootstrapped endpoint and disposable development accounts. Until a matching run is recorded, node execution remains unverified. No local node or testnet setup is required under Q30.\n\n### Typed reads from one live snapshot\n\nThe generic runtime descriptor and read function are shared with the e2e accessor test. The caller selects an existing subnet; the test uses its bootstrapped subnet 1.\n\n");
    for excerpt in [LIVE_NEURONS_DESCRIPTOR, LIVE_TYPED_READS] {
        getting_started.push_str(excerpt);
        getting_started.push_str("\n\n");
    }
    getting_started.push_str("### A typed transfer through a shared executor\n\nThe transfer returns its receipt for the caller to check. Its existing e2e scenario uses two executors sharing one client to submit fifty transfers, then checks finalized outcomes, distinct inclusions, every nonce and the exact receiver balance increase. The documentation function makes one transfer per call; it does not create accounts or a node.\n\n");
    getting_started.push_str(LIVE_TYPED_TRANSFER);
    getting_started.push_str("\n\n");
    pages.insert("getting-started.md".into(), getting_started);
    Ok(pages)
}
