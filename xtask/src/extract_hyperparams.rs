// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_hyperparams_units.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Hyperparameter units from authenticated resolved Python declarations.

use std::{cmp::Ordering, collections::BTreeSet};

use anyhow::{Context, Result, ensure};
use serde_json::Value;

use super::{header, rust_string, rustfmt, value};

const SOURCE: &str = "sdk/python/bittensor/hyperparams.py";

fn dictionary<'a>(document: &'a Value, name: &str) -> Result<Vec<(&'a str, &'a Value)>> {
    let pairs = value(document, name)?["dict"]
        .as_array()
        .with_context(|| format!("{name} must be a dictionary"))?;
    let mut seen = BTreeSet::new();
    pairs
        .iter()
        .map(|pair| {
            let pair = pair
                .as_array()
                .with_context(|| format!("{name} entry must be a key/value pair"))?;
            ensure!(pair.len() == 2, "{name} entry must have two values");
            let key = pair[0]
                .as_str()
                .with_context(|| format!("{name} key must be a string"))?;
            ensure!(!key.is_empty(), "{name} key must not be empty");
            ensure!(seen.insert(key), "{name} repeats key {key}");
            Ok((key, &pair[1]))
        })
        .collect()
}

fn string<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value[field]
        .as_str()
        .with_context(|| format!("hyperparameter {field} must be a string"))
}

#[derive(Clone, Copy, Debug)]
enum Integer {
    Signed(i128),
    Unsigned(u128),
}

impl Integer {
    fn parse(value: &Value) -> Result<Self> {
        ensure!(
            value.is_number(),
            "raw hyperparameter bound must be an integer"
        );
        let number = value.to_string();
        if number.starts_with('-') {
            Ok(Self::Signed(
                number.parse().context("raw bound must fit i128")?,
            ))
        } else {
            Ok(Self::Unsigned(
                number.parse().context("raw bound must fit u128")?,
            ))
        }
    }

    fn compare(self, other: Self) -> Ordering {
        match (self, other) {
            (Self::Signed(left), Self::Signed(right)) => left.cmp(&right),
            (Self::Unsigned(left), Self::Unsigned(right)) => left.cmp(&right),
            (Self::Signed(left), Self::Unsigned(right)) => {
                u128::try_from(left).map_or(Ordering::Less, |left| left.cmp(&right))
            }
            (Self::Unsigned(left), Self::Signed(right)) => {
                u128::try_from(right).map_or(Ordering::Greater, |right| left.cmp(&right))
            }
        }
    }

    fn render(self) -> String {
        match self {
            Self::Signed(value) => format!("RawInteger::Signed({value})"),
            Self::Unsigned(value) => format!("RawInteger::Unsigned({value})"),
        }
    }
}

fn bound(fields: &Value, name: &str) -> Result<Option<Integer>> {
    let value = fields
        .get(name)
        .with_context(|| format!("missing {name} bound"))?;
    if value.is_null() {
        Ok(None)
    } else {
        Integer::parse(value).map(Some)
    }
}

fn render_bound(value: Option<Integer>) -> String {
    value.map_or_else(
        || "None".into(),
        |value| format!("Some({})", value.render()),
    )
}

fn codec(value: &Value, label: &str) -> Result<(bool, u8)> {
    let values = value["tuple"]
        .as_array()
        .with_context(|| format!("{label} codec must be a tuple"))?;
    ensure!(
        values.len() == 2,
        "{label} codec must have signedness and bits"
    );
    let signed = values[0]
        .as_bool()
        .context("codec signedness must be bool")?;
    let bits = u8::try_from(
        values[1]
            .as_u64()
            .context("codec bits must be an integer")?,
    )?;
    ensure!(
        (1..=128).contains(&bits),
        "{label} codec bits must be in 1..=128"
    );
    Ok((signed, bits))
}

fn render_codecs(out: &mut String, document: &Value, source: &str, field: &str) -> Result<()> {
    out.push_str(&format!("{field}: &[\n"));
    for (name, value) in dictionary(document, source)? {
        let (signed, bits) = codec(value, name)?;
        out.push_str(&format!("({},({signed},{bits})),\n", rust_string(name)));
    }
    out.push_str("],\n");
    Ok(())
}

