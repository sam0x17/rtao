// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_hyperparams_units.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Hyperparameter conversion, diagnostics and human rendering.

use rtao_catalog::hyperparams::TABLE;
use rtao_primitives::hyperparams::{HyperparamError, ProportionInput};

// pyt: unit/test_hyperparams_units.py::test_to_raw_integer_is_raw_within_bounds
// pyt: unit/test_hyperparams_units.py::test_to_raw_rejects_outside_codec_width
#[test]
fn source_raw_integer_vectors_keep_their_exact_values() -> Result<(), HyperparamError> {
    for (name, raw) in [
        ("kappa", 32767u128),
        ("kappa", 65535),
        ("bonds_moving_avg", 1_000_000),
        ("adjustment_alpha", u128::from(u64::MAX)),
        ("difficulty", u128::from(u64::MAX)),
        ("tempo", 360),
        ("min_burn", 1_000_000_000_000_000_000),
    ] {
        assert_eq!(TABLE.to_raw(name, raw)?.as_u128(), Some(raw), "{name}");
    }
    Ok(())
}

// pyt: unit/test_hyperparams_units.py::test_to_raw_rejects_raw_above_denominator
// pyt: unit/test_hyperparams_units.py::test_to_raw_rejects_outside_codec_width
#[test]
fn source_raw_refusals_report_the_exact_fraction_semantic_or_codec_range() {
    for (name, raw, expected) in [
        (
            "kappa",
            65536u128,
            "kappa must be within 0..65535 (raw on-chain range); got 65536",
        ),
        (
            "max_weights_limit",
            1_000_000,
            "max_weights_limit must be within 0..65535 (raw on-chain range); got 1000000",
        ),
        (
            "bonds_moving_avg",
            1_000_001,
            "bonds_moving_avg must be within 0..1000000 (raw on-chain range); got 1000001",
        ),
        (
            "adjustment_alpha",
            1u128 << 64,
            "adjustment_alpha must be within 0..18446744073709551615 (raw on-chain range); got 18446744073709551616",
        ),
        (
            "rho",
            65536,
            "rho must be within 0..65535 (raw on-chain range); got 65536",
        ),
        (
            "tempo",
            1_000_000_000_000,
            "tempo must be within 360..50400 (raw on-chain range); got 1000000000000",
        ),
        (
            "min_burn",
            1u128 << 64,
            "min_burn must be within 0..18446744073709551615 (raw on-chain range); got 18446744073709551616",
        ),
    ] {
        let actual: Result<_, HyperparamError> = TABLE.to_raw(name, raw);
        assert_eq!(
            actual.map_err(|error| error.to_string()),
            Err(expected.into()),
            "{name}"
        );
    }
}

// pyt: unit/test_hyperparams_units.py::test_to_raw_bool_only_zero_one
#[test]
fn source_boolean_vectors_require_zero_or_one_and_explain_every_refusal()
-> Result<(), HyperparamError> {
    for (input, expected) in [
        (ProportionInput::from(true), 1),
        (false.into(), 0),
        ("true".into(), 1),
        ("false".into(), 0),
        (0.into(), 0),
        (1.into(), 1),
    ] {
        assert_eq!(
            TABLE.to_raw("registration_allowed", input)?,
            expected.into()
        );
    }
    for (input, expected) in [
        (
            ProportionInput::from(2),
            "registration_allowed takes true/false (or 1/0); got 2",
        ),
        (
            999.into(),
            "registration_allowed takes true/false (or 1/0); got 999",
        ),
        (
            "yes".into(),
            "registration_allowed takes true/false (or 1/0); got 'yes'",
        ),
        (
            "on".into(),
            "registration_allowed takes true/false (or 1/0); got 'on'",
        ),
        (
            "2".into(),
            "registration_allowed takes true/false (or 1/0); got '2'",
        ),
    ] {
        assert_eq!(
            TABLE
                .to_raw("registration_allowed", input)
                .map_err(|error| error.to_string()),
            Err(expected.into())
        );
    }
    Ok(())
}

