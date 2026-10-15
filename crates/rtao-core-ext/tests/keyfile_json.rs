//! Secret redaction and valid plaintext keyfile compatibility.

use bittensor_core::{
    CoreError,
    keys::{CRYPTO_ED25519, CRYPTO_SR25519, Keypair},
};
use rtao_core_ext::keyfile_json::{
    deserialize_keypair_from_keyfile_data, serialized_keypair_to_keyfile_data,
};
use serde_json::json;
use std::error::Error;
use zeroize::Zeroizing;

const PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

fn equivalent_keys(left: &Keypair, right: &Keypair) -> Result<(), CoreError> {
    assert_eq!(left.public_key_bytes(), right.public_key_bytes());
    assert_eq!(left.ss58_address(), right.ss58_address());
    assert_eq!(left.crypto_type(), right.crypto_type());
    assert_eq!(left.seed_bytes(), right.seed_bytes());
    assert_eq!(left.mnemonic(), right.mnemonic());
    let signature = left.sign(b"plaintext keyfile compatibility")?;
    assert!(right.verify(b"plaintext keyfile compatibility", &signature)?);
    Ok(())
}

#[test]
fn valid_secret_constructors_roundtrip_with_retained_fields() -> Result<(), Box<dyn Error>> {
    for crypto in [CRYPTO_SR25519, CRYPTO_ED25519] {
        let seeded = Keypair::from_seed(&[7; 32], crypto)?;
        let private = Zeroizing::new(seeded.private_key_bytes().ok_or("private key missing")?);
        let private_hex = Zeroizing::new(hex::encode(&*private));
        for original in [
            seeded,
            Keypair::from_mnemonic(PHRASE, crypto, None)?,
            Keypair::from_mnemonic(PHRASE, crypto, Some("disposable derivation password"))?,
            Keypair::from_private_key(&private_hex, crypto)?,
            Keypair::from_uri("//Alice", crypto)?,
        ] {
            let bytes = Zeroizing::new(serialized_keypair_to_keyfile_data(&original)?);
            let decoded = deserialize_keypair_from_keyfile_data(&bytes)?;
            equivalent_keys(&original, &decoded)?;
        }
    }
    Ok(())
}

#[test]
fn serialization_preserves_btcli_field_values_and_public_only_shape() -> Result<(), Box<dyn Error>>
{
    for crypto in [CRYPTO_SR25519, CRYPTO_ED25519] {
        let original = Keypair::from_mnemonic(PHRASE, crypto, None)?;
        let bytes = Zeroizing::new(serialized_keypair_to_keyfile_data(&original)?);
        let data: serde_json::Value = serde_json::from_slice(&bytes)?;
        let public = format!("0x{}", hex::encode(original.public_key_bytes()));
        let private = Zeroizing::new(original.private_key_bytes().ok_or("private key missing")?);
        assert_eq!(data["accountId"], json!(public));
        assert_eq!(data["publicKey"], json!(public));
        assert_eq!(data["ss58Address"], json!(original.ss58_address()));
        assert_eq!(data["cryptoType"], json!(crypto));
        assert_eq!(data["secretPhrase"], json!(PHRASE));
        assert_eq!(
            data["secretSeed"],
            json!(format!(
                "0x{}",
                hex::encode(original.seed_bytes().ok_or("seed missing")?)
            ))
        );
        assert_eq!(
            data["privateKey"],
            json!(format!("0x{}", hex::encode(&*private)))
        );
        assert_eq!(data.as_object().ok_or("JSON is not an object")?.len(), 7);

        let public_only = Keypair::new(None, Some(&original.public_key_bytes()), crypto, 42)?;
        let data: serde_json::Value =
            serde_json::from_slice(&serialized_keypair_to_keyfile_data(&public_only)?)?;
        assert_eq!(
            data,
            json!({
                "accountId": public,
                "publicKey": public,
                "ss58Address": original.ss58_address(),
                "cryptoType": crypto,
            })
        );
        let decoded = deserialize_keypair_from_keyfile_data(&serde_json::to_vec(&data)?)?;
        assert_eq!(decoded.public_key_bytes(), original.public_key_bytes());
        assert!(decoded.sign(b"public key cannot sign").is_err());
    }
    Ok(())
}

