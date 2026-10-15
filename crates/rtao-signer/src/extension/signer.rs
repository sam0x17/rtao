// Derived from RaoFoundation/subtensor sdk/python/bittensor/extension/signer.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/extension/picker.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use serde_json::Value;
use tokio::sync::{Mutex, watch};

use super::{BridgeClient, BridgeError, ExtensionAccount, error};
use crate::{
    AccountId, BoxFuture, CryptoType, MultiSignature, SignPurpose, Signer, SignerCaps, SignerError,
    SigningRequest,
};

/// Validate an extension's algorithm name using Python's normalization rules.
#[doc(alias = "bittensor.extension.account_crypto_type")]
pub fn account_crypto_type(account_type: &str) -> Result<CryptoType, BridgeError> {
    match account_type
        .trim_matches(|character: char| {
            character.is_whitespace() || matches!(character, '\u{1c}'..='\u{1f}')
        })
        .to_lowercase()
        .as_str()
    {
        "sr25519" => Ok(CryptoType::Sr25519),
        "ed25519" => Ok(CryptoType::Ed25519),
        "ecdsa" | "ethereum" => Err(error(
            "extension account type is not supported for Substrate signing; use sr25519 or ed25519",
        )),
        _ => Err(error(
            "unsupported extension account type; expected ed25519 or sr25519",
        )),
    }
}

/// Deterministic filters for selecting an advertised extension account.
///
/// An explicit address takes precedence over source/name filters, matching
/// Python. Interactive frontends can list accounts and pass their chosen
/// address; library calls do not read terminal input.
#[derive(Clone, Debug, Default)]
pub struct AccountSelectionOptions {
    /// Exact advertised address; wins over other filters.
    pub address: Option<String>,
    /// Exact extension source filter.
    pub source: Option<String>,
    /// Exact account name filter.
    pub name: Option<String>,
    /// Remembered address, used only when uniquely present after filtering.
    pub default_address: Option<String>,
}

/// Selected public extension identity, with no open transport or secret material.
#[derive(Clone, Debug, PartialEq, Eq)]
#[doc(alias = "bittensor.extension.ExtensionAccountSelection")]
pub struct ExtensionAccountSelection {
    /// Original advertised account, including its exact SS58 spelling.
    pub account: ExtensionAccount,
    /// Validated public key decoded by the pinned core SS58 implementation.
    pub public_key: [u8; 32],
    /// Validated Substrate signing algorithm.
    pub crypto_type: CryptoType,
    /// Python's public-key constructor SS58 display format (42).
    pub ss58_format: u16,
}

/// Select and validate a public identity using a short-lived bridge connection.
#[doc(alias = "bittensor.extension.select_extension_account")]
pub async fn select_extension_account(
    url: &str,
    token: &str,
    options: &AccountSelectionOptions,
) -> Result<ExtensionAccountSelection, BridgeError> {
    let client = BridgeClient::new(url, token)?;
    let accounts = client.list_accounts().await;
    client.close().await;
    select_from_accounts(&accounts?, options)
}
pub(crate) fn select_from_accounts(
    accounts: &[ExtensionAccount],
    options: &AccountSelectionOptions,
) -> Result<ExtensionAccountSelection, BridgeError> {
    if accounts.is_empty() {
        return Err(error(
            "no extension accounts available; authorize your browser extension and try again",
        ));
    }
    let selected = if let Some(address) = &options.address {
        accounts
            .iter()
            .find(|account| &account.address == address)
            .ok_or_else(|| {
                error(&format!(
                    "extension account {} not found",
                    rtao_primitives::pyfmt::string_repr(address)
                ))
            })?
    } else {
        let filtered: Vec<_> = accounts
            .iter()
            .filter(|account| {
                options
                    .source
                    .as_ref()
                    .is_none_or(|source| &account.source == source)
                    && options
                        .name
                        .as_ref()
                        .is_none_or(|name| &account.name == name)
            })
            .collect();
        if filtered.is_empty() {
            return Err(error(&format!(
                "no extension account matched the given filters; available: {}",
                account_hint(accounts)
            )));
        }
        if filtered.len() == 1 {
            filtered[0]
        } else if let Some(default) = &options.default_address {
            let saved: Vec<_> = filtered
                .iter()
                .copied()
                .filter(|account| &account.address == default)
                .collect();
            if saved.len() == 1 {
                saved[0]
            } else {
                return Err(error(&format!(
                    "multiple extension accounts available; pass --signer-address or run in a terminal: {}",
                    account_hint(filtered.iter().copied())
                )));
            }
        } else {
            return Err(error(&format!(
                "multiple extension accounts available; pass --signer-address or run in a terminal: {}",
                account_hint(filtered.iter().copied())
            )));
        }
    };
    let crypto_type = account_crypto_type(&selected.account_type)?;
    let public_key = rtao_core::keys::public_key_from_ss58(&selected.address)
        .map_err(|_| error("extension returned an invalid SS58 address"))?;
    Ok(ExtensionAccountSelection {
        account: selected.clone(),
        public_key,
        crypto_type,
        ss58_format: 42,
    })
}