// pyt: unit/test_hyperparams_units.py::test_to_raw_rejects_negative_human_rao_and_fixed128
// pyt: unit/test_hyperparams_units.py::test_to_raw_overflow_becomes_value_error
// pyt: unit/test_hyperparams_units.py::test_to_raw_human_forms_unchanged
#[test]
fn source_human_float_and_text_vectors_preserve_exact_rounding_and_error_categories()
-> Result<(), HyperparamError> {
    for (name, input, raw) in [
        ("kappa", ProportionInput::from(0.5), 32768u128),
        ("kappa", "0.5".into(), 32768),
        ("bonds_moving_avg", 0.9.into(), 900_000),
        ("min_burn", 0.7.into(), 700_000_000),
        ("registration_allowed", "true".into(), 1),
    ] {
        assert_eq!(TABLE.to_raw(name, input)?.as_u128(), Some(raw), "{name}");
    }
    // HyperparamError is the native typed equivalent of Python ValueError;
    // checking Result and the diagnostic distinguishes refusals from panics.
    for (name, input, expected) in [
        (
            "min_burn",
            ProportionInput::from(-0.5),
            "min_burn cannot be negative",
        ),
        (
            "burn_increase_mult",
            (-1.5).into(),
            "burn_increase_mult cannot be negative",
        ),
        (
            "burn_increase_mult",
            0.5.into(),
            "burn_increase_mult must be within 18446744073709551616..55340232221128654848 (raw on-chain range); got 9223372036854775808",
        ),
        (
            "burn_increase_mult",
            "1e10000".into(),
            "burn_increase_mult value is out of range",
        ),
        (
            "kappa",
            1.5.into(),
            "kappa is a normalized fraction: pass 0..1 (e.g. 0.5), or the raw integer 0..65535 without a decimal point",
        ),
        (
            "kappa",
            (-1).into(),
            "kappa must be within 0..65535 (raw on-chain range); got -1",
        ),
    ] {
        assert_eq!(
            TABLE.to_raw(name, input).map_err(|error| error.to_string()),
            Err(expected.into()),
            "{name}"
        );
    }
    Ok(())
}

// pyt: unit/test_hyperparams_units.py::test_to_raw_integer_is_raw_within_bounds
// pyt: unit/test_hyperparams_units.py::test_to_raw_rejects_raw_above_denominator
// pyt: unit/test_hyperparams_units.py::test_to_raw_rejects_outside_codec_width
#[test]
fn every_catalog_row_accepts_native_boundaries_and_rejects_neighbors()
-> Result<(), Box<dyn std::error::Error>> {
    for (name, _) in TABLE.entries {
        let (lo, hi) = TABLE.raw_bounds(name);
        let lo_big = rtao_primitives::pyfmt::parse_integer(&lo.to_string())
            .ok_or("invalid native lower bound")?;
        let hi_big = rtao_primitives::pyfmt::parse_integer(&hi.to_string())
            .ok_or("invalid native upper bound")?;
        assert_eq!(TABLE.to_raw(name, lo)?, lo, "{name}");
        assert_eq!(TABLE.to_raw(name, hi)?, hi, "{name}");
        let below = lo_big - 1;
        let above = hi_big + 1;
        assert!(
            TABLE.to_raw(name, ProportionInput::Raw(below)).is_err(),
            "{name}"
        );
        assert!(
            TABLE.to_raw(name, ProportionInput::Raw(above)).is_err(),
            "{name}"
        );
    }
    Ok(())
}

// pyt: unit/test_hyperparams_units.py::test_to_raw_bool_only_zero_one
#[test]
fn boolean_inputs_keep_type_distinctions() -> Result<(), HyperparamError> {
    for name in ["registration_allowed", "owner_cut_enabled"] {
        assert_eq!(TABLE.to_raw(name, true)?, 1.into());
        assert_eq!(TABLE.to_raw(name, false)?, 0.into());
        for (text, expected) in [
            ("true", 1),
            ("false", 0),
            ("1", 1),
            ("0", 0),
            (" +TRUE ", 1),
        ] {
            assert_eq!(TABLE.to_raw(name, text)?, expected.into());
        }
        assert_eq!(TABLE.to_raw(name, 0)?, 0.into());
        assert_eq!(TABLE.to_raw(name, 1)?, 1.into());
        for bad in ["yes", "on", "2", "1.0", "0.0", "1e0", "01", "１"] {
            assert!(TABLE.to_raw(name, bad).is_err(), "{bad}");
        }
        for bad in [0.0, 1.0, f64::NAN] {
            assert!(TABLE.to_raw(name, bad).is_err());
        }
        assert!(TABLE.to_raw(name, 2).is_err());
    }
    assert!(TABLE.to_raw("kappa", true).is_err());
    assert_eq!(TABLE.annotate("kappa", true), None);
    assert_eq!(TABLE.annotate("kappa", 32767.0), None);
    assert_eq!(TABLE.annotate("kappa", "32767"), None);
    Ok(())
}

