//! Embed the actual compiled operator examples in the downloadable guides.

use std::{collections::BTreeMap, fs, path::Path};

use anyhow::Result;

// Proc-macro file reads alone do not establish Cargo input dependencies.
// Track each excerpt and template so a cached xtask cannot retain old examples.
const _: &str = include_str!("../../../crates/rtao/examples/validator.rs");
const _: &str = include_str!("../../../crates/rtao/examples/miner.rs");
const _: &str = include_str!("../../../crates/rtao/examples/owner.rs");
const _: &str = include_str!("../../../README.docify.md");

const README: &str = docify::compile_markdown!("../README.docify.md");
const VALIDATOR: &str = docify::embed!(
    "../crates/rtao/examples/validator.rs",
    validator_implementation
);
const MINER: &str = docify::embed!("../crates/rtao/examples/miner.rs", miner_implementation);
const HANDLER: &str = docify::embed!("../crates/rtao/examples/miner.rs", miner_handler);
const OWNER: &str = docify::embed!("../crates/rtao/examples/owner.rs", owner_configuration);

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
    Ok(pages)
}
