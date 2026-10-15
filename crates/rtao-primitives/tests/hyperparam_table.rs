// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_hyperparams_units.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Synthetic-table tests for metadata and codec precedence.

use rtao_primitives::hyperparams::{
    Hyperparam, HyperparamError, HyperparamStorage, HyperparamTable, RawInteger,
};

const BASE: Hyperparam = Hyperparam {
    kind: "int",
    doc: "full docs",
    short: "short",
    signed: None,
    bits: None,
    minimum: None,
    maximum: None,
};
const TABLE: HyperparamTable = HyperparamTable {
    entries: &[
        (
            "metadata",
            Hyperparam {
                kind: "blocks",
                ..BASE
            },
        ),
        (
            "hand",
            Hyperparam {
                kind: "u16",
                ..BASE
            },
        ),
        (
            "signed_only",
            Hyperparam {
                signed: Some(true),
                ..BASE
            },
        ),
        (
            "explicit_unsigned",
            Hyperparam {
                bits: Some(8),
                ..BASE
            },
        ),
        (
            "explicit_signed",
            Hyperparam {
                bits: Some(16),
                signed: Some(true),
                ..BASE
            },
        ),
        (
            "semantic",
            Hyperparam {
                bits: Some(16),
                minimum: Some(RawInteger::Unsigned(360)),
                maximum: Some(RawInteger::Unsigned(50400)),
                ..BASE
            },
        ),
        (
            "fraction",
            Hyperparam {
                kind: "u16",
                bits: Some(32),
                maximum: Some(RawInteger::Unsigned(70000)),
                ..BASE
            },
        ),
        (
            "fixed",
            Hyperparam {
                kind: "fixed128",
                ..BASE
            },
        ),
        (
            "rao",
            Hyperparam {
                kind: "rao",
                ..BASE
            },
        ),
        ("empty", Hyperparam { short: "", ..BASE }),
        (
            "bool",
            Hyperparam {
                kind: "bool",
                ..BASE
            },
        ),
    ],
    storage: &[
        HyperparamStorage {
            parameter: "metadata",
            container: "p",
            name: "i",
            source_ident: "u16",
            metadata_ident: "PerU16",
        },
        HyperparamStorage {
            parameter: "signed_only",
            container: "p",
            name: "i",
            source_ident: "i32",
            metadata_ident: "i32",
        },
        HyperparamStorage {
            parameter: "explicit_unsigned",
            container: "p",
            name: "i",
            source_ident: "i32",
            metadata_ident: "i32",
        },
        HyperparamStorage {
            parameter: "explicit_signed",
            container: "p",
            name: "i",
            source_ident: "u128",
            metadata_ident: "u128",
        },
    ],
    ident_kinds: &[("PerU16", "u16")],
    ident_codecs: &[
        ("PerU16", (false, 16)),
        ("i32", (true, 32)),
        ("u128", (false, 128)),
    ],
    kind_codecs: &[
        ("u16", (false, 16)),
        ("int", (false, 64)),
        ("fixed128", (false, 128)),
        ("rao", (false, 64)),
        ("bool", (false, 1)),
    ],
    fraction_denominators: &[("u16", 65535)],
    blocktime: 12.0,
    rao_per_tao: 1_000_000_000,
    tao_symbol: "τ",
    fixed128_one: 1u128 << 64,
    short_max: 38,
};

// pyt: unit/test_hyperparams_units.py::test_metadata_identity_overrides_hand_table
#[test]
fn metadata_hand_and_unknown_precedence() {
    assert_eq!(TABLE.metadata_kind("metadata"), Some("u16"));
    assert_eq!(TABLE.kind_of("metadata"), "u16");
    assert_eq!(TABLE.normalized("metadata", 32767), Some(32767.0 / 65535.0));
    assert_eq!(TABLE.metadata_kind("hand"), None);
    assert_eq!(TABLE.kind_of("hand"), "u16");
    assert_eq!(TABLE.kind_of("unknown"), "int");
    assert_eq!(TABLE.normalized("hand", true), Some(1.0 / 65535.0));
    assert_eq!(TABLE.normalized("hand", 0.5), Some(0.5 / 65535.0));
    assert_eq!(TABLE.codec_of("unknown"), (false, 64));
    assert_eq!(TABLE.doc_of("hand"), Some("full docs"));
    assert_eq!(TABLE.short_of("hand"), Some("short"));
    assert_eq!(TABLE.short_of("empty"), None);
    assert_eq!(TABLE.doc_of("unknown"), None);
}

