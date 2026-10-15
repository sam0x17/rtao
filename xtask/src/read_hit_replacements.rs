//! Exact descriptor substitutions justified by active, pinned divergences.

use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path},
};

use anyhow::{Context, Result, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};
use toml_edit::{DocumentMut, Item, Table};

type Hit = (String, String, String);

/// A replacement admitted by source, runtime, metadata and divergence checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Witness {
    /// Active registered divergence.
    pub(crate) divergence: String,
    /// Authenticated Python source descriptor.
    pub(crate) from: Hit,
    /// Corrected effective descriptor.
    pub(crate) to: Hit,
}

fn identity(hit: &Hit) -> String {
    format!("{}:{}.{}", hit.0, hit.1, hit.2)
}

fn field<'a>(row: &'a toml_edit::InlineTable, name: &str) -> Result<&'a str> {
    row.get(name)
        .and_then(toml_edit::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .with_context(|| format!("read hit replacement requires {name}"))
}

fn pinned_source(root: &Path, pins: &DocumentMut, path: &str, expected: &str) -> Result<Vec<u8>> {
    ensure!(
        !Path::new(path).is_absolute()
            && Path::new(path)
                .components()
                .all(|part| matches!(part, Component::Normal(_))),
        "replacement source path must be relative and bounded"
    );
    let artifact = pins
        .get("artifacts")
        .and_then(|value| value.get(path))
        .context("replacement citation is not an authenticated artifact")?;
    let relative = format!("vendor/upstream/src/{path}");
    ensure!(
        artifact.get("to").and_then(Item::as_str) == Some(relative.as_str()),
        "replacement artifact destination mismatch"
    );
    ensure!(
        artifact.get("sha256").and_then(Item::as_str) == Some(expected),
        "replacement citation SHA differs from the pin"
    );
    let bytes = fs::read(root.join(relative))?;
    ensure!(
        format!("{:x}", Sha256::digest(&bytes)) == expected,
        "replacement cited source changed"
    );
    Ok(bytes)
}

fn divergence<'a>(
    document: &'a DocumentMut,
    id: &str,
    read: &str,
    symbol: &str,
    mapping: &str,
) -> Result<&'a Table> {
    let rows = document
        .get("divergences")
        .and_then(Item::as_array_of_tables)
        .context("divergence register missing")?;
    let found = rows
        .iter()
        .filter(|row| row.get("id").and_then(Item::as_str) == Some(id))
        .collect::<Vec<_>>();
    ensure!(
        found.len() == 1,
        "replacement needs exactly one registered divergence {id}"
    );
    let row = found[0];
    ensure!(
        row.get("status").and_then(Item::as_str).unwrap_or("active") == "active",
        "replacement divergence {id} must be active"
    );
    ensure!(
        row.get("class").and_then(Item::as_str) == Some("improved"),
        "replacement must register a proved correction"
    );
    ensure!(
        row.get("upstream_symbol").and_then(Item::as_str) == Some(symbol),
        "replacement divergence is for a different upstream symbol"
    );
    ensure!(
        row.get("read_hit_read").and_then(Item::as_str) == Some(read)
            && row.get("read_hit_replacement").and_then(Item::as_str) == Some(mapping),
        "replacement does not match its exact divergence mapping"
    );
    ensure!(
        row.get("pinning_tests")
            .and_then(Item::as_array)
            .is_some_and(|tests| !tests.is_empty()
                && tests.iter().all(|test| test
                    .as_str()
                    .is_some_and(|name| name.starts_with("rtao_reads::")))),
        "replacement needs read pinning tests; parity validates their harness eligibility"
    );
    Ok(row)
}

fn metadata_contains(hit: &Hit) -> bool {
    match hit.0.as_str() {
        "storage" => rtao_runtime::storage::ALL
            .iter()
            .any(|entry| entry.pallet == hit.1 && entry.name == hit.2),
        "constant" => rtao_runtime::constants::ALL
            .iter()
            .any(|entry| entry.pallet == hit.1 && entry.name == hit.2),
        "runtime" => rtao_runtime::runtime_apis::ALL
            .iter()
            .any(|entry| entry.api == hit.1 && entry.name == hit.2),
        _ => false,
    }
}

