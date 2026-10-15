//! Validate input spelling independently from chain account serialization.

use rtao_core::codec::Value;
use rtao_primitives::{AccountId, IntoCoreValue, Ss58Address};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn valid_prefixes_keep_spelling_and_encode_the_same_account() -> TestResult {
    let account = AccountId([7; 32]);
    for prefix in [0, 2, 42, 110] {
        let spelling = account.to_ss58(prefix);
        let address: Ss58Address = spelling.parse()?;
        assert_eq!(address.as_str(), spelling);
        assert_eq!(address.to_string(), spelling);
        assert_eq!(address.account(), account);
        assert_eq!(address.as_bytes(), account.as_bytes());
        assert_eq!((&address).into_core_value(), Value::Bytes(vec![7; 32]));
        assert_eq!(address.into_core_value(), account.into_core_value());
    }
    Ok(())
}

#[test]
fn invalid_input_cannot_construct_an_address_carrier() {
    let account = AccountId([7; 32]);
    let valid = account.to_ss58(0);
    let last = valid.as_bytes().last().copied();
    let mut corrupt = valid.clone();
    corrupt.pop();
    corrupt.push(if last == Some(b'1') { '2' } else { '1' });
    for input in ["", "not an address", "0", corrupt.as_str()] {
        assert!(input.parse::<Ss58Address>().is_err());
        assert!(Ss58Address::try_from(input.to_owned()).is_err());
    }
    assert!(format!(" {valid}").parse::<Ss58Address>().is_err());
    assert!(format!("{valid} ").parse::<Ss58Address>().is_err());
    // Python uses this same core codec and rejects unregistered custom formats.
    assert!(account.to_ss58(1000).parse::<Ss58Address>().is_err());
}

#[test]
fn spelling_identity_and_chain_identity_are_explicit() -> TestResult {
    let account = AccountId([7; 32]);
    let polkadot: Ss58Address = account.to_ss58(0).parse()?;
    let substrate: Ss58Address = account.to_ss58(42).parse()?;
    assert_ne!(polkadot, substrate);
    assert_eq!(polkadot.account(), substrate.account());
    assert_eq!(Ss58Address::from(account), substrate);
    assert_eq!(account.to_string(), substrate.to_string());
    Ok(())
}

#[cfg(feature = "serde")]
#[test]
fn json_roundtrips_the_validated_original_spelling() -> TestResult {
    let account = AccountId([7; 32]);
    for prefix in [0, 2, 110] {
        let spelling = account.to_ss58(prefix);
        let value = serde_json::Value::String(spelling.clone());
        let address: Ss58Address = serde_json::from_value(value.clone())?;
        assert_eq!(serde_json::to_value(&address)?, value);
        assert_eq!(address.account(), account);
        assert_eq!(
            serde_json::to_value(account)?,
            serde_json::json!(account.to_ss58(42))
        );
    }
    assert!(serde_json::from_value::<Ss58Address>(serde_json::json!("invalid")).is_err());
    assert!(serde_json::from_value::<Ss58Address>(serde_json::json!(vec![7; 32])).is_err());
    assert!(serde_json::from_value::<Ss58Address>(serde_json::Value::Null).is_err());
    Ok(())
}

#[cfg(feature = "schemars")]
#[test]
fn input_address_schema_is_a_string() -> TestResult {
    let schema = serde_json::to_value(schemars::schema_for!(Ss58Address))?;
    assert_eq!(schema["type"], serde_json::json!("string"));
    Ok(())
}
