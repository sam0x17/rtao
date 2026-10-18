//! Replay hosted hyperparameter results after checking the complete input recipe.

use std::{collections::BTreeSet, fs, path::Path};

use anyhow::{Context, Result, bail, ensure};
use rtao_catalog::hyperparams::TABLE;
use rtao_primitives::hyperparams::{
    Hyperparam, HyperparamStorage, HyperparamTable, ProportionInput, RawInteger,
};
use rustpython_parser::{Parse, ast};
use serde_json::{Value, json};

const SOURCE: &str = "sdk/python/bittensor/hyperparams.py";
const SETTINGS: &str = "sdk/python/bittensor/settings.py";
const UNKNOWN: &str = "__rtao_unknown__";
const METHODS: [&str; 7] = [
    "metadata_kind",
    "kind_of",
    "doc_of",
    "short_of",
    "codec_of",
    "raw_bounds",
    "value_forms",
];

const fn fixture(kind: &'static str, doc: &'static str, short: &'static str) -> Hyperparam {
    Hyperparam {
        kind,
        doc,
        short,
        signed: None,
        bits: None,
        minimum: None,
        maximum: None,
    }
}

const FIXTURE_ENTRIES: [(&str, Hyperparam); 6] = [
    (
        "__rtao_metadata__",
        fixture("int", "oracle metadata fixture", "metadata fixture"),
    ),
    (
        "__rtao_explicit__",
        Hyperparam {
            signed: Some(true),
            bits: Some(8),
            ..fixture("int", "oracle explicit fixture", "explicit fixture")
        },
    ),
    (
        "__rtao_signed__",
        fixture("blocks", "oracle signed fixture", "signed fixture"),
    ),
    (
        "__rtao_signed_alone__",
        Hyperparam {
            signed: Some(true),
            ..fixture("int", "oracle signed-alone fixture", "signed-alone fixture")
        },
    ),
    (
        "__rtao_fallback__",
        fixture("int", "oracle fallback fixture", ""),
    ),
    (
        "__rtao_unknown_ident__",
        fixture(
            "bool",
            "oracle unknown-ident fixture",
            "unknown-ident fixture",
        ),
    ),
];

const fn storage(name: &'static str, ident: &'static str) -> HyperparamStorage {
    HyperparamStorage {
        parameter: name,
        container: "Oracle",
        name,
        source_ident: ident,
        metadata_ident: ident,
    }
}

const FIXTURE_STORAGE: [HyperparamStorage; 6] = [
    storage("__rtao_metadata__", "TaoBalance"),
    storage("__rtao_explicit__", "PerU16"),
    storage("__rtao_signed__", "i16"),
    storage("__rtao_signed_alone__", "u8"),
    storage("__rtao_unknown_ident__", "UnknownOracleIdent"),
    storage("__rtao_no_meta__", "PerU16"),
];

fn precedence_table() -> HyperparamTable {
    HyperparamTable {
        entries: &FIXTURE_ENTRIES,
        storage: &FIXTURE_STORAGE,
        ..TABLE
    }
}

fn names(table: &HyperparamTable) -> Vec<&'static str> {
    table
        .entries
        .iter()
        .map(|(name, _)| *name)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn precedence_names() -> Vec<&'static str> {
    FIXTURE_ENTRIES
        .iter()
        .map(|(name, _)| *name)
        .chain(FIXTURE_STORAGE.iter().map(|item| item.parameter))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

struct Recipe {
    integers: Vec<String>,
    floats: Vec<u64>,
    ties: Vec<u64>,
    texts: Vec<String>,
}