#[test]
fn phrase_seed_and_private_precedence_with_crypto_retry_match_pinned_behavior()
-> Result<(), Box<dyn Error>> {
    for crypto in [CRYPTO_SR25519, CRYPTO_ED25519] {
        let expected = Keypair::from_mnemonic(PHRASE, crypto, None)?;
        let mut data: serde_json::Value =
            serde_json::from_slice(&serialized_keypair_to_keyfile_data(&expected)?)?;
        data["cryptoType"] = json!(1 - crypto);
        for field in ["secretPhrase", "secretSeed", "privateKey"] {
            let decoded = deserialize_keypair_from_keyfile_data(&serde_json::to_vec(&data)?)?;
            assert_eq!(decoded.public_key_bytes(), expected.public_key_bytes());
            assert_eq!(decoded.crypto_type(), expected.crypto_type());
            data.as_object_mut()
                .ok_or("JSON is not an object")?
                .remove(field);
        }

        let mut data = json!({
            "secretPhrase": PHRASE,
            "secretSeed": "invalid hex that must not win",
            "cryptoType": format!(" {crypto} "),
            " ss58Address": expected.ss58_address(),
        });
        let decoded = deserialize_keypair_from_keyfile_data(&serde_json::to_vec(&data)?)?;
        equivalent_keys(&expected, &decoded)?;
        data["secretPhrase"] = json!("INVALID_SECRET_PHRASE");
        let error = deserialize_keypair_from_keyfile_data(&serde_json::to_vec(&data)?)
            .err()
            .ok_or("invalid phrase was accepted")?;
        assert_eq!(
            error.to_string(),
            "crypto error: could not derive keypair from mnemonic"
        );
    }
    Ok(())
}

#[test]
fn historical_raw_secret_forms_remain_valid() -> Result<(), Box<dyn Error>> {
    let expected = Keypair::from_seed(&[7; 32], CRYPTO_SR25519)?;
    let seed = Zeroizing::new(hex::encode([7; 32]));
    let raw = Zeroizing::new(format!("  0x{}\n", seed.as_str()));
    equivalent_keys(
        &expected,
        &deserialize_keypair_from_keyfile_data(raw.as_bytes())?,
    )?;
    let repeated_prefix = Zeroizing::new(format!("0x0x{}", seed.as_str()));
    let bytes = Zeroizing::new(serde_json::to_vec(
        &json!({"secretSeed": *repeated_prefix}),
    )?);
    equivalent_keys(&expected, &deserialize_keypair_from_keyfile_data(&bytes)?)?;

    let private = Zeroizing::new(expected.private_key_bytes().ok_or("private key missing")?);
    let private_hex = Zeroizing::new(format!("0x{}", hex::encode(&*private)));
    let decoded = deserialize_keypair_from_keyfile_data(private_hex.as_bytes())?;
    assert_eq!(decoded.public_key_bytes(), expected.public_key_bytes());
    assert!(expected.verify(
        b"historical private key",
        &decoded.sign(b"historical private key")?
    )?);

    let expected = Keypair::from_mnemonic(PHRASE, CRYPTO_SR25519, None)?;
    equivalent_keys(
        &expected,
        &deserialize_keypair_from_keyfile_data(PHRASE.as_bytes())?,
    )?;
    Ok(())
}

#[test]
fn malformed_secret_inputs_return_fixed_diagnostics() -> Result<(), Box<dyn Error>> {
    for (input, diagnostic) in [
        (
            json!({"secretPhrase": "DO_NOT_EXPOSE_PHRASE"}),
            "crypto error: could not derive keypair from mnemonic",
        ),
        (
            json!({"secretSeed": "00G0"}),
            "keyfile error: invalid secret seed hex string",
        ),
        (
            json!({"secretSeed": "000"}),
            "keyfile error: invalid secret seed hex string",
        ),
        (
            json!({"secretSeed": "00"}),
            "crypto error: could not derive keypair from seed",
        ),
        (
            json!({"secretSeed": "00", "cryptoType": 99}),
            "crypto error: could not derive keypair from seed",
        ),
        (
            json!({"privateKey": "00Q0"}),
            "crypto error: could not derive keypair from private key",
        ),
        (
            json!({"privateKey": "00"}),
            "crypto error: could not derive keypair from private key",
        ),
    ] {
        let bytes = Zeroizing::new(serde_json::to_vec(&input)?);
        let error = deserialize_keypair_from_keyfile_data(&bytes)
            .err()
            .ok_or("invalid secret was accepted")?;
        assert_eq!(error.to_string(), diagnostic);
        assert!(error.source().is_none());
        assert!(!format!("{error:?}").contains("DO_NOT_EXPOSE"));
    }
    Ok(())
}

#[test]
fn stored_address_mismatch_keeps_the_existing_public_format_diagnostic()
-> Result<(), Box<dyn Error>> {
    let other = Keypair::from_seed(&[8; 32], CRYPTO_SR25519)?;
    let input = json!({"secretPhrase": PHRASE, "ss58Address": other.ss58_address()});
    let error = deserialize_keypair_from_keyfile_data(&serde_json::to_vec(&input)?)
        .err()
        .ok_or("mismatched stored address was accepted")?;
    assert_eq!(
        error.to_string(),
        "keyfile error: ss58Address in keyfile does not match the address derived from secretPhrase (check the keyfile's cryptoType)"
    );
    Ok(())
}
