// Derived from RaoFoundation/subtensor sdk/bittensor-core/src/keyfiles/mod.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! UPSTREAM-TARGET: keyfiles::deserialize_keypair_from_keyfile_data (PR-U6)
//! UPSTREAM-PROBE: symbol keyfiles::deserialize_keypair_from_keyfile_data
//! UPSTREAM-FEATURES: keyfiles-json
//!
//! The pinned core's plaintext keyfile codec, split from host encryption.
//! Valid derivation and JSON ordering follow the pinned core. Owned secret
//! intermediates are wiped, and secret derivation failures use fixed diagnostics.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::collapsible_if
)]

use bittensor_core::{
    CoreError,
    keys::{CRYPTO_ED25519, CRYPTO_SR25519, Keypair},
};
use serde_json::json;
use std::{
    collections::HashMap,
    io::{self, Write},
};
use zeroize::{Zeroize, Zeroizing};

mod secret_json;
use secret_json::{Value as SecretValue, parse as parse_secret_json};

/// The secret input whose derivation failed.
#[derive(Clone, Copy, Debug)]
pub enum SecretInput {
    /// A BIP39 mnemonic, with or without a derivation password.
    Mnemonic,
    /// Raw seed bytes.
    Seed,
    /// A secret URI containing a phrase or derivation junctions.
    Uri,
    /// A hex-encoded private key.
    PrivateKey,
    /// A password-encrypted JSON keystore.
    EncryptedJson,
}

fn wipe_error(error: &mut CoreError) {
    match error {
        CoreError::Keyfile(message)
        | CoreError::WrongPassword(message)
        | CoreError::NotInRuntime(message)
        | CoreError::Codec(message)
        | CoreError::Crypto(message)
        | CoreError::Device(message)
        | CoreError::Rpc(message)
        | CoreError::Policy(message) => message.zeroize(),
    }
}

/// Wipe an upstream derivation diagnostic and replace it with a fixed message.
///
/// The original error owns its message. It is wiped before being discarded, so
/// input-dependent text cannot survive through `Display`, `Debug`, or `source`.
pub fn redacted_derivation_error(mut error: CoreError, input: SecretInput) -> CoreError {
    wipe_error(&mut error);
    CoreError::Crypto(
        match input {
            SecretInput::Mnemonic => "could not derive keypair from mnemonic",
            SecretInput::Seed => "could not derive keypair from seed",
            SecretInput::Uri => "could not derive keypair from secret URI",
            SecretInput::PrivateKey => "could not derive keypair from private key",
            SecretInput::EncryptedJson => "could not derive keypair from encrypted JSON",
        }
        .into(),
    )
}

