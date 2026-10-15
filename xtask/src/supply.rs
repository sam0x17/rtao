//! Raw lockfile audits with dormant vulnerability exceptions and reviewed
//! informational maintenance notices.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{Context, Result, ensure};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use toml_edit::DocumentMut;

const ADVISORY: &str = "RUSTSEC-2026-0253";
const SOURCE: &str = "registry+https://github.com/rust-lang/crates.io-index";
const CHECKSUM: &str = "7f66e8d5d03f609abc3a39e6f08e4164ebf1447a732906d39eb9b99b7919ef39";
const FIXED: &str = "0.18.2";
const RUZSTD_ADVISORY: &str = "RUSTSEC-2026-0344";
const RUZSTD_CHECKSUM: &str = "a7c1c839d570d835527c9a5e4db7cb2198683a988cb9d7293fc8674e6bd58fc8";
const PROC_MACRO_ADVISORY: &str = "RUSTSEC-2026-0173";
const PROC_MACRO_CHECKSUM: &str =
    "11ec05c52be0a07b08061f7dd003e7d7092e0472bc731b4af7bb1ef876109802";
const SUBXT_CHECKSUM: &str = "c5a67bfdccc607684ec2db8dbff6dbfcb72fa0ba10608698d9cc0b3cc452ebe5";
const SUBXT_MACRO_CHECKSUM: &str =
    "77d22d8350ef5f6b455f09b211be9468c47499778e8d18615a00c12143f7f400";
const PROC_MACRO_DESCRIPTION: &str =
    "2f735d4ab46291cc8340d0ab166530ea0ea94959736ef7dc13cc932d6f6913c9";
const SMALLSTR_ADVISORY: &str = "RUSTSEC-2026-0215";
const SMALLSTR_CHECKSUM: &str = "862077b1e764f04c251fe82a2ef562fd78d7cadaeb072ca7c2bcaf7217b1ff3b";
const SCALE_INFO_LEGACY_CHECKSUM: &str =
    "d972ce93a4f81efc40fce251e992faf99ecb090c02bcbd3614e213991038c181";
const SUBXT_METADATA_CHECKSUM: &str =
    "cc5c32a26366a6eac132535164b4f0034a5f4d56718f34db6814af7fc5e10f35";
const FRAME_DECODE_CHECKSUM: &str =
    "c9a9e93cab13c8e9a6accf029997d19943cf54fea5f5eb028a1133b7179d06aa";
const SMALLSTR_DESCRIPTION: &str =
    "1e732bf83d21445cd724ccdd77a21839169c4b8b0a5ccea14c6c8187e56a3f80";

struct ReviewedMaintenance {
    advisory: &'static str,
    name: &'static str,
    version: &'static str,
    checksum: &'static str,
    date: &'static str,
    title: &'static str,
    url: Option<&'static str>,
    description: &'static str,
}

const REVIEWED_MAINTENANCE: &[ReviewedMaintenance] = &[
    ReviewedMaintenance {
        advisory: PROC_MACRO_ADVISORY,
        name: "proc-macro-error2",
        version: "2.0.1",
        checksum: PROC_MACRO_CHECKSUM,
        date: "2026-06-07",
        title: "proc-macro-error2 is unmaintained",
        url: Some("https://github.com/GnomedDev/proc-macro-error-2/issues/17"),
        description: PROC_MACRO_DESCRIPTION,
    },
    ReviewedMaintenance {
        advisory: SMALLSTR_ADVISORY,
        name: "smallstr",
        version: "0.3.1",
        checksum: SMALLSTR_CHECKSUM,
        date: "2026-07-22",
        title: "smallstr is unmaintained",
        url: None,
        description: SMALLSTR_DESCRIPTION,
    },
];

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
    proc_macro_maintenance_chain(packages)?;
    smallstr_maintenance_chain(packages)?;
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

// Unlike the dormant exceptions, this is an active build dependency of the
// optional Subxt adapter. Its reviewed parent chain is pinned until upstream
// replaces the abandoned diagnostic helper. See supply-proc-macro-error2.md.
fn proc_macro_maintenance_chain(packages: &toml_edit::ArrayOfTables) -> Result<()> {
    reviewed_maintenance_chain(
        packages,
        "proc-macro-error2",
        &[
            ("proc-macro-error2", "2.0.1", PROC_MACRO_CHECKSUM),
            ("subxt-macro", "0.51.1", SUBXT_MACRO_CHECKSUM),
            ("subxt", "0.51.1", SUBXT_CHECKSUM),
        ],
        &[
            ("proc-macro-error2", "2.0.1", &["subxt-macro"]),
            ("subxt-macro", "0.51.1", &["subxt"]),
        ],
    )
}

