//! Offline checks for final workspaces consuming the SDK.
//!
//! rtao depends on the core through its republished `rtao-upstream-*`
//! packages (D-183), so a consumer needs no `[patch]`. The doctor checks that
//! the consumer's lock resolves the republished core and BLS fork at the
//! versions its `rtao-core` records, with the recorded crates.io checksums,
//! and that the build graph compiles exactly one BLS implementation.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::Value as Json;
use toml_edit::{DocumentMut, Item};

use rtao::core::dependency_policy::{
    BLS_PACKAGE, CORE_PACKAGE, CRATES_IO, LockedPackage, validate_bls_lock, validate_bls_tree,
};

fn document(path: &Path) -> Result<DocumentMut, String> {
    fs::read_to_string(path)
        .map_err(|error| format!("read {}: {error}", path.display()))?
        .parse()
        .map_err(|error| format!("parse {}: {error}", path.display()))
}

fn cargo_output(command: &mut Command) -> Result<String, String> {
    let output = command
        .output()
        .map_err(|error| format!("run Cargo to inspect the workspace: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Cargo could not inspect the workspace offline:\n{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|error| format!("Cargo output is not UTF-8: {error}"))
}

/// Ask Cargo to identify the package selected by the current directory.
pub(super) fn find_manifest(directory: &Path) -> Result<PathBuf, String> {
    let output = cargo_output(Command::new("cargo").current_dir(directory).args([
        "locate-project",
        "--offline",
        "--message-format",
        "plain",
    ]))?;
    Ok(PathBuf::from(output.trim()))
}

fn workspace_manifest(manifest: &Path) -> Result<PathBuf, String> {
    if manifest.file_name().is_none_or(|name| name != "Cargo.toml") {
        return Err("--manifest-path must name Cargo.toml".into());
    }
    let output = cargo_output(
        Command::new("cargo")
            .args([
                "locate-project",
                "--workspace",
                "--offline",
                "--message-format",
                "plain",
                "--manifest-path",
            ])
            .arg(manifest),
    )?;
    Ok(PathBuf::from(output.trim()))
}

fn metadata(manifest: &Path) -> Result<Json, String> {
    let mut command = Command::new("cargo");
    command.current_dir(
        manifest
            .parent()
            .ok_or("Cargo.toml has no parent directory")?,
    );
    command
        .args([
            "metadata",
            "--offline",
            "--locked",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(manifest);
    serde_json::from_str(&cargo_output(&mut command)?)
        .map_err(|error| format!("parse Cargo workspace metadata: {error}"))
}

/// The record the workspace's republished packages are checked against: the
/// workspace's own `upstream.toml` (rtao's checkout), else the copy inside
/// the `rtao-core` the workspace resolves, which that crate's provenance
/// constants are built from.
fn core_record(root: &Path, manifest_path: &Path) -> Result<DocumentMut, String> {
    let local = root.join("upstream.toml");
    if local.is_file() {
        return document(&local);
    }
    let metadata = metadata(manifest_path).map_err(|error| {
        format!("the expected core versions are recorded in the resolved rtao-core's upstream.toml; inspecting it requires dependencies cached for offline Cargo metadata:\n{error}")
    })?;
    let records = metadata
        .get("packages")
        .and_then(Json::as_array)
        .ok_or("Cargo metadata has no packages")?
        .iter()
        .filter(|package| package.get("name").and_then(Json::as_str) == Some("rtao-core"))
        .map(|package| {
            package
                .get("manifest_path")
                .and_then(Json::as_str)
                .map(Path::new)
                .and_then(Path::parent)
                .map(|crate_root| crate_root.join("upstream.toml"))
                .ok_or("rtao-core metadata has no manifest path")
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let mut records = records.into_iter();
    match (records.next(), records.next()) {
        (Some(record), None) => document(&record),
        (None, _) => Err("the workspace resolves no rtao-core, so it has no record of the core versions to check".into()),
        (Some(_), Some(_)) => Err("workspace resolves more than one rtao-core; the core versions have no single record".into()),
    }
}

/// One republished package's recorded identity.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Republished {
    package: String,
    version: String,
    checksum: Option<String>,
}

fn republished(record: &DocumentMut, key: &str) -> Result<Republished, String> {
    let entry = record
        .get("publish")
        .and_then(|publish| publish.get(key))
        .ok_or_else(|| format!("upstream.toml has no [publish.{key}]; the resolved rtao-core predates the republished core (D-183)"))?;
    let text = |field: &str| {
        entry
            .get(field)
            .and_then(Item::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("upstream.toml [publish.{key}] has no {field}"))
    };
    Ok(Republished {
        package: text("package")?,
        version: text("version")?,
        checksum: entry
            .get("checksum")
            .and_then(Item::as_str)
            .map(str::to_owned),
    })
}

/// Every republished package the record names.
fn recorded(record: &DocumentMut) -> Result<Vec<Republished>, String> {
    let keys = record
        .get("publish")
        .and_then(Item::as_table_like)
        .ok_or("upstream.toml has no [publish]; the resolved rtao-core predates the republished core (D-183)")?
        .iter()
        .filter(|(_, item)| item.get("package").is_some())
        .map(|(key, _)| key.to_owned())
        .collect::<Vec<_>>();
    keys.iter().map(|key| republished(record, key)).collect()
}

/// Patches an earlier rtao revision asked for, which no longer apply.
fn obsolete_patches(manifest: &DocumentMut) -> Vec<String> {
    let Some(patch) = manifest.get("patch").and_then(Item::as_table_like) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for (source, table) in patch.iter() {
        let Some(table) = table.as_table_like() else {
            continue;
        };
        for (name, _) in table.iter() {
            if [
                "w3f-bls",
                "bittensor-core",
                "subtensor-hashed",
                "subtensor-macros",
                "fips204",
                "ruint",
            ]
            .contains(&name)
            {
                found.push(format!("[patch.{source:?}] {name}"));
            }
        }
    }
    found
}

fn locked(lock: &DocumentMut) -> Result<Vec<LockedPackage<'_>>, String> {
    let Some(packages) = lock.get("package") else {
        return Ok(Vec::new());
    };
    packages
        .as_array_of_tables()
        .ok_or("Cargo.lock package entries are not an array of tables")?
        .iter()
        .map(|package| {
            Ok(LockedPackage {
                name: package
                    .get("name")
                    .and_then(Item::as_str)
                    .ok_or("Cargo.lock package has no name")?,
                version: package
                    .get("version")
                    .and_then(Item::as_str)
                    .ok_or("Cargo.lock package has no version")?,
                source: package.get("source").and_then(Item::as_str),
                checksum: package.get("checksum").and_then(Item::as_str),
            })
        })
        .collect()
}

/// The lock resolves each recorded republished package once, at its version,
/// and a crates.io copy has the recorded checksum. Returns the BLS version.
fn verify_lock(lock: &DocumentMut, record: &DocumentMut) -> Result<String, String> {
    let packages = locked(lock)?;
    let expected = recorded(record)?;
    for package in &expected {
        let matching: Vec<_> = packages
            .iter()
            .filter(|locked| locked.name == package.package)
            .collect();
        if matching.len() != 1 {
            return Err(format!(
                "Cargo.lock must contain exactly one {}; found {}. Run `cargo update -p {}` after changing the rtao version.",
                package.package,
                matching.len(),
                package.package
            ));
        }
        let locked = matching[0];
        if locked.version != package.version {
            return Err(format!(
                "Cargo.lock {} is {}, but the resolved rtao-core records {}",
                package.package, locked.version, package.version
            ));
        }
        if locked.source == Some(CRATES_IO)
            && let Some(checksum) = &package.checksum
            && locked.checksum != Some(checksum.as_str())
        {
            return Err(format!(
                "Cargo.lock {} {} has checksum {:?}, but rtao recorded {checksum} for the published package",
                package.package, package.version, locked.checksum
            ));
        }
    }
    if !expected
        .iter()
        .any(|package| package.package == CORE_PACKAGE)
    {
        return Err("upstream.toml [publish] does not record the core".into());
    }
    let bls = expected
        .iter()
        .find(|package| package.package == BLS_PACKAGE)
        .ok_or("upstream.toml [publish] does not record the BLS fork")?;
    validate_bls_lock(&packages, &bls.version)?;
    Ok(bls.version.clone())
}

fn built_tree_command(manifest: &Path) -> Command {
    let mut command = Command::new("cargo");
    if let Some(root) = manifest.parent() {
        command.current_dir(root);
    }
    // Cargo must resolve the consumer's complete build graph. `--locked` and
    // `--offline` keep this inspection from updating its lock or fetching code.
    command
        // Cargo decorates repeated-node markers when the caller forces color;
        // dependency identities must be parsed from plain captured output.
        .env("CARGO_TERM_COLOR", "never")
        .args([
            "tree",
            "--locked",
            "--offline",
            "--workspace",
            "--all-features",
            "--target",
            "all",
            "--edges",
            "normal,build",
            "--prefix",
            "none",
            "--format",
            "{p}",
            "--manifest-path",
        ])
        .arg(manifest);
    command
}

fn verify_built(manifest: &Path, bls_version: &str) -> Result<(), String> {
    let tree = cargo_output(&mut built_tree_command(manifest))?;
    validate_bls_tree(&tree, bls_version)
}

/// The verified workspace and any notes that do not fail the check.
pub(super) struct Check {
    pub(super) manifest: PathBuf,
    pub(super) warning: Option<String>,
}

fn warnings(manifest: &DocumentMut, lock: &DocumentMut, record: &DocumentMut) -> Option<String> {
    let mut notes = Vec::new();
    let obsolete = obsolete_patches(manifest);
    if !obsolete.is_empty() {
        notes.push(format!(
            "remove {} from the root Cargo.toml: rtao depends on the republished core and needs no [patch] (D-183)",
            obsolete.join(", ")
        ));
    }
    if locked(lock).is_ok_and(|packages| {
        packages
            .iter()
            .any(|package| package.name == "bittensor-core")
    }) {
        notes.push(
            "the workspace also resolves bittensor-core itself; rtao uses rtao-upstream-core, and the two are separate copies".into(),
        );
    }
    if republished(record, "core")
        .is_ok_and(|core| core.version != rtao::core::UPSTREAM_CORE_VERSION)
    {
        notes.push(format!(
            "the workspace's rtao records {CORE_PACKAGE} {}, this rtao binary {}; the workspace's own record was verified",
            republished(record, "core")
                .map(|core| core.version)
                .unwrap_or_default(),
            rtao::core::UPSTREAM_CORE_VERSION
        ));
    }
    (!notes.is_empty()).then(|| notes.join("; "))
}

pub(super) fn check(manifest_path: &Path) -> Result<Check, String> {
    let manifest_path = workspace_manifest(manifest_path)?;
    let manifest = document(&manifest_path)?;
    let root = manifest_path
        .parent()
        .ok_or("Cargo.toml has no parent directory")?;
    let lock = document(&root.join("Cargo.lock"))?;
    let record = core_record(root, &manifest_path)?;
    let bls_version = verify_lock(&lock, &record)?;
    verify_built(&manifest_path, &bls_version)?;
    Ok(Check {
        warning: warnings(&manifest, &lock, &record),
        manifest: manifest_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use rtao::core::dependency_policy::DORMANT_BLS_CHECKSUM;

    const VERSION: &str = "0.490.0";
    const SUM: &str = "1111111111111111111111111111111111111111111111111111111111111111";

    fn parse(value: &str) -> Result<DocumentMut, String> {
        value
            .parse()
            .map_err(|error: toml_edit::TomlError| error.to_string())
    }

    fn record(checksum: bool) -> Result<DocumentMut, String> {
        let mut text = String::from("[publish]\n");
        for key in ["core", "w3f-bls"] {
            text.push_str(&format!(
                "[publish.{key}]\npackage = \"rtao-upstream-{key}\"\nversion = {VERSION:?}\n"
            ));
            if checksum {
                text.push_str(&format!("checksum = {SUM:?}\n"));
            }
        }
        parse(&text)
    }

    fn lock(source: Option<&str>, checksum: Option<&str>) -> Result<DocumentMut, String> {
        let mut text = String::from("version = 4\n");
        for name in [CORE_PACKAGE, BLS_PACKAGE] {
            text.push_str(&format!(
                "[[package]]\nname = {name:?}\nversion = {VERSION:?}\n"
            ));
            if let Some(source) = source {
                text.push_str(&format!("source = {source:?}\n"));
            }
            if let Some(checksum) = checksum {
                text.push_str(&format!("checksum = {checksum:?}\n"));
            }
        }
        parse(&text)
    }

    #[test]
    fn republished_packages_resolve_once_at_the_recorded_version() -> Result<(), String> {
        assert_eq!(verify_lock(&lock(None, None)?, &record(true)?)?, VERSION);
        assert_eq!(
            verify_lock(&lock(Some(CRATES_IO), Some(SUM))?, &record(true)?)?,
            VERSION
        );
        // Without a recorded checksum a crates.io copy is accepted by version.
        verify_lock(&lock(Some(CRATES_IO), Some("other"))?, &record(false)?)?;
        for (changed, expected) in [
            (lock(Some(CRATES_IO), Some("other"))?, "checksum"),
            (
                parse(&lock(None, None)?.to_string().replace(VERSION, "0.490.1"))?,
                "records",
            ),
            (
                parse(&format!(
                    "{}[[package]]\nname = {CORE_PACKAGE:?}\nversion = \"0.489.0\"\n",
                    lock(None, None)?
                ))?,
                "exactly one",
            ),
            (parse("version = 4\n")?, "exactly one"),
        ] {
            let error = verify_lock(&changed, &record(true)?)
                .err()
                .ok_or("changed lock accepted")?;
            assert!(error.contains(expected), "{error}");
        }
        assert!(verify_lock(&lock(None, None)?, &parse("[core]\n")?).is_err());
        Ok(())
    }

    #[test]
    fn only_the_recorded_dormant_bls_may_join_the_republished_one() -> Result<(), String> {
        let base = lock(None, None)?.to_string();
        let dormant = |checksum: &str| {
            format!(
                "[[package]]\nname = \"w3f-bls\"\nversion = \"0.1.9\"\nsource = {CRATES_IO:?}\nchecksum = {checksum:?}\n"
            )
        };
        verify_lock(
            &parse(&format!("{base}{}", dormant(DORMANT_BLS_CHECKSUM)))?,
            &record(false)?,
        )?;
        for changed in [
            format!("{base}{}", dormant("tampered")),
            format!(
                "{base}{}{}",
                dormant(DORMANT_BLS_CHECKSUM),
                dormant(DORMANT_BLS_CHECKSUM)
            ),
            format!(
                "{base}[[package]]\nname = \"w3f-bls\"\nversion = \"0.1.3\"\nsource = \"git+https://github.com/RaoFoundation/subtensor.git?tag=v490#d967d13588834f4f5aaa8ef4c8fc1a865d581462\"\n"
            ),
        ] {
            assert!(verify_lock(&parse(&changed)?, &record(false)?).is_err());
        }
        Ok(())
    }

    #[test]
    fn obsolete_patches_and_a_second_core_are_warnings() -> Result<(), String> {
        let manifest = parse(
            "[patch.crates-io]\nw3f-bls = { git = \"https://github.com/RaoFoundation/subtensor.git\", tag = \"v490\" }\nserde = { path = \"serde\" }\n",
        )?;
        assert_eq!(
            obsolete_patches(&manifest),
            ["[patch.\"crates-io\"] w3f-bls"]
        );
        assert!(obsolete_patches(&parse("[workspace]\n")?).is_empty());
        let mut core_record = record(false)?;
        core_record["publish"]["core"]["version"] =
            toml_edit::value(rtao::core::UPSTREAM_CORE_VERSION);
        assert!(warnings(&parse("[workspace]\n")?, &lock(None, None)?, &core_record).is_none());
        let warning = warnings(&manifest, &lock(None, None)?, &core_record)
            .ok_or("obsolete patch not reported")?;
        assert!(warning.contains("needs no [patch]"));
        let with_core = parse(&format!(
            "{}[[package]]\nname = \"bittensor-core\"\nversion = \"0.1.0\"\n",
            lock(None, None)?
        ))?;
        assert!(
            warnings(&parse("[workspace]\n")?, &with_core, &core_record)
                .is_some_and(|warning| warning.contains("separate copies"))
        );
        Ok(())
    }

    #[test]
    fn build_inspection_uses_consumer_workspace_without_lock_updates() -> Result<(), String> {
        let manifest = Path::new("/consumer/workspace/Cargo.toml");
        let command = built_tree_command(manifest);
        assert_eq!(command.get_current_dir(), manifest.parent());
        assert!(command.get_envs().any(|(name, value)| {
            name == "CARGO_TERM_COLOR" && value == Some(std::ffi::OsStr::new("never"))
        }));
        let arguments = command
            .get_args()
            .map(|argument| argument.to_str().ok_or("non-UTF8 argument"))
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(
            arguments,
            [
                "tree",
                "--locked",
                "--offline",
                "--workspace",
                "--all-features",
                "--target",
                "all",
                "--edges",
                "normal,build",
                "--prefix",
                "none",
                "--format",
                "{p}",
                "--manifest-path",
                "/consumer/workspace/Cargo.toml"
            ]
        );
        Ok(())
    }

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Result<Self, String> {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let base = std::env::var_os("RTAO_TEMP_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let path = base.join(format!(
                "doctor-fixture-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).map_err(|error| error.to_string())?;
            Ok(Self(path))
        }

        fn write(&self, path: &str, contents: &str) -> Result<(), String> {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().ok_or("fixture path has no parent")?)
                .map_err(|error| error.to_string())?;
            fs::write(path, contents).map_err(|error| error.to_string())
        }

        fn package(
            &self,
            path: &str,
            name: &str,
            workspace: &str,
            extra: &str,
        ) -> Result<(), String> {
            self.write(&format!("{path}/Cargo.toml"), &format!("[package]\nname = {name:?}\nversion = \"0.0.0\"\nedition = \"2024\"\n{workspace}\n{extra}"))?;
            self.write(&format!("{path}/src/lib.rs"), "")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            // These fixtures contain only manifests and empty Rust sources;
            // all child Cargo commands have exited before the guard is dropped.
            if let Err(error) = fs::remove_dir_all(&self.0) {
                eprintln!("retain doctor fixture {}: {error}", self.0.display());
            }
        }
    }

    #[test]
    fn actual_consumer_tree_exposes_optional_and_cross_target_build_bls() -> Result<(), String> {
        let fixture = Fixture::new()?;
        fixture.write(
            "Cargo.toml",
            "[workspace]\nmembers = [\"consumer\"]\nexclude = [\"bls\"]\nresolver = \"2\"\n",
        )?;
        fixture.package("bls", "w3f-bls", "", "[workspace]\n")?;
        // The local package is intentionally an unexpected BLS source. It must
        // appear in the inspected graph and be rejected, including when only an
        // optional feature or a non-host build edge makes it reachable.
        for extra in [
            "[features]\nactivate = [\"dep:w3f-bls\"]\n[dependencies]\nw3f-bls = { path = \"../bls\", optional = true }\n",
            "[target.'cfg(target_os = \"none\")'.build-dependencies]\nw3f-bls = { path = \"../bls\" }\n",
        ] {
            fixture.package("consumer", "doctor-consumer", "", extra)?;
            let manifest = fixture.0.join("Cargo.toml");
            cargo_output(
                Command::new("cargo")
                    .current_dir(&fixture.0)
                    .args(["generate-lockfile", "--offline", "--manifest-path"])
                    .arg(&manifest),
            )?;
            let before =
                fs::read(fixture.0.join("Cargo.lock")).map_err(|error| error.to_string())?;
            let tree = cargo_output(&mut built_tree_command(&manifest))?;
            assert!(tree.lines().any(|line| line.starts_with("w3f-bls ")));
            assert!(validate_bls_tree(&tree, VERSION).is_err());
            assert_eq!(
                before,
                fs::read(fixture.0.join("Cargo.lock")).map_err(|error| error.to_string())?
            );
        }
        Ok(())
    }

    #[test]
    fn record_comes_from_the_resolved_rtao_core() -> Result<(), String> {
        let fixture = Fixture::new()?;
        let record = format!("[publish.core]\npackage = {CORE_PACKAGE:?}\nversion = {VERSION:?}\n");
        fixture.package("sdk/crates/rtao-core", "rtao-core", "[workspace]\n", "")?;
        fixture.write("sdk/crates/rtao-core/upstream.toml", &record)?;
        fixture.package(
            "app",
            "doctor-app",
            "[workspace]\n",
            "[dependencies]\nrtao-core = { path = \"../sdk/crates/rtao-core\" }\n",
        )?;
        let app = fixture.0.join("app");
        let lock = |directory: &Path| {
            cargo_output(
                Command::new("cargo")
                    .current_dir(directory)
                    .args(["generate-lockfile", "--offline"]),
            )
        };
        lock(&app)?;
        let found = core_record(&app, &app.join("Cargo.toml"))?;
        assert_eq!(found.to_string(), record);
        // A workspace's own record wins.
        fixture.write("app/upstream.toml", "[core]\n")?;
        assert_eq!(
            core_record(&app, &app.join("Cargo.toml"))?.to_string(),
            "[core]\n"
        );
        // Without either, there is nothing to check against.
        fixture.package("bare", "doctor-bare", "[workspace]\n", "")?;
        let bare = fixture.0.join("bare");
        lock(&bare)?;
        assert!(core_record(&bare, &bare.join("Cargo.toml")).is_err());
        Ok(())
    }

    #[test]
    fn cargo_resolves_explicit_workspace_and_excluded_package() -> Result<(), String> {
        let fixture = Fixture::new()?;
        fixture.write(
            "parent/Cargo.toml",
            "[workspace]\nmembers = [\"member\", \"../detached\"]\nexclude = [\"excluded\"]\n",
        )?;
        fixture.package("parent/member", "doctor-member", "", "")?;
        fixture.package("parent/excluded", "doctor-excluded", "", "")?;
        fixture.package(
            "detached",
            "doctor-detached",
            "workspace = \"../parent\"",
            "",
        )?;
        assert_eq!(
            workspace_manifest(&fixture.0.join("detached/Cargo.toml"))?,
            fixture.0.join("parent/Cargo.toml")
        );
        // Without a standalone workspace this nested fixture is also beneath
        // the repository workspace, so Cargo correctly rejects ownership.
        assert!(workspace_manifest(&fixture.0.join("parent/excluded/Cargo.toml")).is_err());
        fixture.package("parent/excluded", "doctor-excluded", "", "[workspace]\n")?;
        assert_eq!(
            workspace_manifest(&fixture.0.join("parent/excluded/Cargo.toml"))?,
            fixture.0.join("parent/excluded/Cargo.toml")
        );
        let selected = find_manifest(&fixture.0.join("parent/member/src"))?;
        assert_eq!(selected, fixture.0.join("parent/member/Cargo.toml"));
        assert_eq!(
            workspace_manifest(&selected)?,
            fixture.0.join("parent/Cargo.toml")
        );
        Ok(())
    }
}