fn render(document: &Value, settings: &Value) -> Result<String> {
    let kinds = value(document, "KINDS")?["tuple"]
        .as_array()
        .context("hyperparameter KINDS must be a tuple")?;
    let mut known_kinds = BTreeSet::new();
    for kind in kinds {
        let kind = kind
            .as_str()
            .context("hyperparameter kind must be a string")?;
        ensure!(
            !kind.is_empty() && known_kinds.insert(kind),
            "empty or repeated hyperparameter kind"
        );
    }
    ensure!(
        !known_kinds.is_empty(),
        "hyperparameter kinds must not be empty"
    );
    let kind_codecs = dictionary(document, "_KIND_CODEC")?;
    ensure!(
        kind_codecs
            .iter()
            .map(|(kind, _)| *kind)
            .collect::<BTreeSet<_>>()
            == known_kinds,
        "kind codec coverage differs from KINDS"
    );
    for (kind, value) in &kind_codecs {
        codec(value, kind)?;
    }
    let ident_kinds = dictionary(document, "_IDENT_KINDS")?;
    for (ident, kind) in &ident_kinds {
        ensure!(
            kind.as_str().is_some_and(|kind| known_kinds.contains(kind)),
            "{ident}: unit identity refers to an unknown kind"
        );
    }
    let storage = dictionary(document, "STORAGE_ITEMS")?;
    let entries = dictionary(document, "HYPERPARAMS")?;
    ensure!(!entries.is_empty(), "HYPERPARAMS must not be empty");
    let short_max = usize::try_from(
        value(document, "SHORT_MAX")?
            .as_u64()
            .context("SHORT_MAX must be an integer")?,
    )?;
    ensure!(short_max > 0, "SHORT_MAX must be positive");
    let mut out = header(document)?;
    out.push_str(&header(settings)?);
    out.push_str("//! Hyperparameter metadata and units extracted from the pinned Python package.\n\nuse rtao_primitives::hyperparams::{Hyperparam, HyperparamStorage, HyperparamTable, RawInteger};\n\n/// Authenticated Python hyperparameter data, with metadata-primary unit identities.\n#[doc(alias = \"bittensor.hyperparams.HYPERPARAMS\")]\npub static TABLE: HyperparamTable = HyperparamTable {\nentries: &[\n");
    for (name, declaration) in &entries {
        ensure!(
            declaration["constructor"].as_str() == Some("Hyperparam"),
            "{name}: expected a Hyperparam constructor"
        );
        let fields = &declaration["fields"];
        let fields_map = fields
            .as_object()
            .context("Hyperparam fields must be an object")?;
        ensure!(
            fields_map.len() == 7
                && [
                    "kind", "doc", "short", "signed", "bits", "minimum", "maximum"
                ]
                .iter()
                .all(|field| fields_map.contains_key(*field)),
            "{name}: Hyperparam fields changed"
        );
        let kind = string(fields, "kind")?;
        ensure!(
            known_kinds.contains(kind),
            "{name}: unknown unit kind {kind}"
        );
        let doc = string(fields, "doc")?;
        let short = string(fields, "short")?;
        ensure!(!short.is_empty(), "{name}: no short description");
        ensure!(
            short.chars().count() <= short_max,
            "{name}: short description exceeds {short_max} Unicode codepoints"
        );
        let signed = if fields["signed"].is_null() {
            None
        } else {
            Some(
                fields["signed"]
                    .as_bool()
                    .context("explicit signedness must be bool")?,
            )
        };
        let bits = if fields["bits"].is_null() {
            None
        } else {
            let bits = u8::try_from(
                fields["bits"]
                    .as_u64()
                    .context("explicit codec bits must be an integer")?,
            )?;
            ensure!(
                (1..=128).contains(&bits),
                "{name}: explicit codec bits must be in 1..=128"
            );
            Some(bits)
        };
        let minimum = bound(fields, "minimum")?;
        let maximum = bound(fields, "maximum")?;
        if let (Some(minimum), Some(maximum)) = (minimum, maximum) {
            ensure!(
                minimum.compare(maximum) != Ordering::Greater,
                "{name}: minimum exceeds maximum"
            );
        }
        if let Some((_, item)) = storage.iter().find(|(parameter, _)| parameter == name) {
            let metadata_ident = string(item, "value_type_ident")?;
            if let Some((_, metadata_kind)) = ident_kinds
                .iter()
                .find(|(ident, _)| *ident == metadata_ident)
            {
                let metadata_kind = metadata_kind
                    .as_str()
                    .context("unit kind must be a string")?;
                ensure!(
                    metadata_kind == kind,
                    "{name}: hand kind '{kind}' disagrees with metadata identity '{metadata_ident}' (kind '{metadata_kind}')"
                );
            }
        }
        out.push_str(&format!("({},Hyperparam {{kind:{},doc:{},short:{},signed:{signed:?},bits:{bits:?},minimum:{},maximum:{}}}),\n", rust_string(name), rust_string(kind), rust_string(doc), rust_string(short), render_bound(minimum), render_bound(maximum)));
    }
    out.push_str("],\nstorage: &[\n");
    for (parameter, item) in storage {
        ensure!(
            entries.iter().any(|(name, _)| *name == parameter),
            "{parameter}: storage item has no hyperparameter entry"
        );
        let container = string(item, "pallet")?;
        let name = string(item, "item")?;
        let source_ident = item
            .get("source_value_type_ident")
            .context("source storage identity field missing")?;
        let source_ident = if source_ident.is_null() {
            ""
        } else {
            source_ident
                .as_str()
                .context("source storage identity must be a string or absent")?
        };
        let metadata_ident = string(item, "value_type_ident")?;
        ensure!(
            [container, name, metadata_ident]
                .iter()
                .all(|value| !value.is_empty()),
            "{parameter}: storage descriptor fields must not be empty"
        );
        // Source identities describe Python's generated descriptor. Runtime
        // identities describe authenticated metadata and may intentionally differ.
        out.push_str(&format!("HyperparamStorage {{parameter:{},container:{},name:{},source_ident:{},metadata_ident:{}}},\n",rust_string(parameter),rust_string(container),rust_string(name),rust_string(source_ident),rust_string(metadata_ident)));
    }
    out.push_str("],\nident_kinds: &[\n");
    for (ident, kind) in ident_kinds {
        out.push_str(&format!(
            "({},{}),\n",
            rust_string(ident),
            rust_string(kind.as_str().context("unit kind must be a string")?)
        ));
    }
    out.push_str("],\n");
    render_codecs(&mut out, document, "_IDENT_CODEC", "ident_codecs")?;
    render_codecs(&mut out, document, "_KIND_CODEC", "kind_codecs")?;
    out.push_str("fraction_denominators: &[\n");
    for (kind, denominator) in dictionary(document, "_FRACTION_DENOMINATOR")? {
        ensure!(
            known_kinds.contains(kind),
            "fraction denominator has unknown kind {kind}"
        );
        let denominator = denominator
            .as_u64()
            .context("fraction denominator must fit u64")?;
        ensure!(denominator > 0, "fraction denominator must be positive");
        out.push_str(&format!("({},{denominator}),\n", rust_string(kind)));
    }
    out.push_str("],\n");
    let blocktime = value(settings, "BLOCKTIME")?
        .as_f64()
        .context("BLOCKTIME must be numeric")?;
    ensure!(
        blocktime.is_finite() && blocktime > 0.0,
        "BLOCKTIME must be finite and positive"
    );
    let rao_per_tao = value(settings, "RAO_PER_TAO")?
        .as_u64()
        .context("RAO_PER_TAO must fit u64")?;
    ensure!(rao_per_tao > 0, "RAO_PER_TAO must be positive");
    let tao_symbol = value(settings, "TAO_SYMBOL")?
        .as_str()
        .context("TAO_SYMBOL must be a string")?;
    ensure!(!tao_symbol.is_empty(), "TAO_SYMBOL must not be empty");
    let fixed128_one = value(document, "FIXED128_ONE")?
        .to_string()
        .parse::<u128>()
        .context("FIXED128_ONE must fit u128")?;
    ensure!(fixed128_one > 0, "FIXED128_ONE must be positive");
    out.push_str(&format!("blocktime:{blocktime:?},rao_per_tao:{rao_per_tao},tao_symbol:{},fixed128_one:{fixed128_one},short_max:{short_max},\n}};\n",rust_string(tao_symbol)));
    Ok(out)
}