fn account_hint<'a>(accounts: impl IntoIterator<Item = &'a ExtensionAccount>) -> String {
    accounts
        .into_iter()
        .map(|account| format!("{} ({}, {})", account.name, account.address, account.source))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Open a fresh authenticated connection for one selected signing session.
#[doc(alias = "bittensor.extension.open_extension_signer")]
pub async fn open_extension_signer(
    url: &str,
    token: &str,
    selection: ExtensionAccountSelection,
) -> Result<ExtensionSigner, BridgeError> {
    let bridge = Arc::new(BridgeClient::new(url, token)?);
    let signer = ExtensionSigner::new(selection, bridge.clone())?;
    bridge.connect().await?;
    Ok(signer)
}
/// Select an account and open a fresh bridge connection for signing.
#[doc(alias = "bittensor.extension.connect_extension_signer")]
pub async fn connect_extension_signer(
    url: &str,
    token: &str,
    options: &AccountSelectionOptions,
) -> Result<ExtensionSigner, BridgeError> {
    let selection = select_extension_account(url, token, options).await?;
    open_extension_signer(url, token, selection).await
}

/// Browser-backed signer. Private keys never enter the Rust process.
#[doc(alias = "bittensor.extension.ExtensionSigner")]
pub struct ExtensionSigner {
    selection: ExtensionAccountSelection,
    bridge: Arc<BridgeClient>,
    two_stage: AtomicBool,
    sign_index: Mutex<u64>,
    cancel: watch::Sender<u64>,
}
impl std::fmt::Debug for ExtensionSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtensionSigner")
            .field("account", &self.selection.account)
            .finish_non_exhaustive()
    }
}
impl ExtensionSigner {
    /// Attach a validated public selection to an inert or connected client.
    pub fn new(
        selection: ExtensionAccountSelection,
        bridge: Arc<BridgeClient>,
    ) -> Result<Self, BridgeError> {
        let actual = rtao_core::keys::public_key_from_ss58(&selection.account.address)
            .map_err(|_| error("extension returned an invalid SS58 address"))?;
        if actual != selection.public_key
            || account_crypto_type(&selection.account.account_type)? != selection.crypto_type
        {
            return Err(error(
                "extension account does not match its selected public identity",
            ));
        }
        Ok(Self {
            selection,
            bridge,
            two_stage: AtomicBool::new(false),
            sign_index: Mutex::new(0),
            cancel: watch::channel(0).0,
        })
    }
    /// Original extension address spelling.
    pub fn ss58_address(&self) -> &str {
        &self.selection.account.address
    }
    /// Advertised extension source.
    pub fn extension_source(&self) -> &str {
        &self.selection.account.source
    }
    /// Advertised human-readable account name.
    pub fn account_name(&self) -> &str {
        &self.selection.account.name
    }
    /// Selected public key bytes.
    pub fn public_key(&self) -> &[u8; 32] {
        &self.selection.public_key
    }
    /// Python-compatible SS58 display format.
    pub fn ss58_format(&self) -> u16 {
        self.selection.ss58_format
    }
    /// Label two successive approvals for a manually managed shield flow.
    pub async fn set_two_stage(&self, enabled: bool) {
        let mut index = self.sign_index.lock().await;
        *index = 0;
        self.two_stage.store(enabled, Ordering::Release);
    }
    /// Close the session's authenticated connection.
    pub async fn close(&self) {
        self.cancel
            .send_modify(|generation| *generation = generation.wrapping_add(1));
        self.bridge.close().await;
    }
    /// Report the final chain submission result to the browser page.
    pub async fn report_transaction_result(&self, success: bool) -> Result<(), BridgeError> {
        self.bridge.report_transaction_result(success).await
    }
    /// Sign a Polkadot-JS payload, replacing its address with the selected identity.
    pub async fn sign_extrinsic_payload(
        &self,
        payload: &serde_json::Map<String, Value>,
    ) -> Result<Value, BridgeError> {
        self.sign_payload(payload, None).await
    }
    async fn sign_payload(
        &self,
        payload: &serde_json::Map<String, Value>,
        purpose: Option<SignPurpose>,
    ) -> Result<Value, BridgeError> {
        let mut cancelled = self.cancel.subscribe();
        tokio::select! {
            result=tokio::time::timeout(super::REQUEST_TIMEOUT,self.sign_payload_inner(payload,purpose))=>result.map_err(|_|error("extension signing timed out"))?,
            _=cancelled.changed()=>Err(error("extension signing session closed")),
        }
    }
    async fn sign_payload_inner(
        &self,
        payload: &serde_json::Map<String, Value>,
        purpose: Option<SignPurpose>,
    ) -> Result<Value, BridgeError> {
        // Validate the borrowed members before cloning the caller's object.
        super::bounded_object(payload)?;
        let mut index = self.sign_index.lock().await;
        *index = index
            .checked_add(1)
            .ok_or_else(|| error("extension signing session exhausted its operation counter"))?;
        let mut body = payload.clone();
        body.insert(
            "address".into(),
            Value::String(self.ss58_address().to_owned()),
        );
        let stage = match purpose {
            Some(SignPurpose::ShieldInner) => {
                Some(("approve 1 of 2 — the shielded transaction", true))
            }
            Some(SignPurpose::ShieldCarrier) => {
                Some(("approve 2 of 2 — the encrypted carrier", false))
            }
            _ if self.two_stage.load(Ordering::Acquire) => {
                if *index == 1 {
                    Some(("approve 1 of 2 — the shielded transaction", true))
                } else {
                    Some(("approve 2 of 2 — the encrypted carrier", false))
                }
            }
            _ => None,
        };
        let result = self
            .bridge
            .sign_extrinsic_payload(
                Value::Object(body),
                stage.map(|s| s.0),
                stage.is_some_and(|s| s.1),
            )
            .await;
        result.map_err(|e| {
            let message = e.to_string().to_lowercase();
            if message.contains("cancel")
                || message.contains("reject")
                || message.contains("denied")
            {
                error("extension signing cancelled")
            } else {
                e
            }
        })
    }
}
impl Signer for ExtensionSigner {
    fn account(&self) -> AccountId {
        AccountId(self.selection.public_key)
    }
    fn crypto_type(&self) -> CryptoType {
        self.selection.crypto_type
    }
    fn caps(&self) -> SignerCaps {
        SignerCaps::PAYLOAD_JSON
    }
    fn sign<'a>(
        &'a self,
        request: &'a SigningRequest<'a>,
    ) -> BoxFuture<'a, Result<MultiSignature, SignerError>> {
        Box::pin(async move {
            let response = self
                .sign_payload(request.payload_json, Some(request.purpose))
                .await
                .map_err(signer_error)?;
            decode_signature(&response, self.crypto_type()).map_err(signer_error)
        })
    }
    fn sign_raw<'a>(&'a self, message: &'a [u8]) -> BoxFuture<'a, Result<[u8; 64], SignerError>> {
        Box::pin(async move {
            if message.len() > super::MESSAGE_LIMIT / 2 - 4096 {
                return Err(signer_error(error(
                    "raw signing message exceeds the bridge size limit",
                )));
            }
            let response = self
                .bridge
                .sign_bytes(self.ss58_address(), &format!("0x{}", hex::encode(message)))
                .await
                .map_err(signer_error)?;
            Ok(decode_signature(&response, self.crypto_type())
                .map_err(signer_error)?
                .bytes)
        })
    }
}
fn signer_error(error: BridgeError) -> SignerError {
    SignerError::Extension(error.to_string())
}
pub(crate) fn decode_signature(
    response: &Value,
    crypto: CryptoType,
) -> Result<MultiSignature, BridgeError> {
    let text = response
        .get("signature")
        .and_then(Value::as_str)
        .ok_or_else(|| error("extension did not return a signature"))?;
    let text = text.strip_prefix("0x").unwrap_or(text);
    if !matches!(text.len(), 128 | 130) {
        return Err(error("extension returned an invalid signature length"));
    }
    let bytes =
        hex::decode(text).map_err(|_| error("extension returned a non-hexadecimal signature"))?;
    if bytes.len() == 65 {
        return MultiSignature::from_bytes_for(&bytes, crypto).map_err(|_| {
            error("extension signature algorithm does not match the selected account")
        });
    }
    let bytes = <[u8; 64]>::try_from(bytes.as_slice())
        .map_err(|_| error("extension returned an invalid signature length"))?;
    Ok(MultiSignature { crypto, bytes })
}