fn literal_list(suite: &ast::Suite, name: &str) -> Result<Vec<Value>> {
    let expression = suite
        .iter()
        .find_map(|statement| {
            match statement {
            ast::Stmt::Assign(assign)
                if assign.targets.iter().any(|target| {
                    matches!(target, ast::Expr::Name(target) if target.id.as_str() == name)
                }) =>
            {
                Some(assign.value.as_ref())
            }
            _ => None,
        }
        })
        .with_context(|| format!("missing literal hyperparameter recipe {name}"))?;
    let ast::Expr::List(list) = expression else {
        bail!("hyperparameter recipe {name} must be a literal list")
    };
    list.elts
        .iter()
        .map(|item| {
            let ast::Expr::Constant(item) = item else {
                bail!("hyperparameter recipe {name} must contain only literals")
            };
            match &item.value {
                ast::Constant::Str(value) => Ok(json!(value)),
                ast::Constant::Int(value) => Ok(serde_json::from_str(&value.to_string())?),
                _ => bail!("unsupported hyperparameter recipe literal in {name}"),
            }
        })
        .collect()
}

impl Recipe {
    fn read(root: &Path) -> Result<Self> {
        let source = fs::read_to_string(root.join("ci/oracle/dump_pure.py"))?;
        let suite = ast::Suite::parse(&source, "ci/oracle/dump_pure.py")?;
        let strings = |name| {
            literal_list(&suite, name)?
                .into_iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .with_context(|| format!("{name} needs strings"))
                })
                .collect::<Result<Vec<_>>>()
        };
        let integers = strings("HYPER_INT_INPUTS")?;
        let texts = strings("HYPER_TEXT_INPUTS")?;
        let unsigned = |name| {
            literal_list(&suite, name)?
                .iter()
                .map(|value| value.as_u64().with_context(|| format!("{name} needs u64")))
                .collect::<Result<Vec<_>>>()
        };
        let floats = unsigned("HYPER_FLOAT_BITS")?;
        let ties = unsigned("HYPER_TIE_NUMERATORS")?;
        ensure!(
            integers.len() >= 32
                && texts.len() >= 34
                && floats.len() >= 22
                && ties == [0, 1, 2, 3, 127, 128, 32766, 32767, 499999],
            "hyperparameter input recipe lost required branch coverage"
        );
        for token in [
            "-1",
            "0",
            "1",
            "9007199254740993",
            "18446744073709551615",
            "18446744073709551616",
            "-170141183460469231731687303715884105728",
            "340282366920938463463374607431768211455",
            "340282366920938463463374607431768211456",
        ] {
            ensure!(
                integers.iter().any(|value| value == token),
                "hyperparameter recipe omits integer anchor {token}"
            );
        }
        for bits in [
            0,
            (-0.0_f64).to_bits(),
            1,
            0.5_f64.to_bits(),
            1.0_f64.to_bits(),
            f64::INFINITY.to_bits(),
            f64::NEG_INFINITY.to_bits(),
            f64::NAN.to_bits(),
        ] {
            ensure!(
                floats.contains(&bits),
                "hyperparameter recipe omits binary64 anchor {bits}"
            );
        }
        for token in [
            "",
            "1.0",
            "1e10000",
            "true",
            "false",
            " +TRUE ",
            "nan",
            "1__0",
            "\u{2003}+1\u{3000}",
            " +１_２ ",
            "١.٥",
        ] {
            ensure!(
                texts.iter().any(|value| value == token),
                "hyperparameter recipe omits textual anchor {token:?}"
            );
        }
        ensure!(
            integers.iter().collect::<BTreeSet<_>>().len() == integers.len()
                && floats.iter().collect::<BTreeSet<_>>().len() == floats.len()
                && texts.iter().collect::<BTreeSet<_>>().len() == texts.len(),
            "hyperparameter recipe contains duplicated anchors"
        );
        Ok(Self {
            integers,
            floats,
            ties,
            texts,
        })
    }

    fn inputs(&self, table: &HyperparamTable, name: &str) -> Result<Vec<Value>> {
        let mut result = Vec::new();
        let integer = |id: String, token: &str| -> Result<Value> {
            let value: Value = serde_json::from_str(token)?;
            ensure!(
                value.is_number() && !token.contains(['.', 'e', 'E']),
                "raw hyperparameter input must remain an integer token"
            );
            Ok(json!({"id":id,"input":{"type":"int","value":value}}))
        };
        let floating = |id: String, bits: u64| json!({"id":id,"input":{"type":"f64","bits":bits}});
        for (index, token) in self.integers.iter().enumerate() {
            result.push(integer(format!("integer/anchor/{index}"), token)?);
        }
        let (lo, hi) = table.raw_bounds(name);
        for (label, boundary) in [("minimum", lo), ("maximum", hi)] {
            let boundary = rtao_primitives::pyfmt::parse_integer(&boundary.to_string())
                .context("native hyperparameter boundary is not an integer")?;
            for offset in [-1_i32, 0, 1] {
                result.push(integer(
                    format!("integer/{label}/{offset}"),
                    &(&boundary + offset).to_string(),
                )?);
            }
        }
        for value in [false, true] {
            result.push(json!({"id":format!("bool/{}",u8::from(value)),"input":{"type":"bool","value":value}}));
        }
        for (index, bits) in self.floats.iter().enumerate() {
            result.push(floating(format!("float/anchor/{index}"), *bits));
        }
        let kind = table.kind_of(name);
        let scale = table
            .fraction_denominators
            .iter()
            .find(|(key, _)| *key == kind)
            .map(|(_, value)| *value as f64)
            .unwrap_or_else(|| match kind {
                "rao" => table.rao_per_tao as f64,
                "fixed128" => table.fixed128_one as f64,
                _ => 1.0,
            });
        for numerator in &self.ties {
            let middle = (*numerator as f64 + 0.5) / scale;
            for (label, value) in [
                ("below", nextafter(middle, false)),
                ("tie", middle),
                ("above", nextafter(middle, true)),
            ] {
                result.push(floating(
                    format!("float/tie/{numerator}/{label}"),
                    value.to_bits(),
                ));
            }
        }
        for (label, boundary) in [("minimum", lo), ("maximum", hi)] {
            let middle = raw_float(boundary) / scale;
            for (side, value) in [
                ("below", nextafter(middle, false)),
                ("at", middle),
                ("above", nextafter(middle, true)),
            ] {
                result.push(floating(format!("float/{label}/{side}"), value.to_bits()));
            }
        }
        for (index, value) in self.texts.iter().enumerate() {
            result
                .push(json!({"id":format!("text/{index}"),"input":{"type":"text","value":value}}));
        }
        Ok(result)
    }
}

