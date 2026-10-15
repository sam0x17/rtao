//! Raw lockfile audits with exact exceptions limited to dormant dependencies.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{Context, Result, ensure};
use serde_json::{Value as Json, json};
use toml_edit::DocumentMut;

const ADVISORY: &str = "RUSTSEC-2026-0253";
const SOURCE: &str = "registry+https://github.com/rust-lang/crates.io-index";
const CHECKSUM: &str = "7f66e8d5d03f609abc3a39e6f08e4164ebf1447a732906d39eb9b99b7919ef39";
const FIXED: &str = "0.18.2";
const RUZSTD_ADVISORY: &str = "RUSTSEC-2026-0344";
const RUZSTD_CHECKSUM: &str = "a7c1c839d570d835527c9a5e4db7cb2198683a988cb9d7293fc8674e6bd58fc8";

struct DormantException {
    name: &'static str,
    version: &'static str,
    checksum: &'static str,
    advisory: &'static str,
    fixed: &'static str,
}

const EXCEPTIONS: &[DormantException] = &[
    DormantException {
        name: "lru",
        version: "0.16.4",
        checksum: CHECKSUM,
        advisory: ADVISORY,
        fixed: FIXED,
    },
    DormantException {
        name: "ruzstd",
        version: "0.8.3",
        checksum: RUZSTD_CHECKSUM,
        advisory: RUZSTD_ADVISORY,
        fixed: "0.9.1",
    },
];
// These existing maintenance notices remain visible and nonblocking. Any new
// notice requires investigation rather than inheriting a blanket allowance.
const MAINTENANCE: &[(&str, &str)] = &[
    ("RUSTSEC-2021-0061", "aes-ctr"),
    ("RUSTSEC-2021-0060", "aes-soft"),
    ("RUSTSEC-2021-0059", "aesni"),
    ("RUSTSEC-2024-0388", "derivative"),
    ("RUSTSEC-2025-0161", "libsecp256k1"),
    ("RUSTSEC-2024-0436", "paste"),
    ("RUSTSEC-2021-0127", "serde_cbor"),
    ("RUSTSEC-2021-0137", "sodiumoxide"),
    ("RUSTSEC-2025-0081", "unic-char-property"),
    ("RUSTSEC-2025-0075", "unic-char-range"),
    ("RUSTSEC-2025-0080", "unic-common"),
    ("RUSTSEC-2025-0090", "unic-emoji-char"),
    ("RUSTSEC-2025-0100", "unic-ucd-ident"),
    ("RUSTSEC-2025-0098", "unic-ucd-version"),
];
const FACADE_PROFILES: &[&str] = &[
    "validator",
    "miner",
    "owner",
    "validator,prometheus",
    "miner,redis",
    "owner,ledger",
    "validator,miner,prometheus",
    "intents-all",
    "blocking",
    "full",
];

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Identity {
    name: String,
    version: String,
    source: Option<String>,
    checksum: Option<String>,
}

impl Identity {
    fn exception(&self) -> Option<&'static DormantException> {
        EXCEPTIONS.iter().find(|rule| {
            self.name == rule.name
                && self.version == rule.version
                && self.source.as_deref() == Some(SOURCE)
                && self.checksum.as_deref() == Some(rule.checksum)
        })
    }

    fn approved(&self) -> bool {
        self.exception().is_some()
    }
}

fn affected(name: &str, version: &str) -> Result<bool> {
    let Some(rule) = EXCEPTIONS.iter().find(|rule| rule.name == name) else {
        return Ok(false);
    };
    Ok(semver::Version::parse(version)? < semver::Version::parse(rule.fixed)?)
}

fn lock_identities(text: &str) -> Result<BTreeSet<Identity>> {
    let lock: DocumentMut = text.parse().context("parse audited lockfile")?;
    let packages = lock["package"]
        .as_array_of_tables()
        .context("lockfile missing packages")?;
    let mut identities = BTreeSet::new();
    for package in packages {
        let identity = Identity {
            name: package["name"]
                .as_str()
                .context("lock package missing name")?
                .into(),
            version: package["version"]
                .as_str()
                .context("lock package missing version")?
                .into(),
            source: package
                .get("source")
                .map(|s| s.as_str().context("invalid lock source"))
                .transpose()?
                .map(str::to_owned),
            checksum: package
                .get("checksum")
                .map(|s| s.as_str().context("invalid lock checksum"))
                .transpose()?
                .map(str::to_owned),
        };
        semver::Version::parse(&identity.version)?;
        ensure!(
            !affected(&identity.name, &identity.version)? || identity.approved(),
            "unexpected affected dependency lock entry: {identity:?}"
        );
        ensure!(identities.insert(identity), "duplicate lockfile identity");
    }
    ensure!(!identities.is_empty(), "empty lockfile");
    Ok(identities)
}

fn object_keys(value: &Json, expected: &[&str]) -> Result<()> {
    let object = value.as_object().context("audit schema expected object")?;
    ensure!(
        object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key)),
        "unexpected audit schema keys: {:?}",
        object.keys().collect::<Vec<_>>()
    );
    Ok(())
}