// pyt: unit/test_hyperparams_units.py::test_to_raw_rejects_negative_human_rao_and_fixed128
// pyt: unit/test_hyperparams_units.py::test_to_raw_overflow_becomes_value_error
// pyt: unit/test_hyperparams_units.py::test_to_raw_human_forms_unchanged
#[test]
fn human_forms_preserve_bounds_and_python_diagnostics() -> Result<(), HyperparamError> {
    for (name, text, raw) in [
        ("kappa", "0.5", 32768u128),
        ("bonds_moving_avg", "0.9", 900000),
        ("min_burn", "0.7", 700000000),
        ("burn_increase_mult", "1.5", 3u128 << 63),
        ("min_burn", "7e-1", 700000000),
    ] {
        assert_eq!(TABLE.to_raw(name, text)?.as_u128(), Some(raw));
    }
    for name in ["min_burn", "burn_increase_mult"] {
        let result = TABLE.to_raw(name, -0.5);
        assert_eq!(
            result.map_err(|e| e.to_string()),
            Err(format!("{name} cannot be negative"))
        );
        assert_eq!(
            TABLE.to_raw(name, "1e10000").map_err(|e| e.to_string()),
            Err(format!("{name} value is out of range"))
        );
        assert_eq!(
            TABLE.to_raw(name, "nan").map_err(|e| e.to_string()),
            Err(format!(
                "{name} takes {}; got 'nan'",
                TABLE.value_forms(name)
            ))
        );
        assert_eq!(
            TABLE.to_raw(name, "nan.").map_err(|e| e.to_string()),
            Err(format!(
                "{name} takes {}; got 'nan.'",
                TABLE.value_forms(name)
            ))
        );
        assert_eq!(
            TABLE.to_raw(name, f64::NAN).map_err(|e| e.to_string()),
            Err("cannot convert float NaN to integer".into())
        );
    }
    assert_eq!(
        TABLE.to_raw("rho", 65536).map_err(|e| e.to_string()),
        Err("rho must be within 0..65535 (raw on-chain range); got 65536".into())
    );
    assert!(TABLE.to_raw("kappa", 1.5).is_err());
    assert!(TABLE.to_raw("kappa", -1).is_err());
    assert!(TABLE.to_raw("burn_increase_mult", 0.5).is_err());
    assert_eq!(TABLE.to_raw("rho", 10.0)?, 10.into());
    assert!(TABLE.to_raw("rho", 10.1).is_err());
    assert_eq!(TABLE.to_raw("rho", "++1")?, 1.into());
    assert_eq!(TABLE.to_raw("rho", " +１_２ ")?, 12.into());
    Ok(())
}

#[test]
fn normalized_and_annotations_divide_once_and_never_clamp() {
    assert_eq!(TABLE.normalized("kappa", -65535), Some(-1.0));
    assert_eq!(TABLE.normalized("kappa", 131070), Some(2.0));
    assert_eq!(TABLE.normalized("rho", 1), None);
    for (name, raw, expected) in [
        ("kappa", 32767u128, "≈ 0.5"),
        ("kappa", 1, "≈ 1.526e-05"),
        ("kappa", 9_007_199_254_740_993, "≈ 1.374e+11"),
        ("min_burn", 700_000_000, "= τ0.7"),
        ("min_burn", u128::from(u64::MAX), "= τ1.84467e+10"),
        ("burn_increase_mult", 3u128 << 63, "≈ 1.5"),
        ("difficulty", u128::from(u64::MAX), "= u64::MAX"),
        ("immunity_period", 5, "≈ 1m"),
        ("immunity_period", 360, "≈ 1h 12m"),
        ("immunity_period", 7205, "≈ 1d"),
        ("immunity_period", 7505, "≈ 1d 1h"),
    ] {
        assert_eq!(
            TABLE.annotate(name, raw).as_deref(),
            Some(expected),
            "{name} {raw}"
        );
    }
    assert_eq!(TABLE.annotate("immunity_period", 4), None);
    assert_eq!(TABLE.annotate("difficulty", u64::MAX - 1), None);
    assert_eq!(TABLE.annotate("unknown", 1), None);
}
