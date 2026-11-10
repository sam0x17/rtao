// The pinned Finney codegen input (`upstream.toml`
// `[networks.finney.metadata]`): the V15 metadata the generated surface was
// generated from. Tests that compare the generated surface with metadata use
// it, so a runtime adoption that moves the pin moves them too (D-165);
// `pin --check` verifies these bytes against `v15_sha256`.

/// The pinned V15 metadata bytes and their spec version.
#[allow(clippy::expect_used, clippy::panic)]
pub fn pinned_metadata() -> &'static (Vec<u8>, u32) {
    static PINNED: std::sync::OnceLock<(Vec<u8>, u32)> = std::sync::OnceLock::new();
    PINNED.get_or_init(|| {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let pins = std::fs::read_to_string(root.join("upstream.toml"))
            .expect("the workspace's upstream.toml");
        let section: Vec<&str> = pins
            .split_once("\n[networks.finney.metadata]\n")
            .expect("upstream.toml pins Finney metadata")
            .1
            .lines()
            .take_while(|line| !line.starts_with('['))
            .collect();
        let field = |key: &str| {
            section
                .iter()
                .find_map(|line| {
                    let (name, value) = line.split_once('=')?;
                    (name.trim() == key).then(|| value.trim().trim_matches('"').to_owned())
                })
                .unwrap_or_else(|| panic!("pinned Finney metadata lacks {key}"))
        };
        let spec = field("spec").parse().expect("pinned spec is a number");
        let bytes = std::fs::read(root.join(field("v15"))).expect("pinned V15 metadata");
        (bytes, spec)
    })
}