fn raw_float(value: RawInteger) -> f64 {
    match value {
        RawInteger::Unsigned(value) => value as f64,
        RawInteger::Signed(value) => value as f64,
    }
}

// Rust 1.89 has no next_up/next_down. The recipe's neighbors only target
// finite values, including zero; this is input generation, not SDK arithmetic.
fn nextafter(value: f64, positive: bool) -> f64 {
    if value == 0.0 {
        return f64::from_bits(if positive { 1 } else { (1_u64 << 63) | 1 });
    }
    let bits = value.to_bits();
    f64::from_bits(if value.is_sign_positive() == positive {
        bits + 1
    } else {
        bits - 1
    })
}

fn keys(value: &Value, expected: &[&str], description: &str) -> Result<()> {
    let object = value
        .as_object()
        .with_context(|| format!("hyperparameter {description} must be an object"))?;
    ensure!(
        object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key)),
        "hyperparameter {description} fields differ"
    );
    Ok(())
}

fn decode(input: &Value) -> Result<ProportionInput<'_>> {
    let kind = super::text(input, "type")?;
    keys(
        input,
        if kind == "f64" {
            &["type", "bits"]
        } else {
            &["type", "value"]
        },
        "input",
    )?;
    Ok(match kind {
        "int" => {
            let value = &input["value"];
            let token = value.to_string();
            ensure!(
                value.is_number() && !token.contains(['.', 'e', 'E']),
                "hyperparameter raw input is not an integer token"
            );
            ProportionInput::Raw(
                rtao_primitives::pyfmt::parse_integer(&token).context("invalid raw integer")?,
            )
        }
        "f64" => ProportionInput::Fraction(f64::from_bits(
            input["bits"].as_u64().context("invalid binary64 bits")?,
        )),
        "bool" => {
            ProportionInput::Boolean(input["value"].as_bool().context("invalid boolean input")?)
        }
        "text" => super::text(input, "value")?.into(),
        _ => bail!("unknown hyperparameter input type {kind}"),
    })
}