pub(super) fn outputs(tables: &Value, settings: &Value) -> Result<Vec<(&'static str, Vec<u8>)>> {
    let document = tables
        .get(SOURCE)
        .context("authenticated hyperparameter module absent")?;
    Ok(vec![(
        "crates/rtao-catalog/src/hyperparams.rs",
        rustfmt(render(document, settings)?.into_bytes())?,
    )])
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn fixtures() -> Result<(Value, Value)> {
        let tables: Value = serde_json::from_str(include_str!(
            "../../vendor/upstream/extracted/named_tables.json"
        ))?;
        let settings = serde_json::from_str(include_str!(
            "../../vendor/upstream/extracted/settings.json"
        ))?;
        Ok((tables["tables"][SOURCE].clone(), settings))
    }

    fn fields<'a>(document: &'a mut Value, name: &str) -> Result<&'a mut Value> {
        let entries = document["values"]["HYPERPARAMS"]["value"]["dict"]
            .as_array_mut()
            .context("fixture HYPERPARAMS missing")?;
        let entry = entries
            .iter_mut()
            .find(|entry| entry[0].as_str() == Some(name))
            .context("fixture hyperparameter missing")?;
        Ok(&mut entry[1]["fields"])
    }

    fn storage_item<'a>(document: &'a mut Value, name: &str) -> Result<&'a mut Value> {
        let entries = document["values"]["STORAGE_ITEMS"]["value"]["dict"]
            .as_array_mut()
            .context("fixture STORAGE_ITEMS missing")?;
        let entry = entries
            .iter_mut()
            .find(|entry| entry[0].as_str() == Some(name))
            .context("fixture storage item missing")?;
        Ok(&mut entry[1])
    }

    // pyt: unit/test_hyperparams_units.py::test_units_gate_passes_on_committed_metadata
    // The native tool returns Result and generated text instead of Python's
    // exit code and informational stdout. Successful rendering validates the
    // complete committed units table before emitting any generated output.
    #[test]
    fn hyperparams_generator_preserves_authenticated_identities_and_exact_u128() -> Result<()> {
        let (document, settings) = fixtures()?;
        let generated = render(&document, &settings)?;
        assert!(generated.contains("source_ident:\"U64F64\",metadata_ident:\"FixedU128\""));
        assert!(generated.contains("RawInteger::Unsigned(184467440737095516160)"));
        assert!(generated.contains("fixed128_one:18446744073709551616"));
        assert!(generated.contains("short_max:38"));
        assert!(generated.contains("rao_per_tao:1000000000"));
        let mut document = document;
        document["values"]["STORAGE_ITEMS"]["value"]["dict"][0][1]["source_value_type_ident"] =
            Value::Null;
        assert!(render(&document, &settings)?.contains("source_ident:\"\",metadata_ident:\"u16\""));
        document["values"]["STORAGE_ITEMS"]["value"]["dict"][0][1]["source_value_type_ident"] =
            json!(false);
        assert!(render(&document, &settings).is_err());
        Ok(())
    }

    #[test]
    fn hyperparams_generator_adds_new_rows_without_a_parameter_allowlist() -> Result<()> {
        let (mut document, settings) = fixtures()?;
        let mut new = document["values"]["HYPERPARAMS"]["value"]["dict"][1].clone();
        new[0] = json!("future_parameter");
        new[1]["fields"]["doc"] = json!("An extracted future parameter.");
        new[1]["fields"]["minimum"] = json!(-7);
        document["values"]["HYPERPARAMS"]["value"]["dict"]
            .as_array_mut()
            .context("fixture HYPERPARAMS missing")?
            .push(new);
        let mut new = document["values"]["STORAGE_ITEMS"]["value"]["dict"][0].clone();
        new[0] = json!("future_parameter");
        document["values"]["STORAGE_ITEMS"]["value"]["dict"]
            .as_array_mut()
            .context("fixture STORAGE_ITEMS missing")?
            .push(new);
        let generated = render(&document, &settings)?;
        assert!(generated.contains("(\"future_parameter\",Hyperparam"));
        assert!(generated.contains("parameter:\"future_parameter\""));
        assert!(generated.contains("RawInteger::Signed(-7)"));
        Ok(())
    }

    #[test]
    fn hyperparams_generator_counts_unicode_codepoints_and_rejects_missing_short() -> Result<()> {
        let (mut document, settings) = fixtures()?;
        fields(&mut document, "rho")?["short"] = json!("é".repeat(38));
        render(&document, &settings)?;
        fields(&mut document, "rho")?["short"] = json!("é".repeat(39));
        assert!(render(&document, &settings).is_err());
        fields(&mut document, "rho")?["short"] = json!("a\u{301}".repeat(19));
        render(&document, &settings)?;
        fields(&mut document, "rho")?["short"] = json!("a\u{301}".repeat(20));
        assert!(render(&document, &settings).is_err());
        fields(&mut document, "rho")?["short"] = json!("");
        assert!(render(&document, &settings).is_err());
        fields(&mut document, "rho")?
            .as_object_mut()
            .context("fixture fields missing")?
            .remove("short");
        assert!(render(&document, &settings).is_err());
        Ok(())
    }

    #[test]
    fn hyperparams_generator_rejects_metadata_unit_mismatch() -> Result<()> {
        let (mut document, settings) = fixtures()?;
        fields(&mut document, "min_childkey_take")?["kind"] = json!("int");
        assert!(render(&document, &settings).is_err());
        let (mut document, settings) = fixtures()?;
        document["values"]["STORAGE_ITEMS"]["value"]["dict"][0][1]["value_type_ident"] =
            json!("TaoBalance");
        assert!(render(&document, &settings).is_err());
        Ok(())
    }

    // pyt: unit/test_hyperparams_units.py::test_units_gate_flags_disagreement
    #[test]
    fn hyperparams_units_gate_reports_exact_tempo_metadata_disagreement() -> Result<()> {
        let (mut document, settings) = fixtures()?;
        storage_item(&mut document, "tempo")?["value_type_ident"] = json!("PerU16");
        // A typed native-tool error replaces Python's status 1 and printed
        // diagnostic while retaining the parameter, both unit kinds and identity.
        assert_eq!(
            render(&document, &settings).map_err(|error| error.to_string()),
            Err(
                "tempo: hand kind 'blocks' disagrees with metadata identity 'PerU16' (kind 'u16')"
                    .into()
            )
        );
        Ok(())
    }

    // pyt: unit/test_hyperparams_units.py::test_units_gate_accepts_agreement
    #[test]
    fn hyperparams_units_gate_accepts_all_three_agreeing_descriptors_together() -> Result<()> {
        let (mut document, settings) = fixtures()?;
        for (name, ident) in [
            ("kappa", "PerU16"),
            ("min_burn", "TaoBalance"),
            ("bonds_moving_avg", "Permill"),
        ] {
            storage_item(&mut document, name)?["value_type_ident"] = json!(ident);
        }
        let generated = render(&document, &settings)?;
        for descriptor in [
            "parameter:\"kappa\",container:\"SubtensorModule\",name:\"Kappa\",source_ident:\"u16\",metadata_ident:\"PerU16\"",
            "parameter:\"min_burn\",container:\"SubtensorModule\",name:\"MinBurn\",source_ident:\"TaoBalance\",metadata_ident:\"TaoBalance\"",
            "parameter:\"bonds_moving_avg\",container:\"SubtensorModule\",name:\"BondsMovingAverage\",source_ident:\"u64\",metadata_ident:\"Permill\"",
        ] {
            assert!(generated.contains(descriptor), "{descriptor}");
        }
        Ok(())
    }

    #[test]
    fn hyperparams_generator_rejects_malformed_bounds_codecs_and_rows() -> Result<()> {
        for (field, invalid) in [
            ("minimum", json!(true)),
            ("maximum", json!(1.0)),
            ("minimum", json!("1")),
            ("bits", json!(0)),
            ("bits", json!(129)),
            ("bits", json!(8.0)),
            ("signed", json!(0)),
            ("kind", json!("unknown")),
            (
                "minimum",
                serde_json::from_str("-170141183460469231731687303715884105729")?,
            ),
            (
                "maximum",
                serde_json::from_str("340282366920938463463374607431768211456")?,
            ),
        ] {
            let (mut document, settings) = fixtures()?;
            fields(&mut document, "rho")?[field] = invalid;
            assert!(render(&document, &settings).is_err(), "{field}");
        }
        let (mut document, settings) = fixtures()?;
        fields(&mut document, "rho")?["minimum"] = json!(2);
        fields(&mut document, "rho")?["maximum"] = json!(1);
        assert!(render(&document, &settings).is_err());
        let (mut document, settings) = fixtures()?;
        document["values"]["_IDENT_CODEC"]["value"]["dict"][0][1] = json!({"tuple":[false,129]});
        assert!(render(&document, &settings).is_err());
        let (mut document, settings) = fixtures()?;
        let duplicate = document["values"]["HYPERPARAMS"]["value"]["dict"][0].clone();
        document["values"]["HYPERPARAMS"]["value"]["dict"]
            .as_array_mut()
            .context("fixture HYPERPARAMS missing")?
            .push(duplicate);
        assert!(render(&document, &settings).is_err());
        let (mut document, settings) = fixtures()?;
        document["values"]["STORAGE_ITEMS"]["value"]["dict"][0][1]["value_type_ident"] =
            Value::Null;
        assert!(render(&document, &settings).is_err());
        Ok(())
    }
}