// This dependency is active in the Subxt adapter; it is not a dormant exception.
// A compatible upstream release must migrate scale-info-legacy's string type.
// See supply-smallstr.md for the retained raw warning and dependency evidence.
fn smallstr_maintenance_chain(packages: &toml_edit::ArrayOfTables) -> Result<()> {
    reviewed_maintenance_chain(
        packages,
        "smallstr",
        &[
            ("smallstr", "0.3.1", SMALLSTR_CHECKSUM),
            ("scale-info-legacy", "0.4.2", SCALE_INFO_LEGACY_CHECKSUM),
            ("frame-decode", "0.18.1", FRAME_DECODE_CHECKSUM),
            ("subxt", "0.51.1", SUBXT_CHECKSUM),
            ("subxt-metadata", "0.51.1", SUBXT_METADATA_CHECKSUM),
        ],
        &[
            ("smallstr", "0.3.1", &["scale-info-legacy"]),
            (
                "scale-info-legacy",
                "0.4.2",
                &["frame-decode", "subxt", "subxt-metadata"],
            ),
        ],
    )
}

fn reviewed_maintenance_chain(
    packages: &toml_edit::ArrayOfTables,
    child: &str,
    identities: &[(&str, &str, &str)],
    edges: &[(&str, &str, &[&str])],
) -> Result<()> {
    if !packages
        .iter()
        .any(|package| package["name"].as_str() == Some(child))
    {
        return Ok(());
    }
    for &(name, version, checksum) in identities {
        let matching: Vec<_> = packages
            .iter()
            .filter(|package| package["name"].as_str() == Some(name))
            .collect();
        ensure!(
            matching.len() == 1,
            "reviewed maintenance chain changed: {name}"
        );
        let package = matching[0];
        ensure!(
            package["version"].as_str() == Some(version)
                && package["source"].as_str() == Some(SOURCE)
                && package["checksum"].as_str() == Some(checksum),
            "reviewed maintenance dependency identity changed: {name}"
        );
    }
    for &(child, version, expected_parents) in edges {
        let mut parents = Vec::new();
        for package in packages {
            let Some(dependencies) = package.get("dependencies") else {
                continue;
            };
            for dependency in dependencies
                .as_array()
                .context("invalid lock dependency list")?
            {
                let dependency = dependency.as_str().context("invalid lock dependency")?;
                if dependency.split_whitespace().next() == Some(child) {
                    ensure!(
                        dependency == child
                            || dependency == format!("{child} {version}")
                            || dependency == format!("{child} {version} ({SOURCE})"),
                        "reviewed maintenance edge identity changed: {dependency}"
                    );
                    parents.push(package["name"].as_str().context("missing parent name")?);
                }
            }
        }
        parents.sort_unstable();
        let mut expected_parents = expected_parents.to_vec();
        expected_parents.sort_unstable();
        ensure!(
            parents == expected_parents,
            "reviewed maintenance parent changed for {child}: {parents:?}"
        );
    }
    Ok(())
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
    if let Some(rule) = REVIEWED_MAINTENANCE.iter().find(|rule| rule.advisory == id) {
        object_keys(
            value,
            &["kind", "package", "advisory", "affected", "versions"],
        )?;
        ensure!(
            kind == Some("unmaintained")
                && identity.name == rule.name
                && identity.version == rule.version
                && identity.source.as_deref() == Some(SOURCE)
                && identity.checksum.as_deref() == Some(rule.checksum),
            "reviewed maintenance finding identity or classification changed"
        );
        object_keys(&value["versions"], &["patched", "unaffected"])?;
        ensure!(
            value["versions"]["patched"] == json!([])
                && value["versions"]["unaffected"] == json!([])
                && value["affected"] == Json::Null,
            "reviewed maintenance finding affected scope changed"
        );
        object_keys(
            advisory,
            &[
                "id",
                "package",
                "title",
                "description",
                "date",
                "aliases",
                "related",
                "collection",
                "categories",
                "keywords",
                "cvss",
                "informational",
                "references",
                "source",
                "url",
                "withdrawn",
                "license",
                "expect-deleted",
            ],
        )?;
        ensure!(
            advisory["cvss"] == Json::Null
                && advisory["source"] == Json::Null
                && advisory["categories"] == json!([])
                && advisory["keywords"] == json!([])
                && advisory["aliases"] == json!([])
                && advisory["related"] == json!([])
                && advisory["references"] == json!([])
                && advisory["date"] == rule.date
                && advisory["title"] == rule.title
                && advisory["url"] == rule.url.map_or(Json::Null, |url| json!(url))
                && advisory["license"] == "CC0-1.0"
                && advisory["expect-deleted"] == json!(false)
                && format!(
                    "{:x}",
                    Sha256::digest(field(advisory, "description")?.as_bytes())
                ) == rule.description,
            "reviewed maintenance advisory scope or severity changed"
        );
        return Ok(false);
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
            "supply: {label}: {} exact dormant advisory findings; {} retained maintenance notices; affected dormant exception versions absent from every checked graph",
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

    fn proc_macro_lock() -> String {
        format!(
            "{}\n[[package]]\nname = 'proc-macro-error2'\nversion = '2.0.1'\nsource = '{SOURCE}'\nchecksum = '{PROC_MACRO_CHECKSUM}'\n\n[[package]]\nname = 'subxt-macro'\nversion = '0.51.1'\nsource = '{SOURCE}'\nchecksum = '{SUBXT_MACRO_CHECKSUM}'\ndependencies = ['proc-macro-error2']\n\n[[package]]\nname = 'subxt'\nversion = '0.51.1'\nsource = '{SOURCE}'\nchecksum = '{SUBXT_CHECKSUM}'\ndependencies = ['subxt-macro']\n",
            lock()
        )
    }

    fn proc_macro_report() -> Result<Json> {
        let mut report = report();
        report["lockfile"]["dependency-count"] = json!(4);
        // Actual unfiltered hosted cargo-audit finding, retained independently
        // from the classifier's constants. See the execution evidence document.
        let finding: Json = serde_json::from_str(include_str!(
            "../tests/fixtures/supply/proc-macro-error2.json"
        ))?;
        report["warnings"]["unmaintained"] = json!([finding]);
        Ok(report)
    }

    fn smallstr_lock() -> String {
        format!(
            "{}\n[[package]]\nname = 'smallstr'\nversion = '0.3.1'\nsource = '{SOURCE}'\nchecksum = '{SMALLSTR_CHECKSUM}'\n\n[[package]]\nname = 'frame-decode'\nversion = '0.18.1'\nsource = '{SOURCE}'\nchecksum = '{FRAME_DECODE_CHECKSUM}'\ndependencies = ['scale-info-legacy']\n\n[[package]]\nname = 'scale-info-legacy'\nversion = '0.4.2'\nsource = '{SOURCE}'\nchecksum = '{SCALE_INFO_LEGACY_CHECKSUM}'\ndependencies = ['smallstr']\n\n[[package]]\nname = 'subxt'\nversion = '0.51.1'\nsource = '{SOURCE}'\nchecksum = '{SUBXT_CHECKSUM}'\ndependencies = ['scale-info-legacy', 'subxt-metadata']\n\n[[package]]\nname = 'subxt-metadata'\nversion = '0.51.1'\nsource = '{SOURCE}'\nchecksum = '{SUBXT_METADATA_CHECKSUM}'\ndependencies = ['scale-info-legacy']\n",
            lock()
        )
    }

    fn smallstr_report() -> Result<Json> {
        let mut report = report();
        report["lockfile"]["dependency-count"] = json!(6);
        // Actual warning from hosted run 38734177643, artifact 11973500903.
        // The independent raw fixture includes the original advisory text.
        let finding: Json =
            serde_json::from_str(include_str!("../tests/fixtures/supply/smallstr.json"))?;
        report["warnings"]["unmaintained"] = json!([finding]);
        Ok(report)
    }

    #[test]
    fn active_smallstr_notice_is_retained_without_a_dormancy_claim() -> Result<()> {
        let locks = lock_identities(&smallstr_lock())?;
        let report = smallstr_report()?;
        let original = report.clone();
        ensure!(
            validate_audit(&report, &locks)?
                == Audit {
                    exceptions: 1,
                    maintenance: 1,
                    failing_status: true,
                }
        );
        ensure!(report == original, "classification mutated the raw audit");
        check_graph(
            "rtao v0.1.0\nsubxt v0.51.1\nsubxt-metadata v0.51.1\nframe-decode v0.18.1\nscale-info-legacy v0.4.2\nsmallstr v0.3.1\n",
            &["rtao".into()],
        )?;
        #[cfg(unix)]
        {
            let terminal = format!(
                "      Loaded 1300 security advisories\n    Scanning Cargo.lock for vulnerabilities (6 crate dependencies)\nID:        {ADVISORY}\nID:        {SMALLSTR_ADVISORY}\nerror: 1 denied warning found!\n"
            );
            ensure!(
                raw_audit(1, &terminal, &report)?
                    .validate(&locks)?
                    .maintenance
                    == 1
            );
            ensure!(
                raw_audit(1, &terminal.replace(SMALLSTR_ADVISORY, ""), &report)?
                    .validate(&locks)
                    .is_err()
            );
            let mut only_maintenance = report.clone();
            only_maintenance["warnings"]["unsound"] = json!([]);
            let terminal = terminal
                .replace(&format!("ID:        {ADVISORY}\n"), "")
                .replace("error: 1 denied warning found!\n", "");
            ensure!(
                raw_audit(0, &terminal, &only_maintenance)?.validate(&locks)?
                    == Audit {
                        exceptions: 0,
                        maintenance: 1,
                        failing_status: false,
                    }
            );
        }
        Ok(())
    }

    #[test]
    fn smallstr_notice_cannot_hide_a_vulnerability_or_new_advisory() -> Result<()> {
        let locks = lock_identities(&smallstr_lock())?;
        let original = smallstr_report()?;
        let mut vulnerability = original.clone();
        let mut finding = vulnerability["warnings"]["unmaintained"][0].take();
        finding
            .as_object_mut()
            .context("fixture warning is not an object")?
            .remove("kind");
        finding["advisory"]["informational"] = Json::Null;
        vulnerability["warnings"]["unmaintained"] = json!([]);
        vulnerability["vulnerabilities"] = json!({"found": true, "count": 1, "list": [finding]});
        ensure!(validate_audit(&vulnerability, &locks).is_err());
        for kind in ["unsound", "notice"] {
            let mut report = original.clone();
            let mut warning = report["warnings"]["unmaintained"][0].take();
            warning["kind"] = json!(kind);
            warning["advisory"]["informational"] = json!(kind);
            report["warnings"]["unmaintained"] = json!([]);
            report["warnings"][kind] = json!([warning]);
            ensure!(validate_audit(&report, &locks).is_err());
        }
        let mut new_advisory = original;
        new_advisory["warnings"]["unmaintained"][0]["advisory"]["id"] = json!("RUSTSEC-2099-9999");
        ensure!(validate_audit(&new_advisory, &locks).is_err());
        Ok(())
    }

    #[test]
    fn smallstr_notice_rejects_changed_identity_even_when_audited() -> Result<()> {
        let locks = lock_identities(&smallstr_lock())?;
        let original = smallstr_report()?;
        for (key, value) in [
            ("name", "another-package"),
            ("version", "0.3.2"),
            ("source", "git+https://example.test/smallstr#1234"),
            ("checksum", "changed-checksum"),
        ] {
            let mut report = original.clone();
            let finding = &mut report["warnings"]["unmaintained"][0];
            let old = json_identity(&finding["package"])?;
            finding["package"][key] = json!(value);
            if key == "name" {
                finding["advisory"]["package"] = json!(value);
            }
            let mut updated_locks = locks.clone();
            updated_locks.remove(&old);
            updated_locks.insert(json_identity(&finding["package"])?);
            ensure!(validate_audit(&report, &updated_locks).is_err());
        }
        Ok(())
    }

    #[test]
    fn smallstr_notice_rejects_scope_severity_and_advisory_changes() -> Result<()> {
        let locks = lock_identities(&smallstr_lock())?;
        let original = smallstr_report()?;
        for (key, value) in [
            (
                "cvss",
                json!("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H"),
            ),
            ("categories", json!(["denial-of-service"])),
            ("aliases", json!(["CVE-2099-1234"])),
            ("description", json!("A newly reported correctness issue")),
            ("date", json!("2026-07-23")),
            ("url", json!("https://example.test/new-evidence")),
            ("severity", json!("high")),
        ] {
            let mut report = original.clone();
            report["warnings"]["unmaintained"][0]["advisory"][key] = value;
            ensure!(validate_audit(&report, &locks).is_err());
        }
        for (key, value) in [
            ("patched", json!([">=0.3.2"])),
            ("unaffected", json!(["<0.3.1"])),
        ] {
            let mut report = original.clone();
            report["warnings"]["unmaintained"][0]["versions"][key] = value;
            ensure!(validate_audit(&report, &locks).is_err());
        }
        let mut missing_scope = original.clone();
        missing_scope["warnings"]["unmaintained"][0]
            .as_object_mut()
            .context("fixture warning is not an object")?
            .remove("affected");
        ensure!(validate_audit(&missing_scope, &locks).is_err());
        let mut report = original;
        report["warnings"]["unmaintained"][0]["affected"] =
            json!({"functions": {"smallstr::SmallString::push_str": ["*"]}});
        ensure!(validate_audit(&report, &locks).is_err());
        Ok(())
    }

    #[test]
    fn smallstr_maintenance_chain_requires_exact_parents_and_identities() -> Result<()> {
        let original = smallstr_lock();
        lock_identities(&original)?;
        for changed in [
            original.replace("0.3.1", "0.3.2"),
            original.replace("0.4.2", "0.4.3"),
            original.replace("0.51.1", "0.51.2"),
            original.replace(SMALLSTR_CHECKSUM, "changed"),
            original.replace(SCALE_INFO_LEGACY_CHECKSUM, "changed"),
            original.replace(SUBXT_METADATA_CHECKSUM, "changed"),
            original.replace(FRAME_DECODE_CHECKSUM, "changed"),
            original.replace(SUBXT_CHECKSUM, "changed"),
            original.replace(SOURCE, "git+https://example.test/changed#1234"),
            original.replace("dependencies = ['smallstr']", "dependencies = []"),
            original.replace(
                "dependencies = ['smallstr']",
                "dependencies = ['smallstr 0.3.2']",
            ),
            original.replace("dependencies = ['scale-info-legacy']", "dependencies = []"),
            original.replace(
                "['scale-info-legacy', 'subxt-metadata']",
                "['subxt-metadata']",
            ),
            format!(
                "{original}\n[[package]]\nname = 'new-parent'\nversion = '1.0.0'\ndependencies = ['smallstr']\n"
            ),
            format!(
                "{original}\n[[package]]\nname = 'new-parent'\nversion = '1.0.0'\ndependencies = ['scale-info-legacy']\n"
            ),
        ] {
            ensure!(lock_identities(&changed).is_err());
        }
        // Removal does not need an exception, and does not weaken unrelated gates.
        lock_identities(&lock())?;
        Ok(())
    }

    #[test]
    fn active_proc_macro_maintenance_notice_stays_visible_and_nonblocking() -> Result<()> {
        let locks = lock_identities(&proc_macro_lock())?;
        let report = proc_macro_report()?;
        ensure!(
            validate_audit(&report, &locks)?
                == Audit {
                    exceptions: 1,
                    maintenance: 1,
                    failing_status: true,
                }
        );
        check_graph(
            "rtao v0.1.0\nsubxt v0.51.1\nsubxt-macro v0.51.1\nproc-macro-error2 v2.0.1\n",
            &["rtao".into()],
        )?;
        #[cfg(unix)]
        {
            let terminal = format!(
                "      Loaded 1300 security advisories\n    Scanning Cargo.lock for vulnerabilities (4 crate dependencies)\nID:        {ADVISORY}\nID:        {PROC_MACRO_ADVISORY}\nerror: 1 denied warning found!\n"
            );
            ensure!(
                raw_audit(1, &terminal, &report)?
                    .validate(&locks)?
                    .maintenance
                    == 1
            );
            ensure!(
                raw_audit(1, &terminal.replace(PROC_MACRO_ADVISORY, ""), &report)?
                    .validate(&locks)
                    .is_err()
            );
        }
        Ok(())
    }

    #[test]
    fn proc_macro_notice_cannot_hide_a_vulnerability_or_new_advisory() -> Result<()> {
        let locks = lock_identities(&proc_macro_lock())?;
        let original = proc_macro_report()?;
        let mut vulnerability = original.clone();
        let mut finding = vulnerability["warnings"]["unmaintained"][0].take();
        finding
            .as_object_mut()
            .context("fixture warning is not an object")?
            .remove("kind");
        finding["advisory"]["informational"] = Json::Null;
        vulnerability["warnings"]["unmaintained"] = json!([]);
        vulnerability["vulnerabilities"] = json!({"found": true, "count": 1, "list": [finding]});
        ensure!(validate_audit(&vulnerability, &locks).is_err());
        for kind in ["unsound", "notice"] {
            let mut report = original.clone();
            let mut warning = report["warnings"]["unmaintained"][0].take();
            warning["kind"] = json!(kind);
            warning["advisory"]["informational"] = json!(kind);
            report["warnings"]["unmaintained"] = json!([]);
            report["warnings"][kind] = json!([warning]);
            ensure!(validate_audit(&report, &locks).is_err());
        }
        let mut new_advisory = original;
        new_advisory["warnings"]["unmaintained"][0]["advisory"]["id"] = json!("RUSTSEC-2099-9999");
        ensure!(validate_audit(&new_advisory, &locks).is_err());
        Ok(())
    }

    #[test]
    fn proc_macro_notice_rejects_changed_identity_even_when_audited() -> Result<()> {
        let locks = lock_identities(&proc_macro_lock())?;
        let original = proc_macro_report()?;
        for (key, value) in [
            ("name", "another-package"),
            ("version", "2.0.2"),
            ("source", "git+https://example.test/proc-macro-error2#1234"),
            ("checksum", "changed-checksum"),
        ] {
            let mut report = original.clone();
            let finding = &mut report["warnings"]["unmaintained"][0];
            let old = json_identity(&finding["package"])?;
            finding["package"][key] = json!(value);
            if key == "name" {
                finding["advisory"]["package"] = json!(value);
            }
            let mut updated_locks = locks.clone();
            updated_locks.remove(&old);
            updated_locks.insert(json_identity(&finding["package"])?);
            ensure!(validate_audit(&report, &updated_locks).is_err());
        }
        Ok(())
    }

    #[test]
    fn proc_macro_notice_rejects_scope_severity_and_description_changes() -> Result<()> {
        let locks = lock_identities(&proc_macro_lock())?;
        let original = proc_macro_report()?;
        for (key, value) in [
            (
                "cvss",
                json!("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H"),
            ),
            ("categories", json!(["denial-of-service"])),
            ("aliases", json!(["CVE-2099-1234"])),
            ("description", json!("A newly reported correctness issue")),
            ("severity", json!("high")),
        ] {
            let mut report = original.clone();
            report["warnings"]["unmaintained"][0]["advisory"][key] = value;
            ensure!(validate_audit(&report, &locks).is_err());
        }
        for (key, value) in [
            ("patched", json!([">=2.0.2"])),
            ("unaffected", json!(["<2.0.1"])),
        ] {
            let mut report = original.clone();
            report["warnings"]["unmaintained"][0]["versions"][key] = value;
            ensure!(validate_audit(&report, &locks).is_err());
        }
        let mut report = original;
        report["warnings"]["unmaintained"][0]["affected"] =
            json!({"functions": {"helper::entry_point": ["*"]}});
        ensure!(validate_audit(&report, &locks).is_err());
        Ok(())
    }

    #[test]
    fn proc_macro_maintenance_chain_must_keep_reviewed_packages_and_edges() -> Result<()> {
        let original = proc_macro_lock();
        for changed in [
            original.replace("2.0.1", "2.0.2"),
            original.replace(PROC_MACRO_CHECKSUM, "changed"),
            original.replace(SUBXT_CHECKSUM, "changed"),
            original.replace(SUBXT_MACRO_CHECKSUM, "changed"),
            original.replace("0.51.1", "0.51.2"),
            original.replace(SOURCE, "git+https://example.test/changed#1234"),
            original.replace("dependencies = ['proc-macro-error2']", "dependencies = []"),
            original.replace(
                "dependencies = ['proc-macro-error2']",
                "dependencies = ['proc-macro-error2 2.0.2']",
            ),
            original.replace("dependencies = ['subxt-macro']", "dependencies = []"),
            format!(
                "{original}\n[[package]]\nname = 'new-parent'\nversion = '1.0.0'\ndependencies = ['proc-macro-error2']\n"
            ),
        ] {
            ensure!(lock_identities(&changed).is_err());
        }
        Ok(())
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
