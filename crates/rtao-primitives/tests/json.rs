//! Arbitrary-precision input regressions and oracle numerical comparisons.

use std::error::Error;

use rtao_core::codec::{Value as CoreValue, value::to_corpus_json};
use rtao_primitives::json::{self, FloatTolerance};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize, PartialEq)]
#[serde(untagged)]
enum Untagged {
    Float(f64),
    Text(String),
}

#[derive(Debug, Deserialize, PartialEq)]
struct Measurements {
    ratio: f64,
}

#[derive(Debug, Deserialize, PartialEq)]
struct Flattened {
    name: String,
    #[serde(flatten)]
    measurements: Measurements,
}

#[test]
fn value_first_fixes_untagged_float_text_deserialization() -> Result<(), Box<dyn Error>> {
    for text in ["0.5", "1e-05", "-0.25"] {
        assert!(serde_json::from_str::<Untagged>(text).is_err());
        assert!(serde_json::from_slice::<Untagged>(text.as_bytes()).is_err());
        let expected = Untagged::Float(text.parse()?);
        assert_eq!(json::from_str::<Untagged>(text)?, expected);
        assert_eq!(json::from_slice::<Untagged>(text.as_bytes())?, expected);
    }
    let spelling = serde_json::from_str::<Value>("1e-05")?;
    assert!(serde_json::from_value::<Untagged>(spelling).is_err());
    assert_eq!(
        json::from_str::<Untagged>("\"named\"")?,
        Untagged::Text("named".into())
    );
    Ok(())
}

#[test]
fn value_first_fixes_flattened_float_text_deserialization() -> Result<(), Box<dyn Error>> {
    for token in ["0.05", "1e-05", "0.05000", "-0.2500"] {
        let text = format!(r#"{{"name":"validator","ratio":{token}}}"#);
        assert!(serde_json::from_str::<Flattened>(&text).is_err());
        assert!(serde_json::from_slice::<Flattened>(text.as_bytes()).is_err());
        let expected = Flattened {
            name: "validator".into(),
            measurements: Measurements {
                ratio: token.parse()?,
            },
        };
        assert_eq!(json::from_str::<Flattened>(&text)?, expected);
        assert_eq!(json::from_slice::<Flattened>(text.as_bytes())?, expected);
    }
    Ok(())
}

#[derive(Debug, Deserialize, PartialEq)]
enum Nested {
    Newtype(Option<Vec<Untagged>>),
    Tuple(Untagged, UtaggedOption),
    Record { value: Flattened },
}

#[derive(Debug, Deserialize, PartialEq)]
struct UtaggedOption(Option<Untagged>);

#[derive(Debug, PartialEq)]
enum AnyNumeric {
    Integer(u64),
    Float(f64),
}

impl<'de> Deserialize<'de> for AnyNumeric {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NumericVisitor;

        impl<'de> serde::de::Visitor<'de> for NumericVisitor {
            type Value = AnyNumeric;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an integer or float")
            }

            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(AnyNumeric::Integer(value))
            }

            fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Self::Value, E> {
                Ok(AnyNumeric::Float(value))
            }
        }

        deserializer.deserialize_any(NumericVisitor)
    }
}

#[test]
fn any_visitors_still_receive_small_integers_and_canonical_floats() -> Result<(), Box<dyn Error>> {
    assert_eq!(json::from_str::<AnyNumeric>("42")?, AnyNumeric::Integer(42));
    assert_eq!(json::from_str::<AnyNumeric>("0.5")?, AnyNumeric::Float(0.5));
    assert_eq!(
        json::from_slice::<AnyNumeric>(b"42")?,
        AnyNumeric::Integer(42)
    );
    assert_eq!(
        json::from_slice::<AnyNumeric>(b"0.5")?,
        AnyNumeric::Float(0.5)
    );
    Ok(())
}