fn metadata_results(table: &HyperparamTable, name: &str) -> Result<Value> {
    let (signed, bits) = table.codec_of(name);
    let (lo, hi) = table.raw_bounds(name);
    Ok(json!({
        "metadata_kind":{"ok":table.metadata_kind(name)},
        "kind_of":{"ok":table.kind_of(name)},
        "doc_of":{"ok":table.doc_of(name)},
        "short_of":{"ok":table.short_of(name)},
        "codec_of":{"ok":[signed,bits]},
        "raw_bounds":{"ok":[serde_json::from_str::<Value>(&lo.to_string())?,serde_json::from_str::<Value>(&hi.to_string())?]},
        "value_forms":{"ok":table.value_forms(name)},
    }))
}

fn numeric_result(value: Option<f64>) -> Value {
    json!({"ok":value.map(|value|json!({"f64_bits":value.to_bits()}))})
}

fn raw_result(table: &HyperparamTable, name: &str, input: ProportionInput<'_>) -> Result<Value> {
    Ok(match table.to_raw(name, input) {
        Ok(raw) => json!({"ok":serde_json::from_str::<Value>(&raw.to_string())?}),
        Err(error) => json!({"error":{"type":"ValueError","message":error.to_string()}}),
    })
}

fn compare(expected: &Value, actual: &Value, name: &str, method: &str, case: &str) -> Result<()> {
    rtao_primitives::json::compare(expected, actual).with_context(|| {
        format!(
            "Python hyperparameter {name}/{method}/{case} differs; {SOURCE}; expected {expected}, actual {actual}"
        )
    })
}

fn check_inputs(rows: &[Value], expected: &[Value], name: &str) -> Result<()> {
    ensure!(
        rows.len() == expected.len(),
        "hyperparameter {name} input inventory is incomplete"
    );
    let mut seen = BTreeSet::new();
    for (row, planned) in rows.iter().zip(expected) {
        let id = super::text(row, "id")?;
        ensure!(
            seen.insert(id),
            "duplicate hyperparameter input {name}/{id}"
        );
        ensure!(
            row["id"] == planned["id"],
            "hyperparameter {name} input ID/order differs"
        );
        compare(&planned["input"], &row["input"], name, "input", id)?;
        decode(&row["input"])?;
    }
    Ok(())
}

fn check_rows(
    table: &HyperparamTable,
    names: &[&str],
    rows: &Value,
    recipe: &Recipe,
) -> Result<()> {
    let rows = rows.as_array().context("hyperparameter rows missing")?;
    ensure!(
        rows.len() == names.len(),
        "hyperparameter table coverage differs"
    );
    for (row, name) in rows.iter().zip(names) {
        keys(row, &["name", "methods", "inputs"], "row")?;
        ensure!(row["name"] == *name, "hyperparameter name/order differs");
        keys(&row["methods"], &METHODS, "method results")?;
        let expected = metadata_results(table, name)?;
        for method in METHODS {
            compare(
                &row["methods"][method],
                &expected[method],
                name,
                method,
                "table",
            )?;
        }
        let inputs = row["inputs"]
            .as_array()
            .context("hyperparameter inputs missing")?;
        check_inputs(inputs, &recipe.inputs(table, name)?, name)?;
        for input in inputs {
            let id = super::text(input, "id")?;
            let numeric = input["input"]["type"] != "text";
            keys(
                input,
                if numeric {
                    &["id", "input", "normalized", "annotate", "to_raw"]
                } else {
                    &["id", "input", "annotate", "to_raw"]
                },
                "input result",
            )?;
            let decoded = decode(&input["input"])?;
            if numeric {
                compare(
                    &input["normalized"],
                    &numeric_result(table.normalized(name, decoded.clone())),
                    name,
                    "normalized",
                    id,
                )?;
            }
            compare(
                &input["annotate"],
                &json!({"ok":table.annotate(name, decoded.clone())}),
                name,
                "annotate",
                id,
            )?;
            compare(
                &input["to_raw"],
                &raw_result(table, name, decoded)?,
                name,
                "to_raw",
                id,
            )?;
        }
    }
    Ok(())
}

