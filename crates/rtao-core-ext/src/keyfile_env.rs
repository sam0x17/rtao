// Derived from RaoFoundation/subtensor sdk/bittensor-core/src/keyfiles/mod.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! UPSTREAM-TARGET: keyfile_env::encode_password_for_environment (PR-U10)
//! UPSTREAM-PROBE: symbol keyfile_env::encode_password_for_environment
//! UPSTREAM-FEATURES: none
//!
//! Pure environment password compatibility, without changing process variables.

use base64::{Engine as _, engine::general_purpose};
use bittensor_core::CoreError;
use zeroize::Zeroizing;

fn xor(data: &[u8], name: &str) -> Result<Zeroizing<Vec<u8>>, CoreError> {
    if name.is_empty() {
        return Err(CoreError::Crypto("env var name must not be empty".into()));
    }
    Ok(Zeroizing::new(
        data.iter()
            .zip(name.bytes().cycle())
            .map(|(byte, key)| byte ^ key)
            .collect(),
    ))
}

/// Return the pinned core's XOR/base64 value without calling `set_var`.
pub fn encode_password_for_environment(name: &str, password: &str) -> Result<String, CoreError> {
    Ok(general_purpose::STANDARD.encode(xor(password.as_bytes(), name)?.as_slice()))
}

/// Decode a caller-supplied environment value without consulting process state.
pub fn decode_password_from_environment(name: &str, value: &str) -> Result<String, CoreError> {
    if name.is_empty() {
        return Err(CoreError::Crypto("env var name must not be empty".into()));
    }
    let value = value.trim();
    // Guard partial output before decoding. The same conservative bound used
    // by STANDARD prevents Vec growth from leaving a previous allocation.
    let mut bytes = Zeroizing::new(Vec::with_capacity(base64::decoded_len_estimate(
        value.len(),
    )));
    general_purpose::STANDARD
        .decode_vec(value, &mut bytes)
        .map_err(|_| CoreError::Keyfile("invalid base64 in wallet password env var".into()))?;
    let decoded = xor(&bytes, name)?;
    std::str::from_utf8(&decoded)
        .map(str::to_owned)
        .map_err(|_| CoreError::Keyfile("invalid wallet password env var: corrupt UTF-8".into()))
}