#[test]
fn codec_precedence_uses_explicit_bits_then_metadata_then_kind() -> Result<(), HyperparamError> {
    assert_eq!(TABLE.codec_of("explicit_unsigned"), (false, 8));
    assert_eq!(TABLE.codec_of("explicit_signed"), (true, 16));
    assert_eq!(TABLE.codec_of("signed_only"), (true, 32));
    assert_eq!(TABLE.codec_of("hand"), (false, 16));
    assert_eq!(
        TABLE.raw_bounds("explicit_signed"),
        ((-32768).into(), 32767.into())
    );
    assert_eq!(
        TABLE.to_raw("explicit_signed", -32768)?.as_i128(),
        Some(-32768)
    );
    assert_eq!(
        TABLE.to_raw("explicit_signed", 32767)?.as_i128(),
        Some(32767)
    );
    assert!(TABLE.to_raw("explicit_signed", 32768).is_err());
    assert!(TABLE.to_raw("explicit_unsigned", -1).is_err());
    Ok(())
}

#[test]
fn semantic_fraction_and_native_bounds_intersect() -> Result<(), HyperparamError> {
    assert_eq!(TABLE.raw_bounds("semantic"), (360.into(), 50400.into()));
    assert_eq!(TABLE.raw_bounds("fraction"), (0.into(), 65535.into()));
    assert_eq!(TABLE.to_raw("semantic", 360)?, 360.into());
    assert_eq!(TABLE.to_raw("semantic", 50400)?, 50400.into());
    assert!(TABLE.to_raw("semantic", 359).is_err());
    assert!(TABLE.to_raw("semantic", 50401).is_err());
    assert_eq!(TABLE.to_raw("fixed", u128::MAX)?.as_u128(), Some(u128::MAX));
    assert!(TABLE.to_raw("rao", 1u128 << 64).is_err());
    assert_eq!(TABLE.to_raw("rao", u64::MAX)?.as_u64(), Some(u64::MAX));
    assert_eq!(TABLE.to_raw("fixed", u128::MAX)?.as_u64(), None);
    Ok(())
}

#[test]
fn half_even_rounding_uses_binary64_human_forms() -> Result<(), HyperparamError> {
    assert_eq!(TABLE.to_raw("rao", 0.5e-9)?, 0.into());
    assert_eq!(TABLE.to_raw("rao", 1.5e-9)?, 2.into());
    assert_eq!(TABLE.to_raw("fixed", 2.0f64.powi(-65))?, 0.into());
    assert_eq!(TABLE.to_raw("fixed", 3.0 * 2.0f64.powi(-65))?, 2.into());
    assert_eq!(TABLE.to_raw("fraction", 0.5)?, 32768.into());
    assert_eq!(TABLE.to_raw("fraction", "1")?, 1.into());
    assert_eq!(TABLE.to_raw("fraction", "1e0")?, 65535.into());
    assert_eq!(TABLE.to_raw("fraction", ".٥")?, 32768.into());
    Ok(())
}

#[test]
fn signed_and_unsigned_comparison_and_checked_accessors() {
    let minus = RawInteger::Signed(i128::MIN);
    let max = RawInteger::Unsigned(u128::MAX);
    assert!(minus < max);
    assert_eq!(RawInteger::Signed(1), RawInteger::Unsigned(1));
    assert_eq!(minus.as_u128(), None);
    assert_eq!(max.as_i128(), None);
    assert_eq!(max.to_string(), u128::MAX.to_string());
}

