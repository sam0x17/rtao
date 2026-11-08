// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/base.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Python's read coercion: every address parameter (`*_ss58`, `*_ss58s`)
//! accepts a receiving address and reads its account
//! (`receiving.account_for_read`); SS58 values pass unchanged.

use serde_json::{Map, Value};

use crate::{ReadError, ReadName};

fn address_param(name: &str) -> bool {
    name.ends_with("_ss58") || name.ends_with("_ss58s")
}

fn resolve(read: ReadName, param: &str, value: &mut Value) -> Result<(), ReadError> {
    match value {
        Value::String(text) => {
            *text = rtao_primitives::receiving::account_for_read(text).map_err(|error| {
                ReadError::InvalidParam {
                    name: read.as_str().into(),
                    reason: format!("{param}: {error}"),
                }
            })?;
        }
        Value::Array(items) => {
            for item in items {
                resolve(read, param, item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Resolve receiving addresses in a read's address parameters to accounts.
pub fn coerce_read_params(
    read: ReadName,
    mut params: Map<String, Value>,
) -> Result<Map<String, Value>, ReadError> {
    for (name, value) in params.iter_mut() {
        if address_param(name) {
            resolve(read, name, value)?;
        }
    }
    Ok(params)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use serde_json::json;

    #[test]
    fn address_parameters_read_the_receiving_address_account() {
        let key =
            rtao_core::keys::Keypair::from_seed(&[3; 32], rtao_core::keys::CRYPTO_HASHED).unwrap();
        let address = key.hashed_receiving_address().unwrap();
        let alice = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
        let params = json!({
            "coldkey_ss58": address,
            "hotkey_ss58s": [address, alice],
            "netuid": 1,
            "name": "tao_not_an_address_parameter",
        });
        let Value::Object(params) = params else {
            unreachable!()
        };
        let coerced = coerce_read_params(ReadName::SubnetState, params).unwrap();
        assert_eq!(coerced["coldkey_ss58"], json!(key.ss58_address()));
        assert_eq!(coerced["hotkey_ss58s"], json!([key.ss58_address(), alice]));
        assert_eq!(coerced["name"], json!("tao_not_an_address_parameter"));
        let Value::Object(bad) = json!({"coldkey_ss58": "tao_malformed"}) else {
            unreachable!()
        };
        assert!(matches!(
            coerce_read_params(ReadName::SubnetState, bad),
            Err(ReadError::InvalidParam { .. })
        ));
    }
}