#[test]
fn float_adaptation_reaches_nested_sequences_options_newtypes_and_enums()
-> Result<(), Box<dyn Error>> {
    for (text, expected) in [
        (
            r#"{"Newtype":[1e-05,0.05000]}"#,
            Nested::Newtype(Some(vec![Untagged::Float(1e-5), Untagged::Float(0.05)])),
        ),
        (
            r#"{"Tuple":[1e-05,0.05000]}"#,
            Nested::Tuple(
                Untagged::Float(1e-5),
                UtaggedOption(Some(Untagged::Float(0.05))),
            ),
        ),
        (
            r#"{"Record":{"value":{"name":"validator","ratio":1e-05}}}"#,
            Nested::Record {
                value: Flattened {
                    name: "validator".into(),
                    measurements: Measurements { ratio: 1e-5 },
                },
            },
        ),
    ] {
        assert_eq!(json::from_str::<Nested>(text)?, expected);
        assert_eq!(json::from_slice::<Nested>(text.as_bytes())?, expected);
    }
    let ordinary: std::collections::BTreeMap<u64, Untagged> = json::from_str(r#"{"7":1e-05}"#)?;
    assert_eq!(ordinary.get(&7), Some(&Untagged::Float(1e-5)));
    let exact: std::collections::BTreeMap<u128, Value> = json::from_str(&format!(
        r#"{{"{}":340282366920938463463374607431768211456}}"#,
        u128::MAX,
    ))?;
    assert_eq!(
        exact
            .get(&u128::MAX)
            .ok_or("wide map key absent")?
            .as_number()
            .ok_or("wide number became nonnumeric")?
            .as_str(),
        "340282366920938463463374607431768211456"
    );
    Ok(())
}

#[test]
#[cfg(feature = "serde")]
fn float_adaptation_leaves_value_and_money_tokens_exact() -> Result<(), Box<dyn Error>> {
    #[derive(Deserialize)]
    struct Exact {
        token: Value,
        money: rtao_primitives::Money,
        amount: rtao_primitives::Amount,
    }
    let digits = "18446744073.709551615";
    let text = format!(r#"{{"token":{digits},"money":{digits},"amount":{digits}}}"#);
    let exact: Exact = json::from_str(&text)?;
    assert_eq!(
        exact
            .token
            .as_number()
            .ok_or("number became nonnumeric")?
            .as_str(),
        digits
    );
    assert_eq!(exact.money.rao()?, u64::MAX);
    assert_eq!(
        exact.amount,
        rtao_primitives::Amount::Exact(rtao_primitives::Money::from_rao(u64::MAX))
    );
    let exact: Exact = json::from_slice(text.as_bytes())?;
    assert_eq!(
        exact
            .token
            .as_number()
            .ok_or("number became nonnumeric")?
            .as_str(),
        digits
    );
    assert_eq!(exact.money.rao()?, u64::MAX);
    for token in [
        "1e-05",
        "0.05000",
        "9007199254740993",
        "340282366920938463463374607431768211456",
        "10000000000000000000000000000000000000000",
    ] {
        let value: Value = json::from_str(token)?;
        assert_eq!(
            value
                .as_number()
                .ok_or("number became nonnumeric")?
                .as_str(),
            token
        );
    }
    #[derive(Deserialize)]
    struct Integers {
        positive: u128,
        negative: i128,
        values: Vec<Value>,
    }
    let huge = "10000000000000000000000000000000000000000";
    let integers: Integers = json::from_str(&format!(
        r#"{{"positive":{},"negative":{},"values":[{huge}]}}"#,
        u128::MAX,
        i128::MIN,
    ))?;
    assert_eq!(integers.positive, u128::MAX);
    assert_eq!(integers.negative, i128::MIN);
    assert_eq!(
        integers
            .values
            .first()
            .ok_or("nested integer absent")?
            .as_number()
            .ok_or("nested integer became nonnumeric")?
            .as_str(),
        huge
    );
    #[derive(Deserialize)]
    struct FlattenedValues {
        #[serde(flatten)]
        values: std::collections::BTreeMap<String, Value>,
    }
    let values: FlattenedValues =
        json::from_str(&format!(r#"{{"u128":{},"u256":{huge}}}"#, u128::MAX))?;
    assert_eq!(
        values
            .values
            .get("u128")
            .ok_or("u128 absent")?
            .as_number()
            .ok_or("u128 became nonnumeric")?
            .as_str(),
        u128::MAX.to_string()
    );
    assert_eq!(
        values
            .values
            .get("u256")
            .ok_or("u256 absent")?
            .as_number()
            .ok_or("u256 became nonnumeric")?
            .as_str(),
        huge
    );
    Ok(())
}

#[test]
fn input_helpers_reject_malformed_trailing_and_invalid_utf8_input() {
    for text in ["", "{", "1 2", "[0,]", "01"] {
        assert!(json::from_str::<Value>(text).is_err());
        assert!(json::from_slice::<Value>(text.as_bytes()).is_err());
    }
    assert!(json::from_slice::<Value>(&[b'"', 0xff, b'"']).is_err());
    assert!(json::from_str::<f64>("1e400").is_err());
    assert!(json::from_str::<f32>("1e40").is_err());
    assert!(json::from_str::<Untagged>("1e400").is_err());
    assert!(json::from_str::<(u8,)>("[1,2]").is_err());
    assert!(json::from_str::<Measurements>(r#"{"ratio":"invalid"}"#).is_err());
}

#[test]
fn corpus_json_u128_i128_and_u256_round_trip_without_digit_loss() -> Result<(), Box<dyn Error>> {
    let vectors = [
        (CoreValue::Uint(u128::MAX), u128::MAX.to_string()),
        (CoreValue::Int(i128::MIN), i128::MIN.to_string()),
        (
            CoreValue::U256([0xff; 32]),
            "115792089237316195423570985008687907853269984665640564039457584007913129639935".into(),
        ),
        (
            CoreValue::U256([
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0,
            ]),
            "340282366920938463463374607431768211456".into(),
        ),
    ];
    for (core, digits) in vectors {
        let corpus = to_corpus_json(&core);
        let text = serde_json::to_string(&corpus)?;
        assert_eq!(text, digits);
        for reparsed in [
            json::from_str::<Value>(&text)?,
            json::from_slice::<Value>(text.as_bytes())?,
        ] {
            let number = reparsed
                .as_number()
                .ok_or("corpus number became non-numeric")?;
            assert_eq!(number.as_str(), digits);
            json::compare(&corpus, &reparsed)?;
        }
    }
    assert_eq!(json::from_str::<u128>(&u128::MAX.to_string())?, u128::MAX);
    assert_eq!(json::from_str::<i128>(&i128::MIN.to_string())?, i128::MIN);
    Ok(())
}

#[test]
fn comparator_keeps_neighboring_large_integers_distinct() -> Result<(), Box<dyn Error>> {
    let vectors = [
        ("11614906923326217", "11614906923326216"),
        ("9007199254740993", "9007199254740992"),
        ("4611686018427387903", "4611686018427387902"),
        (
            "340282366920938463463374607431768211455",
            "340282366920938463463374607431768211454",
        ),
        (
            "340282366920938463463374607431768211456",
            "340282366920938463463374607431768211457",
        ),
        (
            "-340282366920938463463374607431768211456",
            "-340282366920938463463374607431768211457",
        ),
        (
            "-170141183460469231731687303715884105728",
            "-170141183460469231731687303715884105727",
        ),
    ];
    let permissive = FloatTolerance {
        absolute: 1e100,
        relative: 1e100,
    };
    // These vectors are a contract, rather than convenient small integers.
    assert_eq!(11_614_906_923_326_217_u64.to_string(), vectors[0].0);
    assert_eq!(((1_u64 << 53) + 1).to_string(), vectors[1].0);
    assert_eq!((u64::MAX / 4).to_string(), vectors[2].0);
    for (left, right) in vectors {
        let left = json::from_str::<Value>(left)?;
        let right = json::from_str::<Value>(right)?;
        json::compare(&left, &left)?;
        assert!(json::compare(&left, &right).is_err());
        assert!(json::compare_with_tolerance(&left, &right, permissive).is_err());
    }
    json::compare(
        &json::from_str::<Value>("-0")?,
        &json::from_str::<Value>("0")?,
    )?;
    Ok(())
}

#[test]
fn comparator_matches_float_values_across_lexical_spellings() -> Result<(), Box<dyn Error>> {
    for (left, right) in [
        ("1e-05", "1e-5"),
        ("0.0000100", "1e-5"),
        ("1.0", "1e0"),
        ("-0.0", "0e0"),
        ("1.2345678901234567", "1.23456789012345670"),
    ] {
        json::compare(
            &json::from_str::<Value>(left)?,
            &json::from_str::<Value>(right)?,
        )?;
    }
    let left = json::from_str::<Value>("1e-05")?;
    let right = json::from_str::<Value>("1e-5")?;
    assert_ne!(
        left, right,
        "regression must exercise spelling-sensitive Value equality"
    );
    assert!(
        json::compare(
            &json::from_str::<Value>("1.0000000000000000")?,
            &json::from_str::<Value>("1.0000000000000002")?,
        )
        .is_err()
    );
    for (left, right) in [
        ("1", "1.0"),
        ("0", "0e0"),
        ("9007199254740993", "9007199254740993.0"),
    ] {
        assert!(
            json::compare(
                &json::from_str::<Value>(left)?,
                &json::from_str::<Value>(right)?
            )
            .is_err()
        );
    }
    assert!(
        json::compare(
            &json::from_str::<Value>("1e400")?,
            &json::from_str::<Value>("1e400")?
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn tolerance_applies_only_to_pairs_of_float_tokens() -> Result<(), Box<dyn Error>> {
    let tolerance = FloatTolerance {
        absolute: 1e-6,
        relative: 1e-6,
    };
    let parse = json::from_str::<Value>;
    assert!(json::compare(&parse("0.000001")?, &parse("0.0000015")?).is_err());
    json::compare_with_tolerance(&parse("0.000001")?, &parse("0.0000015")?, tolerance)?;
    json::compare_with_tolerance(&parse("1000.0")?, &parse("1000.0005")?, tolerance)?;
    assert!(json::compare_with_tolerance(&parse("1.0")?, &parse("1.01")?, tolerance).is_err());
    assert!(json::compare_with_tolerance(&parse("1")?, &parse("1.0000001")?, tolerance).is_err());
    let huge = FloatTolerance {
        absolute: 1e100,
        relative: 1e100,
    };
    assert!(json::compare_with_tolerance(&parse("1")?, &parse("2")?, huge).is_err());
    assert!(json::compare_with_tolerance(&parse("1")?, &parse("1.0")?, huge).is_err());
    let relative = FloatTolerance {
        absolute: 0.0,
        relative: 1.0,
    };
    assert!(
        json::compare_with_tolerance(&parse("1.7e308")?, &parse("-1.7e308")?, relative).is_err()
    );
    for invalid in [
        FloatTolerance {
            absolute: -1.0,
            relative: 0.0,
        },
        FloatTolerance {
            absolute: f64::INFINITY,
            relative: 0.0,
        },
        FloatTolerance {
            absolute: 0.0,
            relative: f64::NAN,
        },
    ] {
        assert!(json::compare_with_tolerance(&parse("0.5")?, &parse("0.5")?, invalid).is_err());
    }
    Ok(())
}

#[test]
fn comparator_walks_trees_and_reports_json_pointer_differences() -> Result<(), Box<dyn Error>> {
    let parse = json::from_str::<Value>;
    json::compare(
        &parse(r#"{"b":[true,null,"exact"],"a":1e-05}"#)?,
        &parse(r#"{"a":1e-5,"b":[true,null,"exact"]}"#)?,
    )?;
    let error = json::compare(
        &parse(r#"{"a/b~c":[11614906923326217]}"#)?,
        &parse(r#"{"a/b~c":[11614906923326216]}"#)?,
    )
    .err()
    .ok_or("different nested integers matched")?;
    assert_eq!(error.path, "/a~1b~0c/0");
    assert_eq!(error.reason, "integer values differ");
    assert!(error.to_string().contains("/a~1b~0c/0"));
    for (left, right, path) in [
        (r#"{"x":null}"#, "{}", "/x"),
        ("{}", r#"{"x":null}"#, "/x"),
        ("[null]", "[]", "/0"),
        ("[]", "[null]", "/0"),
        ("[true]", "[false]", "/0"),
        ("[\"a\"]", "[\"b\"]", "/0"),
        ("null", "false", ""),
    ] {
        let error = json::compare(&parse(left)?, &parse(right)?)
            .err()
            .ok_or("different trees matched")?;
        assert_eq!(error.path, path);
    }
    Ok(())
}