#[test]
fn boolean_zero_one_bypass_semantic_bounds_as_in_python() -> Result<(), HyperparamError> {
    const BOUNDED: HyperparamTable = HyperparamTable {
        entries: &[(
            "flag",
            Hyperparam {
                kind: "bool",
                minimum: Some(RawInteger::Unsigned(1)),
                ..BASE
            },
        )],
        ..TABLE
    };
    assert_eq!(BOUNDED.to_raw("flag", 0)?, 0.into());
    assert_eq!(BOUNDED.to_raw("flag", false)?, 0.into());
    assert_eq!(BOUNDED.to_raw("flag", "0")?, 0.into());
    assert!(BOUNDED.to_raw("flag", 0.0).is_err());
    Ok(())
}

#[test]
fn fixed128_hints_compare_the_mathematical_scale_without_overflow() -> Result<(), HyperparamError> {
    const SCALE: u128 = 1 << 127;
    const LARGE: HyperparamTable = HyperparamTable {
        entries: &[
            (
                "full_range",
                Hyperparam {
                    kind: "fixed128",
                    minimum: Some(RawInteger::Unsigned(SCALE)),
                    ..BASE
                },
            ),
            (
                "wrapped_ceiling",
                Hyperparam {
                    kind: "fixed128",
                    minimum: Some(RawInteger::Unsigned(SCALE)),
                    maximum: Some(RawInteger::Unsigned(SCALE)),
                    ..BASE
                },
            ),
        ],
        fixed128_one: SCALE,
        ..TABLE
    };
    const GENERIC: &str =
        "a non-negative multiplier with a decimal point (e.g. 1.5) or the raw U64F64 bits";

    assert_eq!(
        LARGE.raw_bounds("full_range"),
        (SCALE.into(), u128::MAX.into())
    );
    assert_eq!(LARGE.value_forms("full_range"), GENERIC);
    // Wrapping 3 * SCALE would equal this ceiling and incorrectly advertise 1..3.
    assert_eq!(LARGE.value_forms("wrapped_ceiling"), GENERIC);
    assert_eq!(LARGE.to_raw("full_range", 1.0)?, SCALE.into());
    assert!(LARGE.to_raw("full_range", 3.0).is_err());
    assert_eq!(
        LARGE
            .to_raw("full_range", "invalid")
            .map_err(|error| error.to_string()),
        Err(format!("full_range takes {GENERIC}; got 'invalid'"))
    );
    Ok(())
}

#[test]
fn fixed128_hints_preserve_the_exact_native_multiplication_boundary() {
    const SCALE: u128 = u128::MAX / 3;
    const AT_LIMIT: HyperparamTable = HyperparamTable {
        entries: &[(
            "multiplier",
            Hyperparam {
                kind: "fixed128",
                minimum: Some(RawInteger::Unsigned(SCALE)),
                maximum: Some(RawInteger::Unsigned(u128::MAX)),
                ..BASE
            },
        )],
        fixed128_one: SCALE,
        ..TABLE
    };
    const ABOVE_LIMIT: HyperparamTable = HyperparamTable {
        entries: &[(
            "multiplier",
            Hyperparam {
                minimum: Some(RawInteger::Unsigned(SCALE + 1)),
                ..AT_LIMIT.entries[0].1
            },
        )],
        fixed128_one: SCALE + 1,
        ..AT_LIMIT
    };

    assert_eq!(
        AT_LIMIT.value_forms("multiplier"),
        "a multiplier with a decimal point in 1..3 (e.g. 1.5) or the raw U64F64 bits"
    );
    // Saturating multiplication would incorrectly select the special hint here.
    assert_eq!(
        ABOVE_LIMIT.value_forms("multiplier"),
        "a non-negative multiplier with a decimal point (e.g. 1.5) or the raw U64F64 bits"
    );
}