/// Preserve authenticated hit evidence and return the exact effective coverage.
///
/// Replacements require source identity, pinned runtime proof and an active
/// divergence with this exact read/from/to mapping. Rejection leaves `hits`
/// untouched. Raw extraction and catalog records are never modified.
pub(crate) fn apply(
    root: &Path,
    name: &str,
    source: &Value,
    catalog: &BTreeSet<Hit>,
    hits: &mut BTreeSet<Hit>,
    reviewed: Option<&Item>,
) -> Result<(BTreeSet<Hit>, Vec<Witness>)> {
    ensure!(
        catalog.is_subset(hits),
        "{name}: authenticated catalog hits missing before replacements"
    );
    let Some(replacements) = reviewed.and_then(|read| read.get("replacements")) else {
        return Ok((catalog.clone(), Vec::new()));
    };
    let pins: DocumentMut = fs::read_to_string(root.join("upstream.toml"))?.parse()?;
    let registry: DocumentMut =
        fs::read_to_string(root.join("parity/divergences.toml"))?.parse()?;
    apply_documents(
        root,
        name,
        source,
        catalog,
        hits,
        replacements,
        &pins,
        &registry,
    )
}

#[allow(clippy::too_many_arguments)]
fn apply_documents(
    root: &Path,
    name: &str,
    source: &Value,
    catalog: &BTreeSet<Hit>,
    hits: &mut BTreeSet<Hit>,
    replacements: &Item,
    pins: &DocumentMut,
    registry: &DocumentMut,
) -> Result<(BTreeSet<Hit>, Vec<Witness>)> {
    let rows = replacements
        .as_array()
        .context("read replacements must be an array")?;
    ensure!(!rows.is_empty(), "read replacements must not be empty");
    let mut effective = hits.clone();
    let mut effective_catalog = catalog.clone();
    let mut sources = BTreeSet::new();
    let mut targets = BTreeSet::new();
    let mut witnesses = Vec::new();
    for row in rows {
        let row = row
            .as_inline_table()
            .context("read replacement must be an inline table")?;
        let required = [
            "divergence",
            "from_kind",
            "from_container",
            "from_name",
            "kind",
            "container",
            "name",
            "source_path",
            "source_blob",
            "source_sha256",
            "runtime_path",
            "runtime_sha256",
            "runtime_predicate",
            "reason",
        ];
        ensure!(
            row.len() == required.len() && required.iter().all(|key| row.contains_key(key)),
            "read replacement fields changed"
        );
        let get = |key| field(row, key);
        let from = (
            get("from_kind")?.to_owned(),
            get("from_container")?.to_owned(),
            get("from_name")?.to_owned(),
        );
        let to = (
            get("kind")?.to_owned(),
            get("container")?.to_owned(),
            get("name")?.to_owned(),
        );
        ensure!(
            from != to && sources.insert(from.clone()) && targets.insert(to.clone()),
            "read replacement is duplicate or has no effect"
        );
        ensure!(
            source["hits"]
                .as_array()
                .context("authenticated read source hits absent")?
                .iter()
                .any(|hit| hit["kind"] == from.0
                    && hit["container"] == from.1
                    && hit["name"] == from.2),
            "replacement source is not an authenticated extracted hit"
        );
        ensure!(
            effective.contains(&from) && !effective.contains(&to),
            "replacement source absent or target already present"
        );
        ensure!(
            metadata_contains(&from) && metadata_contains(&to),
            "replacement source/target descriptor absent from metadata"
        );
        let path = get("source_path")?;
        ensure!(
            source["provenance"]["path"] == path
                && source["provenance"]["blob"] == get("source_blob")?
                && source["provenance"]["sha256"] == get("source_sha256")?,
            "replacement does not bind the authenticated read source identity"
        );
        let bytes = pinned_source(root, pins, path, get("source_sha256")?)?;
        ensure!(
            crate::extract::git_blob(&bytes) == get("source_blob")?,
            "replacement source blob changed"
        );
        let module = path
            .strip_prefix("sdk/python/")
            .and_then(|path| path.strip_suffix(".py"))
            .context("replacement Python source path is not a module")?
            .replace('/', ".");
        let mapping = format!("{} -> {}", identity(&from), identity(&to));
        let _ = divergence(
            registry,
            get("divergence")?,
            name,
            &format!("{module}.{name}"),
            &mapping,
        )?;
        ensure!(
            get("reason")?.len() >= 12,
            "replacement needs a concrete reason"
        );
        let runtime = pinned_source(root, pins, get("runtime_path")?, get("runtime_sha256")?)?;
        let runtime = std::str::from_utf8(&runtime).context("runtime citation must be UTF-8")?;
        let predicate = get("runtime_predicate")?;
        ensure!(
            predicate.contains(&to.2) && runtime.contains(predicate),
            "replacement runtime predicate is absent or omits the target descriptor"
        );
        ensure!(
            effective.remove(&from) && effective.insert(to.clone()),
            "replacement could not preserve exact coverage"
        );
        if effective_catalog.remove(&from) {
            effective_catalog.insert(to.clone());
        }
        witnesses.push(Witness {
            divergence: get("divergence")?.to_owned(),
            from,
            to,
        });
    }
    ensure!(
        effective_catalog.is_subset(&effective),
        "replacement loses catalog coverage"
    );
    *hits = effective;
    Ok((effective_catalog, witnesses))
}

