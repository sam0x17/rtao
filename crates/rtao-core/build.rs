//! Generate the public core provenance constants from the canonical pin file.

use std::{env, error::Error, fs, path::PathBuf};

mod core_provenance;

fn main() -> Result<(), Box<dyn Error>> {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let workspace = manifest
        .parent()
        .and_then(|path| path.parent())
        .ok_or("rtao-core must be nested two directories below the workspace root")?;
    let pins = workspace.join("upstream.toml");
    println!("cargo:rerun-if-changed={}", pins.display());
    let document = fs::read_to_string(pins)?.parse::<toml_edit::DocumentMut>()?;
    let pin = core_provenance::effective(&document)?;
    println!("cargo:rerun-if-env-changed=RTAO_CORE_CANARY_DISPOSABLE");
    println!("cargo:rerun-if-env-changed=RTAO_TEMP_ROOT");
    println!("cargo:rerun-if-env-changed=GITHUB_ACTIONS");
    if !pin.branch.is_empty() {
        core_provenance::require_disposable(workspace)?;
    }
    let tag = pin.tag;
    let commit = pin.commit;
    let branch = pin.branch;
    let patch_commit = pin.patch_commit;
    let patch_source = pin.patch_source;
    // Both the source selector and reference are also consumed by the lock
    // integration test through the same parser.
    let _ = (pin.selector, pin.reference);
    let source = format!(
        "/// Upstream core release, or `canary:main` for a disposable canary build.\npub const BITTENSOR_CORE_TAG: &str = {tag:?};\n\
         /// Base upstream core commit, or resolved disposable canary commit; excludes a carried patch.\npub const BITTENSOR_CORE_COMMIT: &str = {commit:?};\n\
         /// Source branch resolved for a disposable canary; empty for a release build.\npub const BITTENSOR_CORE_BRANCH: &str = {branch:?};\n\
         /// Carried bounded core-patch revision; empty when using upstream directly.\npub const BITTENSOR_CORE_PATCH_COMMIT: &str = {patch_commit:?};\n\
         /// Complete Cargo Git source for the carried core patch; empty without a patch.\npub const BITTENSOR_CORE_PATCH_SOURCE: &str = {patch_source:?};\n"
    );
    fs::write(
        PathBuf::from(env::var("OUT_DIR")?).join("core_pin.rs"),
        source,
    )?;
    Ok(())
}
