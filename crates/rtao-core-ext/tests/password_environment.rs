//! Guarded password decoding retains the upstream byte and error contract.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use bittensor_core::CoreError;
use rtao_core_ext::keyfile_env::{
    decode_password_from_environment, encode_password_for_environment,
};

#[test]
fn legacy_vectors_and_decoder_error_boundaries_are_preserved() -> Result<(), CoreError> {
    for (name, password, expected) in [
        ("BT_PW_TEST", "native-test", "LDUrOSE6eTE2JzY="),
        ("WALLET_α", "pässwörd\0\n", "J4LoPzYjnHjDM0FG"),
        ("A", "", ""),
    ] {
        assert_eq!(encode_password_for_environment(name, password)?, expected);
        assert_eq!(decode_password_from_environment(name, expected)?, password);
        assert_eq!(
            decode_password_from_environment(name, &format!(" \n{expected}\t"))?,
            password
        );
    }
    assert!(matches!(decode_password_from_environment("", ""),
        Err(CoreError::Crypto(message)) if message == "env var name must not be empty"));
    assert!(matches!(decode_password_from_environment("A", "!!!"),
        Err(CoreError::Keyfile(message)) if message == "invalid base64 in wallet password env var"));
    assert!(matches!(decode_password_from_environment("A", "vg=="),
        Err(CoreError::Keyfile(message)) if message == "invalid wallet password env var: corrupt UTF-8"));
    Ok(())
}

#[test]
fn decoder_rejects_malformed_suffix_after_writing_valid_blocks() -> Result<(), CoreError> {
    let name = "BT_PW_TEST";
    let password = "password long enough to cover several valid decoding blocks";
    let encoded = encode_password_for_environment(name, password)?;
    let malformed = format!("{encoded}!!!!");
    let mut partial = zeroize::Zeroizing::new(Vec::with_capacity(base64::decoded_len_estimate(
        malformed.len(),
    )));
    let pointer = partial.as_ptr();
    let capacity = partial.capacity();
    assert!(STANDARD.decode_vec(&malformed, &mut partial).is_err());
    assert_eq!(partial.as_ptr(), pointer);
    assert_eq!(partial.capacity(), capacity);
    // This late failure is different from rejecting an invalid first byte.
    assert!(partial.iter().any(|byte| *byte != 0));
    assert!(matches!(decode_password_from_environment(name, &malformed),
        Err(CoreError::Keyfile(message)) if message == "invalid base64 in wallet password env var"));
    assert_eq!(decode_password_from_environment(name, &encoded)?, password);
    Ok(())
}
