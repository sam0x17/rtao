//! Apply the pinned monorepo's Rust source lints to staged code.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use subtensor_linting::{
    ForbidAsPrimitiveConversion, ForbidKeysRemoveCall, ForbidSaturatingMath, Lint,
    RequireExplicitPalletIndex, RequireFreezeStruct,
};

fn source_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        // UP/build.rs excludes build artifacts, vendored dependencies and the
        // build script itself. Probe suites, examples and benches are source.
        if entry.file_name() == "target"
            || entry.file_name() == "vendor"
            || entry.file_name() == "build.rs"
        {
            continue;
        }
        let kind = entry.file_type()?;
        ensure!(
            !kind.is_symlink(),
            "staged source is a symlink: {}",
            entry.path().display()
        );
        if kind.is_dir() {
            source_files(&entry.path(), files)?;
        } else if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "rs")
        {
            files.push(entry.path());
        }
    }
    Ok(())
}

fn diagnostics(source: &str, is_test: bool) -> Result<Vec<String>> {
    let syntax = syn::parse_file(source)
        .context("cannot parse staged Rust; linting cannot skip malformed source")?;
    let mut errors = Vec::new();
    let mut collect = |result: subtensor_linting::Result| {
        if let Err(failures) = result {
            for error in failures {
                let location = error.span().start();
                errors.push(format!("{}:{}: {error}", location.line, location.column));
            }
        }
    };
    // These are the exact lint calls in UP/build.rs. Extrinsic benchmark
    // validation belongs to the monorepo workspace, which staged crates do not own.
    collect(ForbidAsPrimitiveConversion::lint(&syntax));
    collect(ForbidKeysRemoveCall::lint(&syntax));
    collect(RequireFreezeStruct::lint(&syntax));
    collect(RequireExplicitPalletIndex::lint(&syntax));
    if is_test {
        collect(ForbidSaturatingMath::lint(&syntax));
    }
    Ok(errors)
}

pub(crate) fn run(args: Vec<String>) -> Result<()> {
    ensure!(args.is_empty(), "usage: cargo xtask lint-upstream");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("workspace parent missing")?;
    let staged = root.join("crates/rtao-core-ext");
    let mut files = Vec::new();
    source_files(&staged, &mut files)?;
    files.sort();
    ensure!(!files.is_empty(), "staged source list is empty");
    let mut failures = Vec::new();
    for file in &files {
        let relative = file.strip_prefix(root)?;
        // The upstream rule keys this distinction from the repository-relative
        // filename. The task root may contain 'test' and must not affect it.
        for message in diagnostics(
            &fs::read_to_string(file)?,
            relative.to_string_lossy().contains("test"),
        )? {
            failures.push(format!("{}:{message}", relative.display()));
        }
    }
    ensure!(
        failures.is_empty(),
        "upstream lint failures:\n{}",
        failures.join("\n")
    );
    println!(
        "pinned subtensor-linting passed on {} staged Rust files",
        files.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitive_accessors_and_unfrozen_codec_structs_are_rejected() -> Result<()> {
        ensure!(!diagnostics("fn value(x: U256) -> u64 { x.as_u64() }", false)?.is_empty());
        ensure!(
            !diagnostics(
                "#[derive(Encode, Decode)] pub struct NeedsFreeze { value: u64 }",
                false
            )?
            .is_empty()
        );
        ensure!(diagnostics("fn value(x: u8) -> u64 { u64::from(x) }", false)?.is_empty());
        Ok(())
    }

    #[test]
    fn test_math_rule_and_parse_errors_cannot_disappear() -> Result<()> {
        let source = "fn value(x: u64) -> u64 { x.saturating_add(1) }";
        ensure!(diagnostics(source, false)?.is_empty());
        ensure!(!diagnostics(source, true)?.is_empty());
        ensure!(diagnostics("fn broken {", false).is_err());
        Ok(())
    }

    #[test]
    fn probes_examples_and_benches_are_linted_with_upstream_exclusions() -> Result<()> {
        let temporary = std::env::var_os("RTAO_TEMP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target"));
        let fixture = temporary.join(format!("lint-source-test-{}", std::process::id()));
        fs::create_dir_all(&fixture)?;
        let result = (|| -> Result<()> {
            for directory in [
                "src", "tests", "probes", "examples", "benches", "target", "vendor",
            ] {
                fs::create_dir_all(fixture.join(directory))?;
                fs::write(
                    fixture.join(directory).join("source.rs"),
                    "fn value(x: U256) -> u64 { x.as_u64() }",
                )?;
            }
            fs::write(fixture.join("build.rs"), "fn broken {")?;
            let mut files = Vec::new();
            source_files(&fixture, &mut files)?;
            ensure!(files.len() == 5);
            for file in files {
                ensure!(!diagnostics(&fs::read_to_string(file)?, false)?.is_empty());
            }
            Ok(())
        })();
        // The fixture contains source only and has no processes, services,
        // symlinks, virtual environments or external configuration dependencies.
        fs::remove_dir_all(&fixture)?;
        result
    }
}
