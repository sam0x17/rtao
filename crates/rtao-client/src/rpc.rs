//! Shared typed RPC conversion for the client and runtime cache.

use serde_json::{Value, value::RawValue};
use subxt_rpcs::RpcClient;

use crate::ClientError;

/// Issue a request through the client's supplied transport.
pub(crate) async fn rpc_request(
    rpc: &RpcClient,
    method: &str,
    params: Value,
) -> Result<Value, ClientError> {
    if !params.is_array() {
        return Err(ClientError::Protocol(
            "RPC parameters must be an array".into(),
        ));
    }
    let raw = RawValue::from_string(params.to_string())
        .map_err(|error| ClientError::Protocol(error.to_string()))?;
    let response = rpc
        .request_raw(method, Some(raw))
        .await
        .map_err(ClientError::from)?;
    serde_json::from_str(response.get()).map_err(|error| ClientError::Protocol(error.to_string()))
}

/// Decode a Substrate hexadecimal hash into the chain's owned hash type.
pub(crate) fn parse_hash(value: &Value) -> Result<rtao_primitives::H256, ClientError> {
    let text = value
        .as_str()
        .ok_or_else(|| ClientError::Protocol("RPC hash is not a string".into()))?;
    let bytes = text
        .strip_prefix("0x")
        .ok_or_else(|| ClientError::Protocol("RPC hash lacks 0x prefix".into()))?;
    let bytes = hex::decode(bytes).map_err(|error| ClientError::Protocol(error.to_string()))?;
    Ok(rtao_primitives::H256(bytes.try_into().map_err(|_| {
        ClientError::Protocol("RPC hash must contain 32 bytes".into())
    })?))
}

/// Format an owned chain hash using the Substrate hexadecimal wire format.
pub(crate) fn hash_hex(hash: rtao_primitives::H256) -> String {
    format!("0x{}", hex::encode(hash.0))
}