fn field<'a>(value: &'a Json, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .with_context(|| format!("audit schema missing string {key}"))
}

fn json_identity(value: &Json) -> Result<Identity> {
    let optional = |key| -> Result<Option<String>> {
        let value = value
            .get(key)
            .with_context(|| format!("audit package missing {key}"))?;
        if value.is_null() {
            Ok(None)
        } else {
            Ok(Some(
                value.as_str().context("invalid audit identity")?.into(),
            ))
        }
    };
    ensure!(
        value.get("replace") == Some(&Json::Null),
        "audit package replacement is unsupported"
    );
    let identity = Identity {
        name: field(value, "name")?.into(),
        version: field(value, "version")?.into(),
        source: optional("source")?,
        checksum: optional("checksum")?,
    };
    semver::Version::parse(&identity.version)?;
    Ok(identity)
}

fn audit_finding(value: &Json, kind: Option<&str>, locks: &BTreeSet<Identity>) -> Result<bool> {
    let identity = json_identity(&value["package"])?;
    ensure!(
        locks.contains(&identity),
        "audit reported unknown lock entry: {identity:?}"
    );
    let advisory = &value["advisory"];
    let id = field(advisory, "id")?;
    ensure!(
        field(advisory, "package")? == identity.name,
        "advisory package mismatch"
    );
    ensure!(
        advisory.get("withdrawn") == Some(&Json::Null),
        "unexpected withdrawn advisory"
    );
    ensure!(
        field(advisory, "collection")? == "crates",
        "unsupported advisory collection"
    );
    if let Some(kind) = kind {
        ensure!(
            field(value, "kind")? == kind && field(advisory, "informational")? == kind,
            "audit warning kind mismatch"
        );
    } else {
        ensure!(
            advisory.get("informational") == Some(&Json::Null),
            "vulnerability marked informational"
        );
    }
    // Validate the patch range even if upstream later recategorizes this advisory.
    if let Some(rule) = EXCEPTIONS.iter().find(|rule| rule.advisory == id) {
        ensure!(
            identity
                .exception()
                .is_some_and(|exact| exact.advisory == id),
            "unapproved package for {id}: {identity:?}"
        );
        ensure!(
            kind.is_none() || kind == Some("unsound"),
            "unexpected {id} warning category"
        );
        object_keys(&value["versions"], &["patched", "unaffected"])?;
        let patched = value["versions"]["patched"]
            .as_array()
            .context("missing patched ranges")?;
        ensure!(
            patched.len() == 1
                && semver::VersionReq::parse(patched[0].as_str().context("invalid patch range")?)?
                    == semver::VersionReq::parse(&format!(">={}", rule.fixed))?
                && value["versions"]["unaffected"] == json!([]),
            "{id} version ranges changed; investigate before extending the exception"
        );
        return Ok(true);
    }
    ensure!(
        kind == Some("unmaintained") && MAINTENANCE.contains(&(id, identity.name.as_str())),
        "blocking advisory {id} on {} {}",
        identity.name,
        identity.version
    );
    Ok(false)
}

#[derive(Debug, PartialEq, Eq)]
struct Audit {
    exceptions: usize,
    maintenance: usize,
    failing_status: bool,
}

struct RawAudit {
    terminal: Output,
    json: Output,
}

impl RawAudit {
    fn validate(&self, identities: &BTreeSet<Identity>) -> Result<Audit> {
        ensure!(
            !std::str::from_utf8(&self.json.stderr)?
                .lines()
                .any(|line| line.trim_start().starts_with("error:")),
            "cargo-audit JSON command reported a tool/database error"
        );
        let report: Json = serde_json::from_slice(&self.json.stdout)
            .context("cargo-audit did not produce a valid raw JSON report")?;
        let audit = validate_audit(&report, identities)?;
        let expected = i32::from(audit.failing_status);
        ensure!(
            self.json.status.code() == Some(expected)
                && self.terminal.status.code() == Some(expected),
            "cargo-audit status differs from its raw findings or tool/database error occurred"
        );
        validate_terminal(&self.terminal, &report)?;
        Ok(audit)
    }
}

