//! Narrow provenance and retained-notice gate for the CLI difflib port.

use std::{
    fs,
    path::{Component, Path},
};

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};

const PACKAGE: &str = "rtao-cli";
const SOURCE: &str = "crates/rtao-cli/src/books.rs";
const LICENSE: &str = "crates/rtao-cli/LICENSE-PSF";
const PSF_SHA256: &str = "3b2f81fe21d181c499c59a256c8e1968455d6689d269aa85373bfb6af41da3bf";
pub(super) const PROVENANCE: &str = "// Derived from CPython 3.12.12 Lib/difflib.py @ 4a5632fbf9bf59477c540e3f53fa7cdbeea3e3f5 (PSF-2.0); modified by rtao.";
pub(super) const NOTICE: &str = "CPython difflib SequenceMatcher\n\n\
The CLI name suggestions in crates/rtao-cli/src/books.rs include SequenceMatcher\n\
logic derived from CPython 3.12.12 Lib/difflib.py at commit\n\
4a5632fbf9bf59477c540e3f53fa7cdbeea3e3f5 (source SHA256\n\
3eb2d371b4a171b063921de6bbf115108bc39de8bf72866315868ce2599213b7).\n\
Copyright Python Software Foundation and contributors. The full license is\n\
preserved in crates/rtao-cli/LICENSE-PSF; the package declares PSF-2.0.\n\
Modified by rtao: Rust character matching with no-junk/autojunk behavior and\n\
ratio computation for offline CLI name suggestions. The byte-identical full\n\
PSF license already retained for the datetime port covers both distributed\n\
ports in the release notice bundle.\n\n";

fn regular_bytes(root: &Path, relative: &str) -> Result<Vec<u8>> {
    let mut path = root.to_owned();
    for component in Path::new(relative).components() {
        ensure!(
            matches!(component, Component::Normal(_)),
            "invalid CLI PSF attribution path"
        );
        path.push(component);
        ensure!(
            !fs::symlink_metadata(&path)?.file_type().is_symlink(),
            "CLI PSF attribution path must not be a symlink: {}",
            path.display()
        );
    }
    let metadata = fs::symlink_metadata(&path)?;
    ensure!(
        metadata.is_file() && metadata.len() <= 8 * 1024 * 1024,
        "CLI PSF attribution requires bounded regular files: {}",
        path.display()
    );
    fs::read(path).context("read CLI PSF attribution input")
}

/// Recognize only the identified package, file, immutable source and function.
///
/// All other CPython derived-header claims fail closed. The existing datetime
/// and Unicode predicates retain their separate source contracts.
pub(super) fn source(
    root: &Path,
    package: &str,
    directory: &Path,
    path: &Path,
    source: &str,
) -> Result<bool> {
    let headers: Vec<_> = source
        .lines()
        .filter(|line| line.trim_start().starts_with("// Derived from CPython"))
        .collect();
    let expected = package == PACKAGE
        && directory == root.join("crates/rtao-cli")
        && path == root.join(SOURCE);
    if headers.is_empty() && !expected {
        return Ok(false);
    }
    let parsed = syn::parse_file(source).context("parse claimed CLI CPython source")?;
    let functions: Vec<_> = parsed
        .items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Fn(function) if function.sig.ident == "sequence_ratio" => Some(function),
            _ => None,
        })
        .collect();
    if headers.is_empty() && functions.is_empty() {
        return Ok(false);
    }
    ensure!(
        expected,
        "CPython difflib attribution is supported only for rtao-cli at {SOURCE}"
    );
    let (apache_source, _) = super::attribution(source)?
        .context("identified CLI book port must retain its canonical primary Apache attribution")?;
    ensure!(
        apache_source == "sdk/python/bittensor/cli/context.py",
        "identified CLI book port requires its exact primary upstream source path"
    );
    ensure!(
        headers.len() == 1 && headers[0] == PROVENANCE,
        "CLI SequenceMatcher must retain its exact CPython version/path/commit/license provenance"
    );
    ensure!(
        functions.len() == 1 && functions[0].sig.inputs.len() == 2,
        "CLI CPython attribution must identify the actual sequence_ratio function"
    );
    let following = source
        .lines()
        .skip_while(|line| *line != PROVENANCE)
        .skip(1)
        .find(|line| !line.trim().is_empty() && !line.trim_start().starts_with("//"));
    ensure!(
        following == Some("pub(crate) fn sequence_ratio(left: &str, right: &str) -> f64 {"),
        "CLI CPython provenance must immediately precede the identified function signature"
    );
    ensure!(
        regular_bytes(root, SOURCE)? == source.as_bytes(),
        "CLI CPython source changed during attribution scan"
    );
    let retained = regular_bytes(root, LICENSE)?;
    ensure!(
        format!("{:x}", Sha256::digest(&retained)) == PSF_SHA256,
        "CLI port requires its exact full retained PSF license"
    );
    let distribution = crate::release::LICENSES
        .iter()
        .find(|license| license.name == "LICENSE-PSF")
        .context("release PSF notice asset absent")?;
    crate::release::validate_license_bytes(distribution, &retained)?;
    let notice = regular_bytes(root, "NOTICE")?;
    ensure!(
        std::str::from_utf8(&notice)?.contains(NOTICE),
        "NOTICE must retain the identified CLI CPython source, package, license path and modification summary"
    );
    Ok(true)
}