/// Require the generated witness inventory to equal independently admitted data.
pub(crate) fn validate_compiled(
    expected: &BTreeSet<(String, String, Hit, Hit)>,
    actual: &[rtao_reads::ReadHitReplacement],
) -> Result<()> {
    let identity = |hit: &rtao_reads::Hit| {
        let kind = match hit.kind {
            rtao_reads::HitKind::Storage => "storage",
            rtao_reads::HitKind::RuntimeApi => "runtime",
            rtao_reads::HitKind::Constant => "constant",
        };
        (
            kind.to_owned(),
            hit.container.to_owned(),
            hit.name.to_owned(),
        )
    };
    let compiled: BTreeSet<_> = actual
        .iter()
        .map(|row| {
            (
                row.read.to_owned(),
                row.divergence.to_owned(),
                identity(&row.from),
                identity(&row.to),
            )
        })
        .collect();
    ensure!(
        compiled.len() == actual.len() && &compiled == expected,
        "generated read replacement witnesses are missing, duplicate or unexplained"
    );
    for row in actual {
        let read = rtao_reads::READ_SPECS
            .iter()
            .find(|read| read.name == row.read)
            .context("replacement witness names unknown read")?;
        ensure!(
            !read.hits.contains(&row.from) && read.hits.contains(&row.to),
            "replacement witness source/target disagrees with effective coverage"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use toml_edit::value;

    struct Fixture {
        root: std::path::PathBuf,
        source: Value,
        catalog: BTreeSet<Hit>,
        hits: BTreeSet<Hit>,
        replacements: Item,
        pins: DocumentMut,
        registry: DocumentMut,
    }
    impl Fixture {
        fn load() -> Result<Self> {
            let root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .context("workspace")?
                .to_owned();
            let extracted: Value = serde_json::from_slice(&fs::read(
                root.join("vendor/upstream/extracted/read_hits.json"),
            )?)?;
            let source = extracted["reads"]
                .as_array()
                .context("read records")?
                .iter()
                .find(|read| read["name"] == "subnet_start_schedule")
                .context("read absent")?
                .clone();
            let hits: BTreeSet<Hit> = source["hits"]
                .as_array()
                .context("hits")?
                .iter()
                .map(|hit| {
                    Ok((
                        hit["kind"].as_str().context("kind")?.into(),
                        hit["container"].as_str().context("container")?.into(),
                        hit["name"].as_str().context("name")?.into(),
                    ))
                })
                .collect::<Result<_>>()?;
            let extras: DocumentMut =
                fs::read_to_string(root.join("parity/read_hits_extra.toml"))?.parse()?;
            Ok(Self {
                pins: fs::read_to_string(root.join("upstream.toml"))?.parse()?,
                registry: fs::read_to_string(root.join("parity/divergences.toml"))?.parse()?,
                root,
                source,
                catalog: hits.clone(),
                hits,
                replacements: extras["read"]["subnet_start_schedule"]["replacements"].clone(),
            })
        }
        fn apply(&mut self) -> Result<(BTreeSet<Hit>, Vec<Witness>)> {
            apply_documents(
                &self.root,
                "subnet_start_schedule",
                &self.source,
                &self.catalog,
                &mut self.hits,
                &self.replacements,
                &self.pins,
                &self.registry,
            )
        }
        fn replacement(&mut self) -> Result<&mut toml_edit::InlineTable> {
            self.replacements
                .as_array_mut()
                .context("replacement array")?
                .get_mut(0)
                .and_then(toml_edit::Value::as_inline_table_mut)
                .context("replacement row")
        }
        fn divergence(&mut self) -> Result<&mut Table> {
            self.registry["divergences"]
                .as_array_of_tables_mut()
                .context("divergences")?
                .iter_mut()
                .find(|row| row["id"].as_str() == Some("D-040"))
                .context("D-040")
        }
    }
    #[test]
    fn d040_exact_replacement_keeps_authenticated_raw_evidence() -> Result<()> {
        let mut f = Fixture::load()?;
        let raw = f.source.clone();
        let catalog = f.catalog.clone();
        let (effective, witnesses) = f.apply()?;
        let old = (
            "constant".into(),
            "SubtensorModule".into(),
            "InitialStartCallDelay".into(),
        );
        let new = (
            "storage".into(),
            "SubtensorModule".into(),
            "StartCallDelay".into(),
        );
        assert_eq!(witnesses.len(), 1);
        assert_eq!(witnesses[0].divergence, "D-040");
        assert_eq!(witnesses[0].from, old);
        assert_eq!(witnesses[0].to, new);
        assert!(catalog.contains(&old));
        assert!(!effective.contains(&old));
        assert!(effective.contains(&new));
        assert!(f.hits.contains(&new));
        assert_eq!(f.source, raw);
        assert_eq!(f.catalog, catalog);
        assert_eq!(f.hits.len(), 2);
        Ok(())
    }
    #[test]
    fn d040_compiled_witness_inventory_rejects_wrong_missing_and_duplicate_entries() -> Result<()> {
        let mut fixture = Fixture::load()?;
        let (_, admitted) = fixture.apply()?;
        let expected = admitted
            .into_iter()
            .map(|witness| {
                (
                    "subnet_start_schedule".to_owned(),
                    witness.divergence,
                    witness.from,
                    witness.to,
                )
            })
            .collect();
        let actual = rtao_reads::READ_HIT_REPLACEMENTS;
        validate_compiled(&expected, actual)?;
        let original = *actual.first().context("compiled D-040 witness missing")?;
        assert!(validate_compiled(&expected, &[]).is_err());
        assert!(validate_compiled(&expected, &[original, original]).is_err());
        for wrong in [
            rtao_reads::ReadHitReplacement {
                read: "balance",
                ..original
            },
            rtao_reads::ReadHitReplacement {
                divergence: "D-999",
                ..original
            },
            rtao_reads::ReadHitReplacement {
                from: rtao_reads::Hit {
                    kind: rtao_reads::HitKind::Storage,
                    ..original.from
                },
                ..original
            },
            rtao_reads::ReadHitReplacement {
                to: rtao_reads::Hit {
                    name: "NetworkRegisteredAt",
                    ..original.to
                },
                ..original
            },
            rtao_reads::ReadHitReplacement {
                to: rtao_reads::Hit {
                    name: "DoesNotExist",
                    ..original.to
                },
                ..original
            },
        ] {
            assert!(validate_compiled(&expected, &[wrong]).is_err());
        }
        Ok(())
    }
    #[test]
    fn d040_replacement_refuses_unregistered_pending_and_mismatched_divergences() -> Result<()> {
        for (key, bad) in [
            ("status", "pending"),
            ("class", "stricter"),
            ("read_hit_read", "balance"),
            (
                "read_hit_replacement",
                "storage:SubtensorModule.Other -> storage:SubtensorModule.StartCallDelay",
            ),
            ("upstream_symbol", "bittensor.reads.subnets.other"),
        ] {
            let mut f = Fixture::load()?;
            f.divergence()?[key] = value(bad);
            let initial = f.hits.clone();
            assert!(f.apply().is_err(), "{key}");
            assert_eq!(f.hits, initial);
        }
        let mut f = Fixture::load()?;
        f.replacement()?.insert("divergence", "D-999".into());
        assert!(f.apply().is_err());
        Ok(())
    }
    #[test]
    fn d040_replacement_refuses_stale_source_runtime_and_missing_descriptors() -> Result<()> {
        for (key, bad) in [
            ("from_kind", "storage"),
            ("from_container", "Other"),
            ("from_name", "StartCallDelay"),
            ("source_path", "sdk/python/bittensor/reads/accounts.py"),
            ("source_blob", "0000000000000000000000000000000000000000"),
            (
                "source_sha256",
                "0000000000000000000000000000000000000000000000000000000000000000",
            ),
            (
                "runtime_sha256",
                "0000000000000000000000000000000000000000000000000000000000000000",
            ),
            ("runtime_predicate", "missing StartCallDelay predicate"),
            ("name", "DoesNotExist"),
            ("kind", "bogus"),
            ("runtime_path", "../../Cargo.toml"),
        ] {
            let mut f = Fixture::load()?;
            f.replacement()?.insert(key, bad.into());
            let initial = f.hits.clone();
            assert!(f.apply().is_err(), "{key}");
            assert_eq!(f.hits, initial);
        }
        Ok(())
    }
    #[test]
    fn d040_replacement_refuses_missing_duplicate_and_already_present_hits() -> Result<()> {
        let mut f = Fixture::load()?;
        f.hits.remove(&(
            "constant".into(),
            "SubtensorModule".into(),
            "InitialStartCallDelay".into(),
        ));
        assert!(f.apply().is_err());
        let mut f = Fixture::load()?;
        f.hits.insert((
            "storage".into(),
            "SubtensorModule".into(),
            "StartCallDelay".into(),
        ));
        assert!(f.apply().is_err());
        let mut f = Fixture::load()?;
        let row = f
            .replacements
            .as_array()
            .context("rows")?
            .get(0)
            .context("row")?
            .clone();
        f.replacements.as_array_mut().context("rows")?.push(row);
        let initial = f.hits.clone();
        assert!(f.apply().is_err());
        assert_eq!(f.hits, initial);
        let mut f = Fixture::load()?;
        f.replacement()?.insert("skip_coverage", true.into());
        assert!(f.apply().is_err());
        Ok(())
    }
}
