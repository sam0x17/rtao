// Derived from RaoFoundation/subtensor sdk/python/bittensor/extension/__init__.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Authenticated loopback bridge for browser-extension signing.
//!
//! Construction is inert. Starting a server, opening a browser and spawning a
//! daemon are explicit operations. The wire protocol and embedded page match
//! the pinned Python SDK's `bittensor.extension` package.

mod client;
mod server;
mod session;
mod signer;

pub use client::{BridgeClient, ExtensionAccount};
pub use server::{BridgeOptions, BridgeServer, run_bridge};
pub use session::{
    BridgeFiles, BridgePaths, EnsureBridgeOptions, bridge_http_url, bridge_page_url, bridge_ws_url,
    ensure_bridge, run_bridge_daemon, start_bridge_daemon, stop_bridge_daemon,
};
pub use signer::{
    AccountSelectionOptions, ExtensionAccountSelection, ExtensionSigner, account_crypto_type,
    connect_extension_signer, open_extension_signer, select_extension_account,
};

use std::{fmt, sync::MutexGuard, time::Duration};

/// Python-compatible bridge bind host.
#[doc(alias = "bittensor.extension.DEFAULT_BRIDGE_HOST")]
pub const DEFAULT_BRIDGE_HOST: &str = "127.0.0.1";
/// Python-compatible bridge port.
#[doc(alias = "bittensor.extension.DEFAULT_BRIDGE_PORT")]
pub const DEFAULT_BRIDGE_PORT: u16 = 39295;

pub(crate) const MESSAGE_LIMIT: usize = 1024 * 1024;
pub(crate) const TEXT_LIMIT: usize = 4096;
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
pub(crate) const SEND_TIMEOUT: Duration = Duration::from_secs(5);

/// A bridge authentication, transport, selection or signing failure.
#[derive(Debug, Clone, PartialEq, Eq)]
#[doc(alias = "bittensor.extension.BridgeError")]
pub struct BridgeError(pub(crate) String);

impl fmt::Display for BridgeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for BridgeError {}

pub(crate) fn error(message: &str) -> BridgeError {
    BridgeError(message.to_owned())
}

pub(crate) fn lock<T>(mutex: &std::sync::Mutex<T>) -> Result<MutexGuard<'_, T>, BridgeError> {
    mutex
        .lock()
        .map_err(|_| error("bridge state lock is poisoned"))
}

pub(crate) fn random_id() -> Result<String, BridgeError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| error("OS randomness is unavailable"))?;
    Ok(hex::encode(bytes))
}

pub(crate) fn bounded_json(raw: &[u8]) -> Result<serde_json::Value, BridgeError> {
    if raw.len() > MESSAGE_LIMIT {
        return Err(error("bridge message exceeds the size limit"));
    }
    serde_json::from_slice(raw).map_err(|_| error("bridge sent invalid JSON"))
}

pub(crate) fn wire_error(value: &serde_json::Value) -> BridgeError {
    let text = value
        .get("message")
        .and_then(serde_json::Value::as_str)
        .or_else(|| value.as_str())
        .filter(|text| !text.is_empty())
        .unwrap_or("bridge request failed");
    // Wallet extensions control this text. Bound it and remove terminal escapes.
    BridgeError(
        text.chars()
            .filter(|c| !c.is_control())
            .take(TEXT_LIMIT)
            .collect(),
    )
}

pub(crate) fn bounded_value(value: &serde_json::Value) -> Result<(), BridgeError> {
    let mut remaining = MESSAGE_LIMIT;
    visit_value(value, 0, &mut remaining)
}

pub(crate) fn bounded_object(
    values: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), BridgeError> {
    let mut remaining = MESSAGE_LIMIT;
    for (key, value) in values {
        remaining = remaining
            .checked_sub(key.len())
            .ok_or_else(|| error("bridge message exceeds the size limit"))?;
        visit_value(value, 1, &mut remaining)?;
    }
    Ok(())
}

fn visit_value(
    value: &serde_json::Value,
    depth: usize,
    remaining: &mut usize,
) -> Result<(), BridgeError> {
    if depth > 64 {
        return Err(error("bridge JSON nesting exceeds its limit"));
    }
    let debit = match value {
        serde_json::Value::String(value) => value.len(),
        _ => 1,
    };
    *remaining = remaining
        .checked_sub(debit)
        .ok_or_else(|| error("bridge message exceeds the size limit"))?;
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                visit_value(value, depth + 1, remaining)?;
            }
        }
        serde_json::Value::Object(values) => {
            for (key, value) in values {
                *remaining = remaining
                    .checked_sub(key.len())
                    .ok_or_else(|| error("bridge message exceeds the size limit"))?;
                visit_value(value, depth + 1, remaining)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests;
