//! Generated error classification preserves Python's public precedence.

use rtao_catalog::{
    ErrorCode,
    errors::{classify_error, custom_transaction_name, invalid_transaction_name},
};

#[test]
fn names_precede_text_and_unknown_names_do_not_use_text_fallback() {
    assert_eq!(
        classify_error("insufficient balance", Some("SlippageTooHigh")),
        ErrorCode::InsufficientLiquidity
    );
    assert_eq!(
        classify_error("insufficient balance", Some("NotAnError")),
        ErrorCode::Unknown
    );
    assert_eq!(
        classify_error("unrelated", Some("NewSubnetRateLimit")),
        ErrorCode::RateLimited
    );
    assert_eq!(
        classify_error("unrelated", Some("NewRateLimitExceeded")),
        ErrorCode::RateLimited
    );
    assert_eq!(
        classify_error("BALANCE TOO LOW", None),
        ErrorCode::InsufficientBalance
    );
    assert_eq!(
        classify_error("BALANCE TOO LOW", Some("")),
        ErrorCode::InsufficientBalance
    );
    assert_eq!(classify_error("unknown failure", None), ErrorCode::Unknown);
    assert_eq!(
        classify_error("not enough stake", Some("BadOrigin")),
        ErrorCode::NotAuthorized
    );
}

#[test]
fn custom_and_invalid_transaction_tables_have_exact_lookups() {
    assert_eq!(custom_transaction_name(8), Some("SlippageTooHigh"));
    assert_eq!(custom_transaction_name(255), Some("BadRequest"));
    assert_eq!(custom_transaction_name(28), None);
    assert_eq!(custom_transaction_name(254), None);
    assert_eq!(
        invalid_transaction_name("Transaction is OUTDATED"),
        Some("Stale")
    );
    assert_eq!(
        invalid_transaction_name("Node error without a known needle"),
        None
    );
}

#[test]
fn every_catalog_semantic_code_round_trips_as_a_python_json_token()
-> Result<(), Box<dyn std::error::Error>> {
    let catalog: serde_json::Value =
        serde_json::from_str(include_str!("../../../vendor/upstream/catalog/errors.json"))?;
    let codes = catalog["codes"]
        .as_object()
        .ok_or("catalog codes missing")?;
    for code in codes.keys() {
        let encoded = serde_json::Value::String(code.clone());
        let parsed: ErrorCode = serde_json::from_value(encoded.clone())?;
        assert_eq!(parsed.as_str(), code);
        assert_eq!(ErrorCode::from_code(code), Some(parsed));
        assert_eq!(serde_json::to_value(parsed)?, encoded);
    }
    assert!(ErrorCode::from_code("NOT_AUTHORIZED").is_none());
    assert!(serde_json::from_value::<ErrorCode>(serde_json::json!("not-an-error-code")).is_err());
    Ok(())
}
