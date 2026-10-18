// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_emission_weights.py @ 6a972693ae9580541225bd37696ac2fee26b8ae9 (Apache-2.0); modified by rtao.
//! Pure emission weight rules, parsing, renormalisation and previews.
#![allow(clippy::unwrap_used)] // Test failures identify unexpected fixture values.

use rtao_primitives::emission_weights::{
    self as ew, BURN, DEFAULT_OWNER, MAX_OWNER, MAX_VALIDATOR, MIN_MINER, MINER, ONE, OWNER,
    RECYCLE, RootInputs, Rules, ShareInput, VALIDATOR, Weights,
};
use serde_json::json;

fn default() -> Weights {
    ew::default_weights(DEFAULT_OWNER)
}

fn share(text: &str) -> u64 {
    ew::parse_share(text).unwrap()
}

fn close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{actual} is not within {tolerance} of {expected}"
    );
}

// Serialization order is the Python dictionary order; the JSON map itself may sort.
fn assert_keys_in_order(text: &str, keys: &[&str]) {
    let positions: Vec<_> = keys
        .iter()
        .map(|key| text.find(&format!("\"{key}\":")).unwrap())
        .collect();
    assert!(positions.is_sorted(), "{text}");
}

fn approx(actual: f64, expected: f64) {
    close(actual, expected, expected.abs() * 1e-6 + 1e-12);
}

#[test]
fn constants_match_chain() {
    let one = u128::from(ONE);
    assert_eq!(u128::from(MAX_OWNER), one * 33 / 100);
    assert_eq!(u128::from(MAX_VALIDATOR), one * 50 / 100);
    assert_eq!(u128::from(MIN_MINER), one * 33 / 100);
    assert_eq!(u128::from(DEFAULT_OWNER), 11796 * one / 65535);
    let weights = default();
    assert_eq!(weights.iter().map(|w| u128::from(*w)).sum::<u128>(), one);
    assert_eq!((weights[BURN], weights[RECYCLE]), (0, 0));
    assert_eq!(weights[VALIDATOR], (ONE - DEFAULT_OWNER) / 2);
    assert_eq!(weights[VALIDATOR], ew::DEFAULT_VALIDATOR);
}

#[test]
fn slot_index_accepts_names_and_uids() {
    for (raw, expected) in [("owner", 2), ("Validator", 3), (" miner ", 4), ("3", 3)] {
        assert_eq!(ew::slot_index(raw).unwrap(), expected, "{raw}");
    }
    assert_eq!(ew::slot_index(0usize).unwrap(), 0);
    assert_eq!(ew::slot_index(4usize).unwrap(), 4);
    assert_eq!(ew::slot_index_json(&json!(true)).unwrap(), 1);
    assert_eq!(ew::slot_index_json(&json!(2.9)).unwrap(), 2);
}

#[test]
fn slot_index_rejects_unknown() {
    for error in [
        ew::slot_index("gamma"),
        ew::slot_index(5usize),
        ew::slot_index(-1i64),
        ew::slot_index("5"),
        ew::slot_index_json(&json!(5.0)),
        ew::slot_index_json(&json!(null)),
    ] {
        assert!(error.unwrap_err().to_string().contains("slots are"));
    }
    assert_eq!(
        ew::slot_index("gamma").unwrap_err().to_string(),
        "unknown emission slot 'gamma'; slots are 0 burn, 1 recycle, 2 owner, 3 validator, 4 miner"
    );
    assert_eq!(
        ew::slot_index("5").unwrap_err().to_string(),
        "emission slot 5 is out of range; slots are 0 burn, 1 recycle, 2 owner, 3 validator, 4 miner"
    );
}

#[test]
fn parse_share_forms() {
    assert_eq!(ew::parse_share(0.5).unwrap(), ew::fraction(1, 2));
    assert_eq!(share("41%"), ew::fraction(41, 100));
    assert_eq!(share("0.41"), ew::fraction(41, 100));
    assert_eq!(share(" 12.5% "), ew::fraction(125, 1000));
    assert_eq!(share("1e-1"), ew::fraction(1, 10));
    assert_eq!(ew::parse_share(12345u64).unwrap(), 12345);
    assert_eq!(share("12345"), 12345);
    assert_eq!(share("1_000"), 1000);
    assert_eq!(share("1.0"), ONE);
    assert_eq!(share("100%"), ONE);
    assert_eq!(share("0"), 0);
    assert_eq!(share("0%"), 0);
    assert_eq!(share("-0.0"), 0);
    assert_eq!(share("1e-30"), 0);
    assert_eq!(share("٠.٥"), ew::fraction(1, 2));
    assert_eq!(ew::parse_share_json(&json!(0.25)).unwrap(), ONE / 4);
    assert_eq!(ew::parse_share_json(&json!("25%")).unwrap(), ONE / 4);
    assert_eq!(ew::parse_share_json(&json!(7)).unwrap(), 7);
}