fn validate_terminal(output: &Output, report: &Json) -> Result<()> {
    let text = format!(
        "{}{}",
        std::str::from_utf8(&output.stdout)?,
        std::str::from_utf8(&output.stderr)?
    );
    let vulnerabilities = report["vulnerabilities"]["list"]
        .as_array()
        .context("missing vulnerability list")?;
    let warnings = report["warnings"]
        .as_object()
        .context("missing warning map")?;
    let mut expected_ids = Vec::new();
    for finding in vulnerabilities.iter().chain(
        warnings
            .values()
            .flat_map(|v| v.as_array().into_iter().flatten()),
    ) {
        expected_ids.push(field(&finding["advisory"], "id")?);
    }
    expected_ids.sort_unstable();
    let mut actual_ids: Vec<_> = text
        .lines()
        .filter_map(|line| line.strip_prefix("ID:").map(str::trim))
        .collect();
    actual_ids.sort_unstable();
    ensure!(
        actual_ids == expected_ids,
        "raw terminal audit differs from raw JSON findings or failed before completing"
    );
    let denied = warnings
        .get("unsound")
        .and_then(Json::as_array)
        .map_or(0, Vec::len);
    let mut expected_errors = Vec::new();
    if !vulnerabilities.is_empty() {
        expected_errors.push(format!(
            "error: {} {} found!",
            vulnerabilities.len(),
            if vulnerabilities.len() == 1 {
                "vulnerability"
            } else {
                "vulnerabilities"
            }
        ));
    }
    if denied > 0 {
        expected_errors.push(format!(
            "error: {denied} denied {} found!",
            if denied == 1 { "warning" } else { "warnings" }
        ));
    }
    let actual_errors: Vec<_> = text
        .lines()
        .filter(|line| line.trim_start().starts_with("error:"))
        .map(str::trim)
        .collect();
    ensure!(
        actual_errors == expected_errors,
        "cargo-audit terminal tool/database error or inconsistent failure summary"
    );
    ensure!(
        text.lines()
            .any(|line| line.trim_start().starts_with("Loaded "))
            && text
                .lines()
                .any(|line| line.trim_start().starts_with("Scanning ")),
        "terminal audit lacks database/lockfile scan evidence"
    );
    Ok(())
}

fn validate_audit(report: &Json, locks: &BTreeSet<Identity>) -> Result<Audit> {
    object_keys(
        report,
        &[
            "database",
            "lockfile",
            "settings",
            "vulnerabilities",
            "warnings",
        ],
    )?;
    object_keys(
        &report["database"],
        &["advisory-count", "last-commit", "last-updated"],
    )?;
    ensure!(
        report["database"]["advisory-count"]
            .as_u64()
            .is_some_and(|count| count > 0),
        "empty advisory database"
    );
    object_keys(&report["lockfile"], &["dependency-count"])?;
    ensure!(
        report["lockfile"]["dependency-count"].as_u64() == Some(u64::try_from(locks.len())?),
        "audited dependency count differs from lockfile"
    );
    let settings = &report["settings"];
    object_keys(
        settings,
        &[
            "target_arch",
            "target_os",
            "severity",
            "ignore",
            "informational_warnings",
        ],
    )?;
    ensure!(
        settings["target_arch"] == json!([])
            && settings["target_os"] == json!([])
            && settings["severity"].is_null()
            && settings["ignore"] == json!([]),
        "audit filtering/ignores are forbidden"
    );
    let informational: BTreeSet<_> = settings["informational_warnings"]
        .as_array()
        .context("missing informational settings")?
        .iter()
        .map(|v| v.as_str().context("invalid informational setting"))
        .collect::<Result<_>>()?;
    ensure!(
        informational == BTreeSet::from(["unmaintained", "unsound", "notice"])
            && settings["informational_warnings"]
                .as_array()
                .is_some_and(|a| a.len() == 3),
        "audit informational coverage changed"
    );
    let vulnerabilities = &report["vulnerabilities"];
    object_keys(vulnerabilities, &["found", "count", "list"])?;
    let findings = vulnerabilities["list"]
        .as_array()
        .context("missing vulnerability list")?;
    ensure!(
        vulnerabilities["count"].as_u64() == Some(u64::try_from(findings.len())?)
            && vulnerabilities["found"].as_bool() == Some(!findings.is_empty()),
        "inconsistent vulnerability count"
    );
    let mut audit = Audit {
        exceptions: 0,
        maintenance: 0,
        failing_status: !findings.is_empty(),
    };
    let mut seen = BTreeSet::new();
    for finding in findings {
        ensure!(
            seen.insert((
                field(&finding["advisory"], "id")?.to_owned(),
                json_identity(&finding["package"])?
            )),
            "duplicate audit finding"
        );
        audit.exceptions += usize::from(audit_finding(finding, None, locks)?);
    }
    for (kind, findings) in report["warnings"]
        .as_object()
        .context("missing warning object")?
    {
        ensure!(
            ["unmaintained", "unsound", "notice", "yanked"].contains(&kind.as_str()),
            "unknown warning category {kind}"
        );
        let findings = findings.as_array().context("invalid warning list")?;
        if kind == "unsound" && !findings.is_empty() {
            audit.failing_status = true;
        }
        for finding in findings {
            ensure!(
                seen.insert((
                    field(&finding["advisory"], "id")?.to_owned(),
                    json_identity(&finding["package"])?
                )),
                "duplicate audit finding"
            );
            if audit_finding(finding, Some(kind), locks)? {
                audit.exceptions += 1;
            } else {
                audit.maintenance += 1;
            }
        }
    }
    ensure!(
        audit.exceptions <= EXCEPTIONS.len(),
        "too many dormant exception findings"
    );
    Ok(audit)
}