pub(super) fn check(root: &Path, document: &Value) -> Result<()> {
    keys(
        document,
        &[
            "schema",
            "domain",
            "recipe",
            "source",
            "source_sha256",
            "settings_sha256",
            "table_names",
            "storage_idents",
            "vectors",
            "precedence",
        ],
        "document",
    )?;
    ensure!(
        document["schema"] == 1
            && document["recipe"] == 1
            && document["domain"] == "native-codec"
            && document["source"] == SOURCE,
        "hyperparameter oracle schema/domain/source differs"
    );
    for (field, source) in [("source_sha256", SOURCE), ("settings_sha256", SETTINGS)] {
        ensure!(
            document[field]
                == super::sha(&fs::read(root.join("vendor/upstream/src").join(source))?),
            "hyperparameter oracle source hash differs: {source}"
        );
    }
    let mut table_names = names(&TABLE);
    compare(
        &json!(table_names),
        &document["table_names"],
        "TABLE",
        "names",
        "table",
    )?;
    let mut items = TABLE.storage.to_vec();
    items.sort_by_key(|item| item.parameter);
    let idents: Vec<_> = items
        .iter()
        .map(|item| json!({"parameter":item.parameter,"ident":item.source_ident}))
        .collect();
    compare(
        &json!(idents),
        &document["storage_idents"],
        "TABLE",
        "storage",
        "table",
    )?;
    let recipe = Recipe::read(root)?;
    table_names.push(UNKNOWN);
    check_rows(&TABLE, &table_names, &document["vectors"], &recipe)?;
    check_rows(
        &precedence_table(),
        &precedence_names(),
        &document["precedence"],
        &recipe,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests construct input plans and small native assertions only.
    // They never manufacture a Python output corpus or clear oracle pending.
    #[test]
    fn complete_recipe_parses_and_plans_without_python() -> Result<()> {
        let recipe = Recipe::read(super::super::root()?)?;
        assert_eq!(names(&TABLE).len(), 51);
        assert_eq!(TABLE.storage.len(), 45);
        assert_eq!(precedence_names().len(), 7);
        let table = precedence_table();
        let mut catalog = names(&TABLE);
        catalog.push(UNKNOWN);
        for (table, names) in [(&TABLE, catalog), (&table, precedence_names())] {
            for name in names {
                let inputs = recipe.inputs(table, name)?;
                assert_eq!(inputs.len(), 129, "{name}");
                check_inputs(&inputs, &inputs, name)?;
                metadata_results(table, name)?;
                for case in &inputs {
                    let input = decode(&case["input"])?;
                    if case["input"]["type"] != "text" {
                        numeric_result(table.normalized(name, input.clone()));
                    }
                    table.annotate(name, input.clone());
                    raw_result(table, name, input)?;
                }
                assert_eq!(
                    inputs
                        .iter()
                        .filter(|row| row["input"]["type"] == "bool")
                        .count(),
                    2
                );
                assert_eq!(
                    inputs
                        .iter()
                        .filter(|row| row["id"]
                            .as_str()
                            .is_some_and(|id| id.starts_with("float/tie/")))
                        .count(),
                    27
                );
            }
        }
        Ok(())
    }

    #[test]
    fn input_inventory_rejects_missing_duplicated_changed_or_retyped_cases() -> Result<()> {
        let recipe = Recipe::read(super::super::root()?)?;
        let expected = recipe.inputs(&TABLE, "kappa")?;
        let mut changed = expected.clone();
        changed.pop();
        assert!(check_inputs(&changed, &expected, "kappa").is_err());
        let mut changed = expected.clone();
        changed[1] = changed[0].clone();
        assert!(check_inputs(&changed, &expected, "kappa").is_err());
        let mut changed = expected.clone();
        changed[1]["input"] = json!({"type":"f64","bits":0});
        assert!(check_inputs(&changed, &expected, "kappa").is_err());
        let mut changed = expected.clone();
        changed[0]["input"]["value"] = json!(-2);
        assert!(check_inputs(&changed, &expected, "kappa").is_err());
        let mut changed = expected.clone();
        changed.swap(0, 1);
        assert!(check_inputs(&changed, &expected, "kappa").is_err());
        Ok(())
    }

    #[test]
    fn native_bounds_keep_oversized_diagnostic_inputs_exact() -> Result<()> {
        let recipe = Recipe::read(super::super::root()?)?;
        let cases = recipe.inputs(&TABLE, "collateral_drain_ratio")?;
        let above = cases
            .iter()
            .find(|row| row["id"] == "integer/anchor/31")
            .context("u128 upper neighbor")?;
        assert_eq!(
            above["input"]["value"].to_string(),
            "340282366920938463463374607431768211456"
        );
        assert_eq!(
            decode(&above["input"])?,
            ProportionInput::Raw(
                rtao_primitives::pyfmt::parse_integer("340282366920938463463374607431768211456")
                    .context("integer")?
            )
        );
        assert!(
            TABLE
                .to_raw("collateral_drain_ratio", decode(&above["input"])?)
                .is_err()
        );
        let cases = recipe.inputs(&TABLE, "min_burn")?;
        let above = cases
            .iter()
            .find(|row| row["id"] == "integer/maximum/1")
            .context("TAO upper neighbor")?;
        assert_eq!(above["input"]["value"].to_string(), "18446744073709551616");
        assert!(TABLE.to_raw("min_burn", decode(&above["input"])?).is_err());
        Ok(())
    }

    #[test]
    fn binary64_neighbors_and_result_tokens_preserve_bits_and_types() -> Result<()> {
        for value in [0.0, -0.0, 0.5, 1.0, -1.0, 18446744073709551616.0] {
            let below = nextafter(value, false);
            let above = nextafter(value, true);
            assert!(below < value && value < above);
            assert_eq!(nextafter(above, false), value);
        }
        assert_eq!(
            numeric_result(Some(-0.0)),
            json!({"ok":{"f64_bits":1_u64<<63}})
        );
        assert_eq!(
            numeric_result(Some(f64::NAN)),
            json!({"ok":{"f64_bits":f64::NAN.to_bits()}})
        );
        assert_eq!(numeric_result(None), json!({"ok":null}));
        assert!(
            compare(
                &json!({"ok":1}),
                &json!({"ok":1.0}),
                "test",
                "to_raw",
                "typed"
            )
            .is_err()
        );
        assert!(
            compare(
                &json!({"error":{"type":"ValueError","message":"expected"}}),
                &json!({"error":{"type":"ValueError","message":"changed"}}),
                "test",
                "to_raw",
                "error"
            )
            .is_err()
        );
        assert!(decode(&json!({"type":"int","value":1.0})).is_err());
        assert!(decode(&json!({"type":"bool","value":1})).is_err());
        assert!(decode(&json!({"type":"f64","bits":0,"unexpected":true})).is_err());
        Ok(())
    }

    #[test]
    fn precedence_fixtures_force_metadata_explicit_signed_and_fallback_branches() {
        let table = precedence_table();
        assert_eq!(table.kind_of("__rtao_metadata__"), "rao");
        assert_eq!(table.codec_of("__rtao_metadata__"), (false, 64));
        assert_eq!(table.kind_of("__rtao_explicit__"), "u16");
        assert_eq!(table.codec_of("__rtao_explicit__"), (true, 8));
        assert_eq!(
            table.raw_bounds("__rtao_explicit__"),
            (0.into(), 127.into())
        );
        assert_eq!(table.codec_of("__rtao_signed__"), (true, 16));
        assert_eq!(
            table.raw_bounds("__rtao_signed__"),
            ((-32768).into(), 32767.into())
        );
        assert_eq!(table.codec_of("__rtao_signed_alone__"), (false, 8));
        assert_eq!(table.codec_of("__rtao_unknown_ident__"), (false, 1));
        assert_eq!(table.codec_of("__rtao_fallback__"), (false, 64));
        assert_eq!(table.short_of("__rtao_fallback__"), None);
        assert_eq!(table.kind_of("__rtao_no_meta__"), "u16");
        assert_eq!(table.doc_of("__rtao_no_meta__"), None);
    }
}