#[test]
fn parse_share_lands_exactly_on_chain_bounds() {
    for (raw, bound) in [
        (ShareInput::from("33%"), MAX_OWNER),
        (ShareInput::from("0.33"), MAX_OWNER),
        (ShareInput::from(0.33), MAX_OWNER),
        (ShareInput::from("50%"), MAX_VALIDATOR),
        (ShareInput::from("0.5"), MAX_VALIDATOR),
        (ShareInput::from(0.5), MAX_VALIDATOR),
        (ShareInput::from("33%"), MIN_MINER),
    ] {
        assert_eq!(ew::parse_share(raw.clone()).unwrap(), bound, "{raw:?}");
    }
}

#[test]
fn documented_bounds_pass_validation_in_every_spelling() {
    let cases: [&[(&str, ShareInput<'_>)]; 7] = [
        &[("owner", "33%".into()), ("validator", "34%".into())],
        &[("owner", "0.33".into()), ("validator", "0.34".into())],
        &[("owner", 0.33.into()), ("validator", 0.34.into())],
        &[("owner", "17%".into()), ("validator", "50%".into())],
        &[("owner", "0.17".into()), ("validator", "0.5".into())],
        &[("owner", "18%".into()), ("miner", "33%".into())],
        &[("owner", 0.18.into()), ("miner", 0.33.into())],
    ];
    for pinned in cases {
        let fixed: Vec<_> = pinned
            .iter()
            .map(|(slot, value)| {
                (
                    ew::slot_index(*slot).unwrap(),
                    ew::parse_share(value.clone()).unwrap(),
                )
            })
            .collect();
        let weights = ew::renormalize(&default(), &fixed).unwrap();
        assert_eq!(ew::validate(&weights, &Rules::default()), [], "{pinned:?}");
    }
}

#[test]
fn parse_share_rejects_out_of_range() {
    for raw in [
        ShareInput::from("1.5"),
        ShareInput::from("150%"),
        ShareInput::from("-0.1"),
        ShareInput::from(-5i64),
        ShareInput::from(u128::from(ONE) + 1),
        ShareInput::from("abc"),
        ShareInput::from(""),
        ShareInput::from("%"),
        ShareInput::from("1e999999999999999999999"),
        ShareInput::from("nan%"),
        ShareInput::from(true),
    ] {
        assert!(ew::parse_share(raw.clone()).is_err(), "{raw:?}");
    }
    assert_eq!(
        ew::parse_share("lots").unwrap_err().to_string(),
        "cannot read emission share 'lots'; give a 0..1 fraction (0.41), a percentage (41%), or raw u64 parts"
    );
    assert_eq!(
        ew::parse_share("1.5").unwrap_err().to_string(),
        "emission share '1.5' must be between 0 and 1 (0% and 100%)"
    );
    assert_eq!(
        ew::parse_share(-5i64).unwrap_err().to_string(),
        "emission share -5 is outside 0..1 (0..18446744073709551615 parts)"
    );
    assert_eq!(
        ew::parse_share(true).unwrap_err().to_string(),
        "emission share must be a number, not a boolean"
    );
    assert_eq!(
        ew::parse_share("  ").unwrap_err().to_string(),
        "emission share is empty"
    );
}

#[test]
fn format_share_rounds_like_python() {
    assert_eq!(ew::format_share(ONE, 2), "100.00%");
    assert_eq!(ew::format_share(ONE / 2, 2), "50.00%");
    assert_eq!(ew::format_share(0, 2), "0.00%");
    assert_eq!(ew::format_share(0, 0), "0%");
    // 0.125 is an exact binary tie; Python's format rounds it to even.
    assert_eq!(format!("{:.2}", 0.125), "0.12");
    assert_eq!(format!("{:.0}", 2.5), "2");
}

#[test]
fn merge_zeroes_reserved_slots_and_keeps_the_rest() {
    let current = [7, 9, 10, 20, 30];
    assert_eq!(ew::merge(&current, &[(OWNER, 11)]), [0, 0, 11, 20, 30]);
    assert_eq!(
        ew::merge(&current, &[(VALIDATOR, 1), (MINER, 2)]),
        [0, 0, 10, 1, 2]
    );
}

#[test]
fn renormalize_scales_untouched_slots_proportionally() {
    let owner = share("10%");
    let out = ew::renormalize(&default(), &[(OWNER, owner)]).unwrap();
    assert_eq!(
        out.iter().map(|w| u128::from(*w)).sum::<u128>(),
        u128::from(ONE)
    );
    assert_eq!(out[OWNER], owner);
    assert!(out[VALIDATOR].abs_diff(out[MINER]) <= 4);
    assert!(out[VALIDATOR] > default()[VALIDATOR]);
}

#[test]
fn renormalize_keeps_two_pinned_and_fills_the_third() {
    let out = ew::renormalize(&default(), &[(OWNER, ONE / 10), (VALIDATOR, ONE / 2)]).unwrap();
    assert_eq!(out, [0, 0, ONE / 10, ONE / 2, ONE - ONE / 10 - ONE / 2]);
}

#[test]
fn renormalize_rejects_overflow_and_nonzero_reserved() {
    let error =
        |fixed: &[(usize, u64)]| ew::renormalize(&default(), fixed).unwrap_err().to_string();
    assert!(error(&[(OWNER, ONE), (VALIDATOR, 1)]).contains("exceed 100%"));
    assert_eq!(
        error(&[(BURN, 1)]),
        "burn (slot 0) must stay 0 in this runtime"
    );
    assert!(error(&[(OWNER, 1), (VALIDATOR, 1), (MINER, 1)]).contains("not 100%"));
}

#[test]
fn renormalize_splits_evenly_when_free_slots_are_zero() {
    let out = ew::renormalize(&[0, 0, ONE, 0, 0], &[(OWNER, ONE / 2)]).unwrap();
    assert_eq!(
        out.iter().map(|w| u128::from(*w)).sum::<u128>(),
        u128::from(ONE)
    );
    assert!(out[VALIDATOR].abs_diff(out[MINER]) <= 1);
}

#[test]
fn validate_accepts_default_and_names_each_rule() {
    let rules = Rules {
        min_validator: Some(share("2.2%")),
        ..Rules::default()
    };
    assert_eq!(ew::validate(&default(), &rules), []);
    let errors = |weights: Weights, rules: &Rules| -> Vec<&'static str> {
        ew::validate(&weights, rules)
            .into_iter()
            .map(|violation| violation.error)
            .collect()
    };
    let d = default();
    assert_eq!(
        errors([1, 0, d[2], d[3], d[4]], &rules),
        ["BurnEmissionMustBeZero", "EmissionWeightsDoNotSumToOne"]
    );
    assert!(errors([0, 1, d[2], d[3], d[4]], &rules).contains(&"RecycleEmissionMustBeZero"));
    let renormalized = |slot, text| ew::renormalize(&d, &[(slot, share(text))]).unwrap();
    assert!(errors(renormalized(OWNER, "34%"), &rules).contains(&"OwnerEmissionAboveMaximum"));
    assert!(
        errors(renormalized(VALIDATOR, "51%"), &rules).contains(&"ValidatorEmissionAboveMaximum")
    );
    assert_eq!(
        errors(renormalized(VALIDATOR, "1%"), &rules),
        ["ValidatorEmissionTooLowToPreserveRoot"]
    );
    assert_eq!(
        errors([0, 0, 0, 0, ONE], &rules),
        ["ValidatorEmissionTooLowToPreserveRoot"]
    );
    // With no root stake there is nothing to preserve: even V = 0 passes.
    assert_eq!(
        errors([0, 0, 0, 0, ONE], &Rules::default()),
        [] as [&str; 0]
    );
    assert!(
        errors(
            [
                0,
                0,
                MAX_OWNER,
                MAX_VALIDATOR,
                ONE - MAX_OWNER - MAX_VALIDATOR
            ],
            &rules
        )
        .contains(&"MinerEmissionBelowMinimum")
    );
    assert_eq!(
        errors([0, 0, 0, ONE / 2, ONE / 2], &rules),
        ["EmissionWeightsDoNotSumToOne"]
    );
    let sum = ew::validate(&[0, 0, 0, ONE / 2, ONE / 2], &rules);
    assert_eq!(
        sum[0].to_string(),
        "EmissionWeightsDoNotSumToOne: the five slots sum to 100.000000% (18446744073709551614 parts), not exactly 100% (18446744073709551615 parts); renormalise another slot in the same call (`btcli subnets emission set-value` does this for you)"
    );
}

#[test]
fn validate_lease_floor_only_on_lease_created_subnets() {
    let low_owner = ew::renormalize(&default(), &[(OWNER, share("5%"))]).unwrap();
    assert_eq!(ew::validate(&low_owner, &Rules::default()), []);
    let leased = Rules {
        min_owner: DEFAULT_OWNER,
        created_via_lease: true,
        ..Rules::default()
    };
    let violations = ew::validate(&low_owner, &leased);
    assert_eq!(violations.len(), 1);
    assert_eq!(violations[0].error, "LeasedSubnetOwnerCutBelowMinimum");
    assert!(violations[0].message.contains("lease"));
}

#[test]
fn root_preservation_helpers_match_sn4_numbers() {
    // Mainnet SN4-like: 5.443M TAO on root, 6.457M alpha issued, TaoWeight 18%, V0 = 41%.
    let (root_tao, issuance, tao_weight) = (5_443_000e9, 6_457_000e9, 0.18);
    let baseline = ew::baseline_root_share(tao_weight, root_tao, issuance);
    close(baseline, 0.0540, 1e-4);
    let validator = ew::share_to_float(default()[VALIDATOR]);
    approx(
        ew::root_emission_share(validator, tao_weight, root_tao, issuance),
        baseline,
    );
    approx(
        ew::validator_floor(tao_weight, root_tao, issuance),
        baseline,
    );
    let baseline_parts = (baseline * ONE as f64) as u64;
    assert_eq!(ew::min_validator_parts(baseline_parts), baseline_parts + 1);
    assert_eq!(ew::min_validator_parts(ONE), ONE);
    approx(ew::root_share_paid(share("30%"), baseline_parts), baseline);
    approx(ew::root_share_paid(share("5%"), baseline_parts), 0.05);
    approx(ew::root_share_paid(share("5.5%"), baseline_parts), baseline);
    assert!(ew::baseline_root_share(0.36, root_tao, issuance) > baseline);
    assert!(ew::baseline_root_share(tao_weight, root_tao, 10.0 * issuance) < baseline);
    assert_eq!(ew::validator_floor(tao_weight, 0.0, issuance), 0.0);
    approx(ew::validator_floor(tao_weight, root_tao, 0.0), validator);
    assert_eq!(
        ew::root_emission_share(validator, tao_weight, 0.0, 0.0),
        0.0
    );
}

#[test]
fn preview_reports_vectors_and_root_shares() {
    let (root_tao, issuance, tao_weight) =
        (5_443_000 * 10u64.pow(9), 6_457_000 * 10u128.pow(9), 0.18);
    let baseline_parts =
        (ew::baseline_root_share(tao_weight, root_tao as f64, issuance as f64) * ONE as f64) as u64;
    let baseline = ew::share_to_float(baseline_parts);
    let inputs = RootInputs {
        baseline_parts,
        global_tao_weight: tao_weight,
        root_tao,
        alpha_issuance: issuance,
    };
    let d = default();
    let after = ew::renormalize(&d, &[(VALIDATOR, share("30%"))]).unwrap();
    let preview = ew::preview(&d, &after, &inputs);
    assert_eq!(preview.before.weights, d);
    assert_eq!(preview.after.weights, after);
    approx(preview.after.shares.validator, 0.30);
    assert_eq!(preview.changed, ["owner", "validator", "miner"]);
    let root = &preview.root;
    assert_eq!(root.baseline_share, baseline);
    approx(root.share_before, baseline);
    approx(root.share_after.unwrap(), baseline);
    close(root.validators_before, 0.41 - baseline, 1e-4);
    approx(root.validators_after.unwrap(), 0.30 - baseline);
    assert_eq!((root.capped_before, root.capped_after), (false, false));
    assert_eq!(root.validator_floor, baseline);
    assert_eq!(root.global_tao_weight, tao_weight);
    assert_eq!((root.root_tao, root.alpha_issuance), (root_tao, issuance));
    assert_keys_in_order(
        &serde_json::to_string(&preview).unwrap(),
        &["before", "after", "changed", "root"],
    );

    let too_low = ew::preview(
        &d,
        &ew::renormalize(&d, &[(VALIDATOR, share("5%"))]).unwrap(),
        &inputs,
    );
    assert_eq!(too_low.root.share_after, None);
    assert_eq!(too_low.root.validators_after, None);
    assert!(!too_low.root.capped_after);

    let capped = ew::preview(
        &ew::renormalize(&d, &[(VALIDATOR, share("5%"))]).unwrap(),
        &ew::renormalize(&d, &[(VALIDATOR, share("5.5%"))]).unwrap(),
        &inputs,
    );
    assert!(capped.root.capped_before);
    approx(capped.root.share_before, 0.05);
    close(capped.root.validators_before, 0.0, 1e-12);
    assert!(!capped.root.capped_after);
    approx(capped.root.share_after.unwrap(), baseline);
    approx(capped.root.validators_after.unwrap(), 0.055 - baseline);
}

#[test]
fn rate_limit_status() {
    let status = ew::rate_limit_status(0, 1_000, ew::OWNER_RATE_LIMIT_BLOCKS);
    assert!(status.owner_can_change_now);
    assert_eq!(status.next_allowed_block, 1_000);
    let status = ew::rate_limit_status(900, 1_000, ew::OWNER_RATE_LIMIT_BLOCKS);
    assert_eq!(
        serde_json::to_value(status).unwrap(),
        json!({
            "limit_blocks": 50_400,
            "last_change_block": 900,
            "next_allowed_block": 51_300,
            "blocks_until_allowed": 50_300,
            "owner_can_change_now": false,
        })
    );
    assert!(ew::rate_limit_status(900, 51_300, ew::OWNER_RATE_LIMIT_BLOCKS).owner_can_change_now);
}

#[test]
fn rules_limits_and_table_cover_every_slot() {
    let rules = Rules {
        min_validator: Some(share("2.2%")),
        min_owner: DEFAULT_OWNER,
        created_via_lease: true,
    };
    let limits = rules.limits();
    assert_keys_in_order(
        &serde_json::to_string(&limits).unwrap(),
        &["burn", "recycle", "owner", "validator", "miner"],
    );
    assert_eq!(limits.owner.min_parts, Some(DEFAULT_OWNER));
    assert!(limits.owner.why.contains("lease"));
    close(limits.validator.min.unwrap(), 0.022, 1e-12);
    assert_eq!(limits.validator.max_parts, MAX_VALIDATOR);
    assert_eq!(limits.miner.max_parts, ONE);
    let rows = |rules: &Rules| -> std::collections::BTreeMap<&'static str, String> {
        rules
            .table()
            .into_iter()
            .map(|(name, rule, _)| (name, rule))
            .collect()
    };
    let table = rows(&rules);
    assert_eq!(table["burn"], "= 0%");
    assert_eq!(table["recycle"], "= 0%");
    assert_eq!(table["owner"], "18.00% to 33.00%");
    assert_eq!(table["validator"], "2.2000% to 50.0000%");
    assert_eq!(table["miner"], ">= 33.00%");
    assert_eq!(table["sum"], "= 100%");
    let table = rows(&Rules::default());
    assert_eq!(table["validator"], "<= 50.0000%");
    assert_eq!(table["owner"], "0.00% to 33.00%");
    assert_eq!(Rules::default().limits().validator.min, None);
    assert_eq!(
        serde_json::to_value(Rules::default().limits().owner).unwrap(),
        json!({
            "min": 0.0,
            "max": ew::share_to_float(MAX_OWNER),
            "min_parts": 0,
            "max_parts": MAX_OWNER,
            "why": "0 turns the owner cut off",
        })
    );
}

#[test]
fn normalize_weights_pads_and_bounds_chain_vectors() {
    assert_eq!(ew::normalize_weights(&[1, 2]).unwrap(), [1, 2, 0, 0, 0]);
    assert_eq!(
        ew::normalize_weights(&[0; 6]).unwrap_err().to_string(),
        "emission weight vector has 6 slots; at most 5 are known"
    );
    assert_eq!(
        ew::describe(&default())[OWNER],
        ("owner", "18.00%".to_owned())
    );
}