fn check_graph(output: &str, roots: &[String]) -> Result<()> {
    let mut packages = BTreeSet::new();
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let mut words = line.split_whitespace();
        let name = words.next().context("empty dependency graph line")?;
        let version = words
            .next()
            .and_then(|v| v.strip_prefix('v'))
            .context("unrecognized cargo tree package")?;
        semver::Version::parse(version).context("invalid cargo tree package version")?;
        ensure!(
            !affected(name, version)?,
            "affected {name} {version} is active in a shipped or test graph"
        );
        packages.insert(name.to_owned());
    }
    ensure!(
        !roots.is_empty() && roots.iter().all(|root| packages.contains(root)),
        "dependency graph missing its selected roots"
    );
    Ok(())
}

struct Runner {
    root: PathBuf,
    temporary: PathBuf,
    evidence: PathBuf,
    next: usize,
}

impl Runner {
    fn new(root: &Path) -> Result<Self> {
        let temporary = PathBuf::from(
            std::env::var_os("RTAO_TEMP_ROOT")
                .context("supply requires the recorded RTAO_TEMP_ROOT")?,
        );
        ensure!(temporary.is_absolute(), "RTAO_TEMP_ROOT must be absolute");
        fs::create_dir_all(temporary.join("tmp"))?;
        let evidence = temporary.join(format!("ci/supply-{}", std::process::id()));
        ensure!(
            !evidence.exists(),
            "supply evidence directory already exists"
        );
        fs::create_dir_all(&evidence)?;
        Ok(Self {
            root: root.into(),
            temporary,
            evidence,
            next: 0,
        })
    }

    fn cargo(&mut self, label: &str, args: &[String]) -> Result<Output> {
        self.next += 1;
        let stem = format!("{:03}-{label}", self.next);
        fs::write(
            self.evidence.join(format!("{stem}.command.json")),
            serde_json::to_vec_pretty(&json!({"program": "cargo", "args": args}))?,
        )?;
        let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .current_dir(&self.root)
            .args(args)
            .env("CARGO_TARGET_DIR", self.temporary.join("build"))
            .env("TMPDIR", self.temporary.join("tmp"))
            .env("CARGO_TERM_COLOR", "never")
            .output()
            .with_context(|| format!("run supply {label}"))?;
        fs::write(self.evidence.join(format!("{stem}.stdout")), &output.stdout)?;
        fs::write(self.evidence.join(format!("{stem}.stderr")), &output.stderr)?;
        fs::write(
            self.evidence.join(format!("{stem}.status")),
            format!("{:?}\n", output.status.code()),
        )?;
        Ok(output)
    }