fn wipe_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(text) => text.zeroize(),
        serde_json::Value::Array(values) => values.iter_mut().for_each(wipe_json),
        serde_json::Value::Object(values) => {
            for (mut name, mut value) in std::mem::take(values) {
                name.zeroize();
                wipe_json(&mut value);
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

struct SecretJson<'a>(SecretValue<'a>);

fn stored_crypto_type(value: &SecretValue<'_>) -> u8 {
    value
        .get("cryptoType")
        .and_then(|value| match value {
            SecretValue::Number(number) => number.parse::<u8>().ok(),
            SecretValue::String(text) => text.as_str().trim().parse::<u8>().ok(),
            _ => None,
        })
        .unwrap_or(CRYPTO_SR25519)
}

/// Inspect public keyfile fields without deriving a key or copying secret text.
///
/// The callback receives the legacy-defaulted crypto type, preferred stored
/// address, and preferred public-key hex, in that order. An existing field of
/// the wrong type still takes precedence over its alias, as in the core's JSON
/// lookup. References remain valid only during the callback. Parsing validates
/// the entire JSON document, caps it at 16 MiB and keeps decoded strings under
/// wiping ownership on success, duplicate replacement and error paths.
///
/// This accepts plaintext JSON only; it never decrypts or calls a secret-key
/// constructor. Borrowed input and any copies made by the callback remain the
/// caller's responsibility.
pub fn with_public_keyfile_fields<T>(
    keyfile_data: &[u8],
    inspect: impl FnOnce(u8, Option<&str>, Option<&str>) -> T,
) -> Result<T, CoreError> {
    let input = std::str::from_utf8(keyfile_data).map_err(|_| key_err("invalid keyfile JSON"))?;
    let value =
        parse_secret_json(input, SecretJson).map_err(|_| key_err("invalid keyfile JSON"))?;
    let address = stored_ss58(&value.0);
    let public = value
        .0
        .get("publicKey")
        .or_else(|| value.0.get("accountId"))
        .and_then(SecretValue::as_str);
    Ok(inspect(stored_crypto_type(&value.0), address, public))
}

struct SecretMap(HashMap<&'static str, serde_json::Value>);

impl Drop for SecretMap {
    fn drop(&mut self) {
        self.0.values_mut().for_each(wipe_json);
    }
}

fn secret_hex_value(secret: &[u8]) -> serde_json::Value {
    let encoded = Zeroizing::new(hex::encode(secret));
    let mut prefixed = Zeroizing::new(format!("0x{}", encoded.as_str()));
    serde_json::Value::String(std::mem::take(&mut *prefixed))
}

fn decode_secret_seed(seed: &str) -> Result<Zeroizing<Vec<u8>>, CoreError> {
    let mut bytes = Zeroizing::new(vec![0; seed.len() / 2]);
    hex::decode_to_slice(seed, &mut bytes)
        .map_err(|_| key_err("invalid secret seed hex string"))?;
    Ok(bytes)
}

struct JsonSize(usize);

impl Write for JsonSize {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("keyfile JSON is too large"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn serialize_secret_map(data: &SecretMap) -> Result<Vec<u8>, CoreError> {
    // Count without retaining bytes, then allocate once. A growing Vec could
    // release earlier secret-bearing allocations before its guard can wipe them.
    let mut size = JsonSize(0);
    serde_json::to_writer(&mut size, &data.0)
        .map_err(|error| key_err(format!("serialization error: {error}")))?;
    let mut bytes = Zeroizing::new(Vec::with_capacity(size.0));
    serde_json::to_writer(&mut *bytes, &data.0)
        .map_err(|error| key_err(format!("serialization error: {error}")))?;
    Ok(std::mem::take(&mut *bytes))
}

fn key_err(msg: impl Into<String>) -> CoreError {
    CoreError::Keyfile(msg.into())
}

/// Detect the pinned upstream encryption prefix without deriving a key.
pub fn keyfile_data_is_encrypted_nacl(keyfile_data: &[u8]) -> bool {
    keyfile_data.starts_with(b"$NACL")
}

/// Detect the pinned upstream encryption prefix without deriving a key.
pub fn keyfile_data_is_encrypted_ansible(keyfile_data: &[u8]) -> bool {
    keyfile_data.starts_with(b"$ANSIBLE_VAULT")
}

/// Detect the pinned upstream encryption prefix without deriving a key.
pub fn keyfile_data_is_encrypted_legacy(keyfile_data: &[u8]) -> bool {
    keyfile_data.starts_with(b"gAAAAA")
}

/// Detect the pinned upstream encryption prefix without deriving a key.
pub fn keyfile_data_is_encrypted(keyfile_data: &[u8]) -> bool {
    keyfile_data_is_encrypted_nacl(keyfile_data)
        || keyfile_data_is_encrypted_ansible(keyfile_data)
        || keyfile_data_is_encrypted_legacy(keyfile_data)
}

/// Detect the pinned upstream encryption prefix without deriving a key.
pub fn keyfile_data_encryption_method(keyfile_data: &[u8]) -> &'static str {
    if keyfile_data_is_encrypted_nacl(keyfile_data) {
        "NaCl"
    } else if keyfile_data_is_encrypted_ansible(keyfile_data) {
        "Ansible Vault"
    } else if keyfile_data_is_encrypted_legacy(keyfile_data) {
        "legacy"
    } else {
        "unknown"
    }
}

/// Serialize the upstream btcli keyfile JSON, including retained secrets.
///
/// Owned JSON strings and extracted private bytes are wiped before return. The
/// returned plaintext and the borrowed keypair remain their owners' responsibility.
pub fn serialized_keypair_to_keyfile_data(keypair: &Keypair) -> Result<Vec<u8>, CoreError> {
    let mut data = SecretMap(HashMap::new());

    let public_key = keypair.public_key_bytes();
    let public_key_str = hex::encode(public_key);
    data.0
        .insert("accountId", json!(format!("0x{public_key_str}")));
    data.0
        .insert("publicKey", json!(format!("0x{public_key_str}")));

    // Legacy btwallet keyfiles always carried secretPhrase/secretSeed, and
    // third-party parsers (subnet tooling, struct-based Rust readers) can
    // require them. Write them whenever the keypair retained them so files
    // created here stay parseable by legacy readers.
    if let Some(mnemonic) = keypair.mnemonic() {
        data.0.insert("secretPhrase", json!(mnemonic));
    }
    if let Some(seed) = keypair.seed_bytes() {
        data.0.insert("secretSeed", secret_hex_value(seed));
    }

    if let Some(private_key) = keypair.private_key_bytes() {
        let private_key = Zeroizing::new(private_key);
        data.0.insert("privateKey", secret_hex_value(&private_key));
    }

    data.0.insert("ss58Address", json!(keypair.ss58_address()));
    data.0.insert("cryptoType", json!(keypair.crypto_type()));

    serialize_secret_map(&data)
}

/// Stored ss58Address, including the legacy leading-space `" ss58Address"`
/// key some old btwallet files carry.
fn stored_ss58<'a>(keyfile_dict: &'a SecretValue<'_>) -> Option<&'a str> {
    keyfile_dict
        .get("ss58Address")
        .or_else(|| keyfile_dict.get(" ss58Address"))
        .and_then(|value| value.as_str())
}

/// Derive a keypair and cross-check it against the keyfile's stored
/// ss58Address. Legacy keyfiles sometimes omit or mislabel cryptoType, so on
/// a mismatch the other crypto type is tried before giving up: the stored
/// address is the ground truth for which key the file holds.
fn resolve_checked<F>(
    keyfile_dict: &SecretValue<'_>,
    crypto_type: u8,
    derived_from: &'static str,
    input: SecretInput,
    derive: F,
) -> Result<Keypair, CoreError>
where
    F: Fn(u8) -> Result<Keypair, CoreError>,
{
    let keypair = derive(crypto_type).map_err(|error| redacted_derivation_error(error, input))?;
    let Some(stored) = stored_ss58(keyfile_dict) else {
        return Ok(keypair);
    };
    if keypair.ss58_address() == stored {
        return Ok(keypair);
    }
    let alternate = if crypto_type == CRYPTO_SR25519 {
        CRYPTO_ED25519
    } else {
        CRYPTO_SR25519
    };
    if let Ok(alternate_keypair) =
        derive(alternate).map_err(|error| redacted_derivation_error(error, input))
    {
        if alternate_keypair.ss58_address() == stored {
            return Ok(alternate_keypair);
        }
    }
    Err(key_err(format!(
        "ss58Address in keyfile does not match the address derived from {derived_from} \
         (check the keyfile's cryptoType)",
    )))
}

/// Whether raw (non-JSON) keyfile content looks like a bare BIP39 phrase, as
/// written by pre-JSON-era bittensor wallets.
fn looks_like_mnemonic(text: &str) -> bool {
    let words: Vec<&str> = text.split_whitespace().collect();
    matches!(words.len(), 12 | 15 | 18 | 21 | 24)
        && words
            .iter()
            .all(|word| word.chars().all(|c| c.is_ascii_lowercase()))
}

/// Fallback for raw (non-JSON) keyfile payloads: a bare hex seed/private key
/// or a bare mnemonic, as written by pre-JSON-era bittensor wallets.
fn keypair_from_raw_text(text: &str) -> Option<Keypair> {
    let trimmed = text.trim();
    let hex_body = trimmed.strip_prefix("0x").unwrap_or(trimmed);
    if matches!(hex_body.len(), 64 | 128) && hex_body.chars().all(|c| c.is_ascii_hexdigit()) {
        if hex_body.len() == 64 {
            return decode_secret_seed(hex_body).ok().and_then(|bytes| {
                Keypair::from_seed(&bytes, CRYPTO_SR25519)
                    .map_err(|error| redacted_derivation_error(error, SecretInput::Seed))
                    .ok()
            });
        }
        return Keypair::from_private_key(trimmed, CRYPTO_SR25519)
            .map_err(|error| redacted_derivation_error(error, SecretInput::PrivateKey))
            .ok();
    }
    if looks_like_mnemonic(trimmed) {
        if let Ok(keypair) = Keypair::from_mnemonic(trimmed, CRYPTO_SR25519, None)
            .map_err(|error| redacted_derivation_error(error, SecretInput::Mnemonic))
        {
            return Some(keypair);
        }
    }
    None
}

/// Decode the pinned upstream plaintext keyfile format and historical forms.
///
/// Unescaped JSON strings and numbers borrow the input. Decoded escaped strings
/// and seed bytes are wiped on return, replacement and error paths. The parser
/// allocates no private serde_json scratch or decimal buffers. Borrowed input
/// and the returned upstream keypair remain their owners' responsibility.
pub fn deserialize_keypair_from_keyfile_data(keyfile_data: &[u8]) -> Result<Keypair, CoreError> {
    if keyfile_data.len() > secret_json::MAX_INPUT_BYTES {
        return Err(key_err("keyfile exceeds 16 MiB limit"));
    }
    let decoded = std::str::from_utf8(keyfile_data).map_err(|_| {
        if keyfile_data_is_encrypted(keyfile_data) {
            key_err("keyfile is encrypted; decrypt it with its password first")
        } else {
            key_err("failed to decode keyfile data: not utf-8 text (unknown or corrupt format)")
        }
    })?;

    let keyfile_dict = match parse_secret_json(decoded, SecretJson) {
        Ok(value) => value,
        Err(_) => {
            if let Some(keypair) = keypair_from_raw_text(decoded) {
                return Ok(keypair);
            }
            return Err(key_err(
                "failed to parse keyfile data: not keyfile JSON, a raw hex seed, or a mnemonic",
            ));
        }
    };

    // A polkadot.js / mobile-app keystore export is valid JSON but a wholly
    // different (password-encrypted) format; name it instead of failing with
    // a generic parse error.
    if keyfile_dict.0.get("encoded").is_some() && keyfile_dict.0.get("encoding").is_some() {
        return Err(key_err(
            "this keyfile is a polkadot.js / mobile-app JSON export, not a btcli keyfile; \
             import it with `btcli wallet regen-coldkey --json-path <file>`",
        ));
    }

    // Historical writers disagree on the cryptoType JSON type: python btwallet
    // wrote a number, some JS tooling wrote a numeric string.
    let crypto_type = stored_crypto_type(&keyfile_dict.0);

    if let Some(secret_phrase) = keyfile_dict
        .0
        .get("secretPhrase")
        .and_then(|value| value.as_str())
    {
        return resolve_checked(
            &keyfile_dict.0,
            crypto_type,
            "secretPhrase",
            SecretInput::Mnemonic,
            |ct| Keypair::from_mnemonic(secret_phrase, ct, None),
        );
    }

    if let Some(seed) = keyfile_dict
        .0
        .get("secretSeed")
        .and_then(|value| value.as_str())
    {
        let seed = seed.trim_start_matches("0x");
        let seed_bytes = decode_secret_seed(seed)?;
        return resolve_checked(
            &keyfile_dict.0,
            crypto_type,
            "secretSeed",
            SecretInput::Seed,
            |ct| Keypair::from_seed(&seed_bytes, ct),
        );
    }

    if let Some(private_key) = keyfile_dict
        .0
        .get("privateKey")
        .and_then(|value| value.as_str())
    {
        return resolve_checked(
            &keyfile_dict.0,
            crypto_type,
            "privateKey",
            SecretInput::PrivateKey,
            |ct| Keypair::from_private_key(private_key, ct),
        );
    }

    if let Some(ss58) = keyfile_dict
        .0
        .get("ss58Address")
        .and_then(|value| value.as_str())
    {
        return Keypair::new(Some(ss58), None, crypto_type, 42);
    }

    let found_fields = keyfile_dict
        .0
        .as_object()
        .map(|object| {
            let mut unknown = false;
            // Only these fixed, non-secret labels enter serde_json. Its map
            // preserves the previous diagnostic ordering in graphs with and
            // without the dependency's preserve_order feature.
            let mut known = serde_json::Map::new();
            for name in object.keys() {
                match name.as_str() {
                    "accountId" | "publicKey" | "cryptoType" | "secretPhrase" | "secretSeed"
                    | "privateKey" | "ss58Address" | " ss58Address" => {
                        known.insert(name.as_str().to_owned(), serde_json::Value::Null);
                    }
                    _ => {
                        unknown = true;
                    }
                }
            }
            let mut names = known.keys().map(String::as_str).collect::<Vec<_>>();
            if unknown {
                names.push("[redacted unknown fields]");
            }
            names.join(", ")
        })
        .unwrap_or_else(|| "none".to_string());
    Err(key_err(format!(
        "keypair could not be created from keyfile data: no secretPhrase, secretSeed, \
         privateKey, or ss58Address field (found: {found_fields})",
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn completed_secret_tree_guard_drops_when_trailing_validation_fails()
    -> Result<(), Box<dyn Error>> {
        use std::cell::Cell;
        struct TrackedJson<'input, 'track> {
            _json: SecretJson<'input>,
            dropped: &'track Cell<bool>,
        }
        impl Drop for TrackedJson<'_, '_> {
            fn drop(&mut self) {
                self.dropped.set(true);
            }
        }
        let dropped = Cell::new(false);
        let input = r#"{"secretPhrase":"public\u0020disposable\nsecret"} trailing"#;
        let error = parse_secret_json(input, |value| TrackedJson {
            _json: SecretJson(value),
            dropped: &dropped,
        });
        assert!(error.is_err());
        assert!(dropped.get());
        let value = parse_secret_json(
            r#"{"secretPhrase":"public\u0020disposable\nsecret"}"#,
            SecretJson,
        )?;
        assert_eq!(
            value.0.get("secretPhrase").and_then(SecretValue::as_str),
            Some("public disposable\nsecret")
        );
        Ok(())
    }

    #[test]
    fn escaped_duplicate_recovery_fields_keep_precedence_and_crypto_retry()
    -> Result<(), Box<dyn Error>> {
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        for crypto in [CRYPTO_ED25519, CRYPTO_SR25519] {
            let expected = Keypair::from_mnemonic(phrase, crypto, None)?;
            let input = format!(
                r#"{{"secretPhrase":"invalid disposable first phrase","secret\u0050hrase":"{}","secretSeed":"not used","privateKey":"not used","cryptoType":{},"ss58Address":"{}","unknown":{{"secret":"disposable\u0020unknown","number":123456789012345678901234567890}}}}"#,
                phrase.replace(' ', "\\u0020"),
                1 - crypto,
                expected.ss58_address(),
            );
            let (parsed, audit) = secret_json::audit_drops(|| {
                deserialize_keypair_from_keyfile_data(input.as_bytes())
            });
            let parsed = parsed?;
            assert_eq!(parsed.crypto_type(), crypto);
            assert_eq!(parsed.public_key_bytes(), expected.public_key_bytes());
            assert_eq!(parsed.mnemonic(), Some(phrase));
            assert!(expected.verify(
                b"keyfile escaped fixture",
                &parsed.sign(b"keyfile escaped fixture")?
            )?);
            assert!(audit.drops >= 3 && audit.wiped && audit.stable);
        }
        // A duplicate of the wrong type must replace the earlier string too.
        let input = r#"{"secretPhrase":"invalid\u0020unused","secretPhrase":null,"secretSeed":"0x0707070707070707070707070707070707070707070707070707070707070707","cryptoType":" 0 "}"#;
        let actual = deserialize_keypair_from_keyfile_data(input.as_bytes())?;
        assert_eq!(
            actual.public_key_bytes(),
            Keypair::from_seed(&[7; 32], CRYPTO_ED25519)?.public_key_bytes()
        );
        Ok(())
    }

    #[test]
    fn public_field_projection_matches_serde_for_alias_types_duplicates_and_crypto_defaults()
    -> Result<(), Box<dyn Error>> {
        for input in [
            r#"{"ss58Address":"first","ss58Address":"last\u0020address","publicKey":"0x00","cryptoType":"\u00200\u0020","secretPhrase":"disposable\u0020secret"}"#,
            r#"{"ss58Address":null," ss58Address":"ignored","publicKey":false,"accountId":"ignored","cryptoType":0}"#,
            r#"{" ss58Address":"legacy","accountId":"legacy hex","cryptoType":-0}"#,
            r#"{"ss58Address":[],"publicKey":{},"cryptoType":1e+0}"#,
            r#"{"cryptoType":123456789012345678901234567890,"unknown":{"secret":"disposable\u0020secret"}}"#,
            r#"{"cryptoType":true,"ss58Address":"value"}"#,
            "null",
            "[]",
            "false",
            "0",
            "\"public disposable string\"",
        ] {
            let expected: serde_json::Value = serde_json::from_str(input)?;
            let crypto = expected
                .get("cryptoType")
                .and_then(|value| match value {
                    serde_json::Value::Number(number) => number.to_string().parse::<u8>().ok(),
                    serde_json::Value::String(text) => text.trim().parse::<u8>().ok(),
                    _ => None,
                })
                .unwrap_or(CRYPTO_SR25519);
            let address = expected
                .get("ss58Address")
                .or_else(|| expected.get(" ss58Address"))
                .and_then(serde_json::Value::as_str);
            let public = expected
                .get("publicKey")
                .or_else(|| expected.get("accountId"))
                .and_then(serde_json::Value::as_str);
            let (result, audit) = secret_json::audit_drops(|| {
                with_public_keyfile_fields(
                    input.as_bytes(),
                    |actual_crypto, actual_address, actual_public| {
                        assert_eq!(
                            (actual_crypto, actual_address, actual_public),
                            (crypto, address, public),
                            "{input}"
                        );
                    },
                )
            });
            result?;
            assert!(audit.wiped && audit.stable);
        }
        // Private serde_json map sentinels are ordinary unknown JSON keys, as
        // in Python, rather than alternate deserialization instructions.
        for input in [
            br#"{"$serde_json::private::Number":"0","ss58Address":"ordinary address"}"#.as_slice(),
            br#"{"$serde_json::private::RawValue":"{\"ss58Address\":\"injected address\"}","ss58Address":"ordinary address"}"#.as_slice(),
        ] {
            with_public_keyfile_fields(input, |crypto, address, _| {
                assert_eq!(crypto, CRYPTO_SR25519);
                assert_eq!(address, Some("ordinary address"));
            })?;
        }
        Ok(())
    }

    #[test]
    fn public_projection_wipes_on_callback_error_partial_json_and_trailing_garbage()
    -> Result<(), Box<dyn Error>> {
        let valid = br#"{"ss58Address":"public address","secretPhrase":"public\u0020disposable"}"#;
        let (callback, audit) = secret_json::audit_drops(|| {
            with_public_keyfile_fields(valid, |_, _, _| Err::<(), _>("public callback error"))
        });
        assert!(callback?.is_err());
        assert_eq!(audit.drops, 1);
        assert!(audit.wiped && audit.stable);
        for input in [
            r#"{"ss58Address":"address","secretPhrase":"public\u0020disposable","unfinished":"#,
            r#"{"ss58Address":"address","secretPhrase":"public\u0020disposable"} trailing"#,
        ] {
            let mut called = false;
            let (result, audit) = secret_json::audit_drops(|| {
                with_public_keyfile_fields(input.as_bytes(), |_, _, _| called = true)
            });
            assert!(result.is_err());
            assert!(!called);
            assert_eq!(audit.drops, 1);
            assert!(audit.wiped && audit.stable);
            let error = deserialize_keypair_from_keyfile_data(input.as_bytes())
                .err()
                .ok_or("invalid JSON accepted")?;
            assert_eq!(
                error.to_string(),
                "keyfile error: failed to parse keyfile data: not keyfile JSON, a raw hex seed, or a mnemonic"
            );
            assert!(!format!("{error:?}").contains("disposable"));
        }
        Ok(())
    }

    #[test]
    fn direct_keyfile_limit_refuses_oversized_historical_text_and_callbacks_but_accepts_boundary_json()
    -> Result<(), Box<dyn Error>> {
        let limit = secret_json::MAX_INPUT_BYTES;
        // Reuse one allocation for all three cases. The raw text would be a
        // valid historical mnemonic after trim if the cap did not run first.
        let mut input = String::with_capacity(limit + 1);
        input.push_str("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about");
        input.extend(std::iter::repeat_n(' ', limit + 1 - input.len()));
        let error = deserialize_keypair_from_keyfile_data(input.as_bytes())
            .err()
            .ok_or("oversized historical mnemonic was derived")?;
        assert_eq!(
            error.to_string(),
            "keyfile error: keyfile exceeds 16 MiB limit"
        );

        input.clear();
        input.push_str(
            r#"{"ss58Address":"public boundary address","secretPhrase":"public\u0020disposable"}"#,
        );
        input.extend(std::iter::repeat_n(' ', limit + 1 - input.len()));
        let mut called = false;
        let oversized = with_public_keyfile_fields(input.as_bytes(), |_, _, _| called = true);
        assert!(oversized.is_err());
        assert!(!called);

        assert_eq!(input.pop(), Some(' '));
        assert_eq!(input.len(), limit);
        with_public_keyfile_fields(input.as_bytes(), |crypto, address, public| {
            called = true;
            assert_eq!(crypto, CRYPTO_SR25519);
            assert_eq!(address, Some("public boundary address"));
            assert_eq!(public, None);
        })?;
        assert!(called);
        Ok(())
    }

    #[test]
    fn missing_keyfile_field_errors_redact_unknown_input_key_names() -> Result<(), Box<dyn Error>> {
        let sentinel = "rtao-public-disposable-unknown-key-secret-sentinel";
        let input =
            serde_json::to_vec(&json!({sentinel:"other disposable value", "publicKey": false}))?;
        let error = deserialize_keypair_from_keyfile_data(&input)
            .err()
            .ok_or("unexpected keyfile success")?;
        assert!(matches!(&error, CoreError::Keyfile(_)));
        for rendered in [error.to_string(), format!("{error:?}")] {
            assert!(!rendered.contains(sentinel));
            assert!(!rendered.contains("other disposable value"));
            assert!(rendered.contains("publicKey"));
            assert!(rendered.contains("[redacted unknown fields]"));
        }
        Ok(())
    }

    fn upstream_errors(message: &str) -> [CoreError; 8] {
        [
            CoreError::Keyfile(message.into()),
            CoreError::WrongPassword(message.into()),
            CoreError::NotInRuntime(message.into()),
            CoreError::Codec(message.into()),
            CoreError::Crypto(message.into()),
            CoreError::Device(message.into()),
            CoreError::Rpc(message.into()),
            CoreError::Policy(message.into()),
        ]
    }

    #[test]
    fn recursively_wipes_json_strings_and_owned_object_keys() {
        let mut value = json!([
            "phrase",
            ["seed", ["private key"], true, null, 7],
            {"private field name": {"nested": "secret value"}}
        ]);
        wipe_json(&mut value);
        assert_eq!(value, json!(["", ["", [""], true, null, 7], {}]));
        let mut scalar = json!("scalar secret");
        wipe_json(&mut scalar);
        assert_eq!(scalar, json!(""));
    }

    #[test]
    fn wipes_every_owned_core_error_message_before_discarding_it() {
        for mut error in upstream_errors("secret diagnostic with a seed or phrase") {
            wipe_error(&mut error);
            let message = match &error {
                CoreError::Keyfile(message)
                | CoreError::WrongPassword(message)
                | CoreError::NotInRuntime(message)
                | CoreError::Codec(message)
                | CoreError::Crypto(message)
                | CoreError::Device(message)
                | CoreError::Rpc(message)
                | CoreError::Policy(message) => message,
            };
            assert!(message.is_empty());
        }
    }

    #[test]
    fn redacted_errors_expose_only_the_selected_static_input_label() {
        for (input, diagnostic) in [
            (
                SecretInput::Mnemonic,
                "could not derive keypair from mnemonic",
            ),
            (SecretInput::Seed, "could not derive keypair from seed"),
            (SecretInput::Uri, "could not derive keypair from secret URI"),
            (
                SecretInput::PrivateKey,
                "could not derive keypair from private key",
            ),
            (
                SecretInput::EncryptedJson,
                "could not derive keypair from encrypted JSON",
            ),
        ] {
            for original in upstream_errors("DO_NOT_EXPOSE_THE_INPUT") {
                let error = redacted_derivation_error(original, input);
                assert!(matches!(&error, CoreError::Crypto(message) if message == diagnostic));
                assert_eq!(error.to_string(), format!("crypto error: {diagnostic}"));
                assert_eq!(format!("{error:?}"), format!("Crypto({diagnostic:?})"));
                assert!(error.source().is_none());
            }
        }
    }

    #[test]
    fn guarded_serialization_retains_hashmap_iteration_and_exact_json_bytes()
    -> Result<(), Box<dyn Error>> {
        let data = SecretMap(HashMap::from([
            (
                "secretPhrase",
                json!("a phrase with \"quotes\" and \\ escapes"),
            ),
            ("secretSeed", secret_hex_value(&[7; 32])),
            ("cryptoType", json!(CRYPTO_SR25519)),
        ]));
        let original = Zeroizing::new(serde_json::to_string(&data.0)?);
        let guarded = Zeroizing::new(serialize_secret_map(&data)?);
        assert_eq!(guarded.as_slice(), original.as_bytes());
        Ok(())
    }
}