    fn graph(&mut self, label: &str, selection: &[String], roots: &[String]) -> Result<()> {
        let mut args: Vec<String> = [
            "tree",
            "--locked",
            "--offline",
            "--target",
            "all",
            "--edges",
            "normal,build,dev",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ]
        .map(str::to_owned)
        .into();
        args.extend_from_slice(selection);
        let output = self.cargo(label, &args)?;
        ensure!(
            output.status.success(),
            "{label} dependency inspection failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        check_graph(std::str::from_utf8(&output.stdout)?, roots)
            .with_context(|| format!("{label} dormant lru guard"))
    }

    fn audit(&mut self, label: &str, lock: &Path) -> Result<RawAudit> {
        let mut args: Vec<String> = ["audit", "--deny", "unsound", "--color", "never", "--db"]
            .map(str::to_owned)
            .into();
        args.push(self.temporary.join("ci/audit-db").display().to_string());
        args.extend(["--file".into(), lock.display().to_string()]);
        if label == "fuzz" {
            args.push("--no-fetch".into());
        }
        let terminal = self.cargo(&format!("{label}-audit-raw-text"), &args)?;
        print!(
            "{}{}",
            String::from_utf8_lossy(&terminal.stdout),
            String::from_utf8_lossy(&terminal.stderr)
        );
        if label != "fuzz" {
            args.push("--no-fetch".into());
        }
        args.push("--json".into());
        let json = self.cargo(&format!("{label}-audit-raw-json"), &args)?;
        Ok(RawAudit { terminal, json })
    }
}

fn package_features(metadata: &Json, name: &str) -> Result<Vec<String>> {
    let packages = metadata["packages"]
        .as_array()
        .context("metadata missing packages")?;
    let package = packages
        .iter()
        .find(|p| p["name"].as_str() == Some(name))
        .with_context(|| format!("metadata missing {name}"))?;
    Ok(package["features"]
        .as_object()
        .context("metadata missing feature map")?
        .keys()
        .cloned()
        .collect())
}

struct Selection {
    label: String,
    args: Vec<String>,
    roots: Vec<String>,
}

fn selections(metadata: &Json) -> Result<Vec<Selection>> {
    let mut selections = Vec::new();
    for package in ["rtao", "rtao-neuron", "rtao-cli"] {
        let roots = vec![package.to_owned()];
        let base = vec!["-p".into(), package.into()];
        selections.push(Selection {
            label: format!("{package}-default"),
            args: base.clone(),
            roots: roots.clone(),
        });
        let mut none = base.clone();
        none.push("--no-default-features".into());
        selections.push(Selection {
            label: format!("{package}-none"),
            args: none.clone(),
            roots: roots.clone(),
        });
        let mut all = base;
        all.push("--all-features".into());
        selections.push(Selection {
            label: format!("{package}-all"),
            args: all,
            roots: roots.clone(),
        });
        let mut features = package_features(metadata, package)?;
        if package == "rtao" {
            features.extend(FACADE_PROFILES.iter().map(|f| (*f).to_owned()));
        }
        if package == "rtao-neuron" {
            features.extend([
                "validator,prometheus".into(),
                "miner,prometheus".into(),
                "owner,prometheus".into(),
            ]);
        }
        let features: BTreeSet<_> = features.into_iter().filter(|f| f != "default").collect();
        for (index, features) in features.into_iter().enumerate() {
            let mut args = none.clone();
            args.extend(["--features".into(), features]);
            selections.push(Selection {
                label: format!("{package}-profile-{index}"),
                args,
                roots: roots.clone(),
            });
        }
    }
    Ok(selections)
}

/// Run unfiltered audits, then prove each exact exception is dormant everywhere.
pub(crate) fn run(args: Vec<String>) -> Result<()> {
    if args == ["--help"] || args == ["-h"] {
        println!(
            "cargo xtask supply\nRetains raw cargo-audit text/JSON and graph evidence under RTAO_TEMP_ROOT/ci/supply-<pid>.\nOnly exact dormant lru RUSTSEC-2026-0253, ruzstd RUSTSEC-2026-0344 and known maintenance notices may pass; tool/schema errors and activation block."
        );
        return Ok(());
    }
    ensure!(args.is_empty(), "usage: cargo xtask supply");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("missing workspace root")?;
    let mut runner = Runner::new(root)?;
    println!(
        "supply: unfiltered evidence at {}",
        runner.evidence.display()
    );
    let versions: DocumentMut = fs::read_to_string(root.join("ci/tool-versions.toml"))?.parse()?;
    let expected = versions["tools"]["cargo-audit"]
        .as_str()
        .context("missing pinned cargo-audit version")?;
    let version = runner.cargo("audit-version", &["audit".into(), "--version".into()])?;
    let version_text = std::str::from_utf8(&version.stdout)?;
    let mut words = version_text.split_whitespace();
    ensure!(
        version.status.success()
            && matches!(words.next(), Some("cargo-audit" | "cargo-audit-audit"))
            && words.next() == Some(expected)
            && words.next().is_none(),
        "supply requires pinned cargo-audit {expected}"
    );
    let locks = [root.join("Cargo.lock"), root.join("fuzz/Cargo.lock")];
    let texts = locks
        .iter()
        .map(fs::read_to_string)
        .collect::<std::io::Result<Vec<_>>>()?;
    // Collect both complete raw audits before applying policy, including when
    // an unexpected affected entry or unrelated advisory must block the gate.
    let mut raw_audits = Vec::new();
    for (index, label) in ["workspace", "fuzz"].iter().enumerate() {
        fs::write(
            runner.evidence.join(format!("{label}-Cargo.lock")),
            &texts[index],
        )?;
        raw_audits.push(runner.audit(label, &locks[index])?);
    }
    let identities = texts
        .iter()
        .map(|text| lock_identities(text))
        .collect::<Result<Vec<_>>>()?;
    let output = runner.cargo(
        "metadata",
        &[
            "metadata",
            "--locked",
            "--offline",
            "--all-features",
            "--no-deps",
            "--format-version",
            "1",
        ]
        .map(str::to_owned),
    )?;
    ensure!(
        output.status.success(),
        "supply profile metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Json = serde_json::from_slice(&output.stdout)?;
    let members = metadata["workspace_members"]
        .as_array()
        .context("metadata missing workspace members")?;
    let roots = metadata["packages"]
        .as_array()
        .context("metadata missing packages")?
        .iter()
        .filter(|p| members.contains(&p["id"]))
        .map(|p| Ok(field(p, "name")?.to_owned()))
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        roots.len() == members.len(),
        "metadata has unknown workspace members"
    );
    runner.graph(
        "workspace-all-features",
        &["--workspace".into(), "--all-features".into()],
        &roots,
    )?;
    runner.graph(
        "fuzz-all-features",
        &[
            "--manifest-path".into(),
            "fuzz/Cargo.toml".into(),
            "--workspace".into(),
            "--all-features".into(),
        ],
        &["rtao-fuzz".into()],
    )?;
    for selection in selections(&metadata)? {
        runner.graph(&selection.label, &selection.args, &selection.roots)?;
    }
    for (index, path) in locks.iter().enumerate() {
        ensure!(
            fs::read_to_string(path)? == texts[index],
            "audited lockfile changed during supply checks"
        );
    }
    let audits = raw_audits
        .iter()
        .zip(&identities)
        .map(|(raw, identities)| raw.validate(identities))
        .collect::<Result<Vec<_>>>()?;
    for (label, audit) in ["workspace", "fuzz"].iter().zip(audits) {
        println!(
            "supply: {label}: {} exact dormant advisory findings; {} retained maintenance notices; active affected dependencies absent from every checked graph",
            audit.exceptions, audit.maintenance
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock() -> String {
        format!(
            "[[package]]\nname = 'lru'\nversion = '0.16.4'\nsource = '{SOURCE}'\nchecksum = '{CHECKSUM}'\n"
        )
    }

    fn finding(kind: Option<&str>) -> Json {
        let mut finding = json!({
            "package": {"name": "lru", "version": "0.16.4", "source": SOURCE, "checksum": CHECKSUM, "replace": null},
            "advisory": {"id": ADVISORY, "package": "lru", "collection": "crates", "withdrawn": null, "informational": kind},
            "versions": {"patched": [">=0.18.2"], "unaffected": []}, "affected": null
        });
        if let Some(kind) = kind {
            finding["kind"] = json!(kind);
        }
        finding
    }

    fn report() -> Json {
        json!({"database": {"advisory-count": 1300, "last-commit": null, "last-updated": null},
            "lockfile": {"dependency-count": 1},
            "settings": {"target_arch": [], "target_os": [], "severity": null, "ignore": [], "informational_warnings": ["unmaintained", "unsound", "notice"]},
            "vulnerabilities": {"found": false, "count": 0, "list": []},
            "warnings": {"unsound": [finding(Some("unsound"))]}})
    }

    #[cfg(unix)]
    fn raw_audit(status: i32, terminal: &str, report: &Json) -> Result<RawAudit> {
        use std::os::unix::process::ExitStatusExt;
        Ok(RawAudit {
            terminal: Output {
                status: std::process::ExitStatus::from_raw(status << 8),
                stdout: terminal.as_bytes().to_vec(),
                stderr: Vec::new(),
            },
            json: Output {
                status: std::process::ExitStatus::from_raw(status << 8),
                stdout: serde_json::to_vec(report)?,
                stderr: Vec::new(),
            },
        })
    }

    #[test]
    #[cfg(unix)]
    fn raw_status_and_terminal_errors_cannot_masquerade_as_the_exception() -> Result<()> {
        let locks = lock_identities(&lock())?;
        let terminal = format!(
            "      Loaded 1300 security advisories\n    Scanning Cargo.lock for vulnerabilities (1 crate dependencies)\nID:        {ADVISORY}\nerror: 1 denied warning found!\n"
        );
        ensure!(
            raw_audit(1, &terminal, &report())?
                .validate(&locks)?
                .exceptions
                == 1
        );
        ensure!(
            raw_audit(0, &terminal, &report())?
                .validate(&locks)
                .is_err()
        );
        ensure!(
            raw_audit(2, &terminal, &report())?
                .validate(&locks)
                .is_err()
        );
        for bad in [
            terminal.replace(
                "error: 1 denied warning found!",
                "error: failed to load database",
            ),
            terminal.replace(ADVISORY, "RUSTSEC-2099-9999"),
            terminal.replace(
                "    Scanning Cargo.lock for vulnerabilities (1 crate dependencies)\n",
                "",
            ),
        ] {
            ensure!(raw_audit(1, &bad, &report())?.validate(&locks).is_err());
        }
        let mut error = raw_audit(1, &terminal, &report())?;
        error.json.stderr = b"error: database unavailable\n".to_vec();
        ensure!(error.validate(&locks).is_err());
        error.json.stdout = b"{invalid-json".to_vec();
        error.json.stderr.clear();
        ensure!(error.validate(&locks).is_err());
        Ok(())
    }

    #[test]
    fn accepts_only_provenance_pinned_dormant_finding() -> Result<()> {
        let locks = lock_identities(&lock())?;
        ensure!(
            validate_audit(&report(), &locks)?
                == Audit {
                    exceptions: 1,
                    maintenance: 0,
                    failing_status: true
                }
        );
        check_graph(
            "rtao v0.1.0 (/workspace/crates/rtao)\n\nlru v0.18.2\n",
            &["rtao".into()],
        )?;
        Ok(())
    }

    #[test]
    fn activation_and_empty_or_malformed_graphs_block() -> Result<()> {
        for graph in [
            "rtao v0.1.0\nlru v0.16.4\n",
            "rtao v0.1.0\nlru v0.17.0\n",
            "rtao v0.1.0\nlru\n",
            "",
            "unrecognized",
        ] {
            ensure!(
                check_graph(graph, &["rtao".into()]).is_err(),
                "accepted graph {graph:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn unknown_affected_lock_entries_block() -> Result<()> {
        for changed in [
            lock().replace("0.16.4", "0.17.0"),
            lock().replace(SOURCE, "git+https://example.test/lru#1234"),
            lock().replace(CHECKSUM, "changed"),
            format!("{}{}", lock(), lock()),
            format!("{}{}", lock(), lock().replace("0.16.4", "0.16.3")),
        ] {
            ensure!(lock_identities(&changed).is_err());
        }
        Ok(())
    }

    #[test]
    fn unknown_advisory_or_audited_identity_blocks() -> Result<()> {
        let locks = lock_identities(&lock())?;
        for (key, changed) in [
            ("advisory", "id"),
            ("package", "version"),
            ("package", "source"),
            ("package", "checksum"),
        ] {
            let mut report = report();
            report["warnings"]["unsound"][0][key][changed] = json!("unexpected");
            ensure!(validate_audit(&report, &locks).is_err());
        }
        let mut report = report();
        report["warnings"]["unmaintained"] = json!([finding(Some("unmaintained"))]);
        ensure!(validate_audit(&report, &locks).is_err());
        Ok(())
    }

    #[test]
    fn only_exact_existing_maintenance_notices_remain_nonblocking() -> Result<()> {
        let maintenance_lock = format!(
            "{}\n[[package]]\nname = 'derivative'\nversion = '2.2.0'\nsource = '{SOURCE}'\nchecksum = 'maintenance-checksum'\n",
            lock()
        );
        let locks = lock_identities(&maintenance_lock)?;
        let mut report = report();
        report["lockfile"]["dependency-count"] = json!(2);
        let mut notice = finding(Some("unmaintained"));
        notice["package"]["name"] = json!("derivative");
        notice["package"]["version"] = json!("2.2.0");
        notice["package"]["checksum"] = json!("maintenance-checksum");
        notice["advisory"]["package"] = json!("derivative");
        notice["advisory"]["id"] = json!("RUSTSEC-2024-0388");
        report["warnings"]["unmaintained"] = json!([notice]);
        ensure!(validate_audit(&report, &locks)?.maintenance == 1);
        report["warnings"]["unmaintained"][0]["advisory"]["id"] = json!("RUSTSEC-2099-9999");
        ensure!(validate_audit(&report, &locks).is_err());
        Ok(())
    }

    #[test]
    fn vulnerability_recategorization_still_requires_exact_exception() -> Result<()> {
        let locks = lock_identities(&lock())?;
        let mut report = report();
        report["warnings"] = json!({});
        report["vulnerabilities"] = json!({"found": true, "count": 1, "list": [finding(None)]});
        ensure!(validate_audit(&report, &locks)?.exceptions == 1);
        report["vulnerabilities"]["list"][0]["advisory"]["id"] = json!("RUSTSEC-2099-9999");
        ensure!(validate_audit(&report, &locks).is_err());
        Ok(())
    }

    #[test]
    fn filters_schema_errors_duplicate_findings_and_changed_ranges_block() -> Result<()> {
        let locks = lock_identities(&lock())?;
        let mut variants = Vec::new();
        for (key, value) in [
            ("ignore", json!([ADVISORY])),
            ("target_os", json!(["linux"])),
            ("target_arch", json!(["aarch64"])),
            ("severity", json!("high")),
            ("informational_warnings", json!(["unmaintained"])),
        ] {
            let mut report = report();
            report["settings"][key] = value;
            variants.push(report);
        }
        let mut duplicate = report();
        duplicate["warnings"]["unsound"] =
            json!([finding(Some("unsound")), finding(Some("unsound"))]);
        variants.push(duplicate);
        let mut changed = report();
        changed["warnings"]["unsound"][0]["versions"]["patched"] = json!([">=0.17.0"]);
        variants.push(changed);
        let mut unknown = report();
        unknown["error"] = json!("database failure");
        variants.push(unknown);
        let mut count = report();
        count["vulnerabilities"]["count"] = json!(1);
        variants.push(count);
        for report in variants {
            ensure!(validate_audit(&report, &locks).is_err());
        }
        Ok(())
    }

    #[test]
    fn profiles_cover_facade_neuron_cli_and_image_arguments() -> Result<()> {
        let metadata = json!({"packages": [
            {"name": "rtao", "features": {"default": [], "new-feature": []}},
            {"name": "rtao-neuron", "features": {"validator": [], "miner": [], "owner": [], "prometheus": []}},
            {"name": "rtao-cli", "features": {"ledger": []}}]});
        let selections = selections(&metadata)?;
        for (package, feature) in [
            ("rtao", "new-feature"),
            ("rtao", "owner,ledger"),
            ("rtao-neuron", "validator,prometheus"),
            ("rtao-neuron", "miner,prometheus"),
            ("rtao-cli", "ledger"),
        ] {
            ensure!(
                selections
                    .iter()
                    .any(|selection| selection.roots == [package.to_owned()]
                        && selection.args.contains(&feature.to_owned()))
            );
        }
        Ok(())
    }

    fn ruzstd_lock() -> String {
        format!(
            "[[package]]\nname = 'ruzstd'\nversion = '0.8.3'\nsource = '{SOURCE}'\nchecksum = '{RUZSTD_CHECKSUM}'\n"
        )
    }

    fn combined_report() -> Json {
        let mut report = report();
        report["lockfile"]["dependency-count"] = json!(2);
        let mut ruzstd = finding(None);
        ruzstd["package"]["name"] = json!("ruzstd");
        ruzstd["package"]["version"] = json!("0.8.3");
        ruzstd["package"]["checksum"] = json!(RUZSTD_CHECKSUM);
        ruzstd["advisory"]["id"] = json!(RUZSTD_ADVISORY);
        ruzstd["advisory"]["package"] = json!("ruzstd");
        ruzstd["versions"]["patched"] = json!([">=0.9.1"]);
        report["vulnerabilities"] = json!({"found": true, "count": 1, "list": [ruzstd]});
        report
    }

    #[test]
    fn exact_dormant_ruzstd_keeps_raw_failure_and_the_existing_lru_rule() -> Result<()> {
        let locks = lock_identities(&format!("{}{}", lock(), ruzstd_lock()))?;
        let report = combined_report();
        ensure!(
            validate_audit(&report, &locks)?
                == Audit {
                    exceptions: 2,
                    maintenance: 0,
                    failing_status: true,
                }
        );
        check_graph(
            "rtao v0.1.0\n\nlru v0.18.2\nruzstd v0.9.1\n",
            &["rtao".into()],
        )?;
        #[cfg(unix)]
        {
            let terminal = format!(
                "      Loaded 1300 security advisories\n    Scanning Cargo.lock for vulnerabilities (2 crate dependencies)\nID:        {RUZSTD_ADVISORY}\nID:        {ADVISORY}\nerror: 1 vulnerability found!\nerror: 1 denied warning found!\n"
            );
            ensure!(
                raw_audit(1, &terminal, &report)?
                    .validate(&locks)?
                    .exceptions
                    == 2
            );
            ensure!(raw_audit(0, &terminal, &report)?.validate(&locks).is_err());
        }
        Ok(())
    }

    #[test]
    fn ruzstd_activation_and_unknown_affected_lock_entries_block() -> Result<()> {
        for version in ["0.7.0", "0.8.2", "0.8.3", "0.9.0"] {
            let graph = format!("rtao v0.1.0\nruzstd v{version}\n");
            ensure!(check_graph(&graph, &["rtao".into()]).is_err());
        }
        for changed in [
            ruzstd_lock().replace("0.8.3", "0.8.2"),
            ruzstd_lock().replace("0.8.3", "0.9.0"),
            ruzstd_lock().replace(SOURCE, "git+https://example.test/ruzstd#1234"),
            ruzstd_lock().replace(RUZSTD_CHECKSUM, "changed"),
            ruzstd_lock().replace(&format!("checksum = '{RUZSTD_CHECKSUM}'\n"), ""),
            format!("{}{}", ruzstd_lock(), ruzstd_lock()),
        ] {
            ensure!(lock_identities(&changed).is_err());
        }
        Ok(())
    }

    #[test]
    fn ruzstd_exception_rejects_other_advisories_identities_ranges_and_filters() -> Result<()> {
        let locks = lock_identities(&format!("{}{}", lock(), ruzstd_lock()))?;
        let baseline = combined_report();
        let mut variants = Vec::new();
        for (section, key, changed) in [
            ("advisory", "id", "RUSTSEC-2099-9999"),
            ("advisory", "id", ADVISORY),
            ("advisory", "package", "lru"),
            ("package", "name", "unexpected"),
            ("package", "version", "0.8.2"),
            ("package", "source", "git+https://example.test/ruzstd#1234"),
            ("package", "checksum", "changed"),
        ] {
            let mut report = baseline.clone();
            report["vulnerabilities"]["list"][0][section][key] = json!(changed);
            variants.push(report);
        }
        for (key, changed) in [
            ("patched", json!([">=0.9.0"])),
            ("patched", json!([">=0.9.1", ">=1.0.0"])),
            ("unaffected", json!(["<0.1.0"])),
        ] {
            let mut report = baseline.clone();
            report["vulnerabilities"]["list"][0]["versions"][key] = changed;
            variants.push(report);
        }
        for (key, changed) in [
            ("ignore", json!([RUZSTD_ADVISORY])),
            ("severity", json!("high")),
            ("target_os", json!(["linux"])),
        ] {
            let mut report = baseline.clone();
            report["settings"][key] = changed;
            variants.push(report);
        }
        let mut cross_identity = baseline.clone();
        cross_identity["vulnerabilities"]["list"][0]["advisory"]["id"] = json!(ADVISORY);
        cross_identity["vulnerabilities"]["list"][0]["versions"]["patched"] = json!([">=0.18.2"]);
        variants.push(cross_identity);
        let mut cross_identity = baseline.clone();
        cross_identity["warnings"]["unsound"][0]["advisory"]["id"] = json!(RUZSTD_ADVISORY);
        cross_identity["warnings"]["unsound"][0]["versions"]["patched"] = json!([">=0.9.1"]);
        variants.push(cross_identity);
        let mut duplicate = baseline.clone();
        duplicate["vulnerabilities"]["count"] = json!(2);
        duplicate["vulnerabilities"]["list"] = json!([
            baseline["vulnerabilities"]["list"][0].clone(),
            baseline["vulnerabilities"]["list"][0].clone(),
        ]);
        variants.push(duplicate);
        for report in variants {
            ensure!(validate_audit(&report, &locks).is_err());
        }
        Ok(())
    }
}
