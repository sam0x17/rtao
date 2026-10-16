// Derived from RaoFoundation/subtensor sdk/python/bittensor/vault/signer.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use serde_json::{Value, json};
use tokio::sync::Mutex as AsyncMutex;

use super::{
    REQUEST_LIMIT, VAULT_GUIDE_URL, VaultError, VaultRequestTimeouts, VaultServerOptions,
    VaultSessionServer, VaultStatus, error, lock, svg_data_uri, uos,
};
use crate::{
    AccountId, BoxFuture, CryptoType, MultiSignature, SignPurpose, Signer, SignerCaps, SignerError,
    SigningContext, SigningRequest,
};

/// Offline identity and explicit session-opening options for a Vault signer.
#[derive(Clone)]
pub struct VaultOptions {
    /// Key scheme; Polkadot Vault defaults to sr25519.
    pub crypto_type: CryptoType,
    /// Address display prefix; chain proof context still uses the selected runtime's prefix.
    pub ss58_format: u16,
    /// Loopback listener and opt-in browser launch.
    pub server: VaultServerOptions,
    /// Page-connect and immortal-signature deadlines.
    pub timeouts: VaultRequestTimeouts,
    /// Optional progress callback.
    pub on_status: Option<VaultStatus>,
}
impl Default for VaultOptions {
    fn default() -> Self {
        Self {
            crypto_type: CryptoType::Sr25519,
            ss58_format: 42,
            server: VaultServerOptions::default(),
            timeouts: VaultRequestTimeouts::default(),
            on_status: None,
        }
    }
}
impl std::fmt::Debug for VaultOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultOptions")
            .field("crypto_type", &self.crypto_type)
            .field("ss58_format", &self.ss58_format)
            .field("server", &self.server)
            .field("timeouts", &self.timeouts)
            .finish_non_exhaustive()
    }
}
#[derive(Default)]
struct Display {
    summary: Option<String>,
    two_stage: bool,
    index: u64,
}

/// Air-gapped signer whose private key stays on the Polkadot Vault phone.
///
/// Construction validates only the public SS58 address. Signing uses an
/// RFC-0078 proof for the request's own context, displays UOS QR frames and
/// verifies the returned signature before accepting it. Raw signing is refused.
#[doc(alias = "bittensor.vault.VaultSigner")]
#[doc(alias = "bittensor.vault.signer.VaultSigner")]
pub struct VaultSigner {
    address: String,
    public_key: [u8; 32],
    options: VaultOptions,
    display: Mutex<Display>,
    operation: AsyncMutex<()>,
    server: AsyncMutex<Option<Arc<VaultSessionServer>>>,
}
impl std::fmt::Debug for VaultSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("VaultSigner").field(&self.address).finish()
    }
}
impl VaultSigner {
    /// Construct an offline sr25519 signer without binding a socket or opening a browser.
    pub fn new(address: &str) -> Result<Self, VaultError> {
        Self::with_options(address, VaultOptions::default())
    }
    /// Construct with explicit algorithm, deadlines and browser behavior.
    pub fn with_options(address: &str, options: VaultOptions) -> Result<Self, VaultError> {
        let public_key = rtao_core::keys::public_key_from_ss58(address)
            .map_err(|_| error("invalid vault signer SS58 address"))?;
        if !options.server.address.ip().is_loopback() {
            return Err(error("Vault page server must bind a loopback address"));
        }
        Ok(Self {
            address: address.to_owned(),
            public_key,
            options,
            display: Mutex::new(Display::default()),
            operation: AsyncMutex::new(()),
            server: AsyncMutex::new(None),
        })
    }
    /// Original public address spelling, preserving its SS58 prefix.
    pub fn ss58_address(&self) -> &str {
        &self.address
    }
    /// Public signing key decoded at construction.
    pub fn public_key(&self) -> &[u8; 32] {
        &self.public_key
    }
    /// Requested address display prefix.
    pub fn ss58_format(&self) -> u16 {
        self.options.ss58_format
    }
    /// Set the optional display-only transaction description.
    pub fn set_summary(&self, summary: Option<String>) -> Result<(), VaultError> {
        lock(&self.display)?.summary = summary;
        Ok(())
    }
    /// Label manually managed two-stage shield approvals, resetting their display index.
    ///
    /// Explicit `SigningRequest::purpose` stages take precedence over this fallback.
    pub fn set_two_stage(&self, enabled: bool) -> Result<(), VaultError> {
        let mut display = lock(&self.display)?;
        display.two_stage = enabled;
        display.index = 0;
        Ok(())
    }
    /// Start or reuse the session page without waiting for a scan.
    pub async fn session_server(&self) -> Result<Arc<VaultSessionServer>, VaultError> {
        let mut server = self.server.lock().await;
        if let Some(server) = server.as_ref()
            && server.is_running()
        {
            return Ok(server.clone());
        }
        let started = Arc::new(VaultSessionServer::new(self.options.server.clone())?);
        started.start().await?;
        *server = Some(started.clone());
        Ok(started)
    }
    /// Wait for the page and start its camera before a time-critical shield flow.
    pub async fn warm_up(&self) -> Result<(), VaultError> {
        let server = self.session_server().await?;
        self.status(&format!(
            "get ready at {} — unlock Polkadot Vault and open its scanner before continuing",
            server.http_url()
        ));
        server
            .warm_up(self.options.timeouts.connect)
            .await
            .map_err(Into::into)
    }
    /// Compute the core RFC-0078 digest using the runtime identity and fixed TAO constants.
    ///
    /// No mutable context is cached: signing always uses the request's own
    /// metadata and avoids coupling concurrent requests to an earlier digest call.
    pub async fn metadata_digest(&self, context: &SigningContext) -> Result<[u8; 32], VaultError> {
        let metadata = context.metadata_bytes.clone();
        let info = chain_info(context);
        rtao_core::offload(move || rtao_core::digest::metadata_digest(&metadata, &info))
            .await
            .map_err(|_| error("Vault metadata digest worker failed"))?
            .map_err(|_| error("cannot compute the Vault metadata digest"))
    }
    /// Sign a prepared transaction with an RFC-0078 proof and verify the scanned reply.
    pub async fn sign_unsigned_extrinsic(
        &self,
        request: &SigningRequest<'_>,
    ) -> Result<MultiSignature, VaultError> {
        let _operation = self.operation.lock().await;
        let call = request.call.to_vec();
        let extra = request.included_in_extrinsic.to_vec();
        let additional = request.included_in_signed_data.to_vec();
        let metadata = request.context.metadata_bytes.clone();
        let info = chain_info(request.context);
        let proof = rtao_core::offload(move || {
            rtao_core::digest::generate_extrinsic_proof(
                &call,
                &extra,
                &additional,
                &metadata,
                &info,
            )
        })
        .await
        .map_err(|_| error("Vault metadata proof worker failed"))?
        .map_err(|_| error("cannot generate the Vault metadata proof"))?;
        let unsigned =
            uos::Transaction::from_request(request, &self.public_key, self.options.crypto_type);
        let frames = uos::transaction_frames_with_proof(&unsigned, &proof)?;
        self.round_trip(request, frames, false).await
    }
    /// Explicit legacy portal-metadata signing for callers that have no proof flow.
    ///
    /// The trait's `sign` method always uses proof-carrying frames. This helper
    /// retains Python's legacy fallback and reports whether the QR signs a hash.
    pub async fn sign_legacy(
        &self,
        request: &SigningRequest<'_>,
    ) -> Result<MultiSignature, VaultError> {
        let _operation = self.operation.lock().await;
        let unsigned =
            uos::Transaction::from_request(request, &self.public_key, self.options.crypto_type);
        let (frames, hashed) = uos::transaction_frames(&unsigned)?;
        self.round_trip(request, frames, hashed).await
    }
    async fn round_trip(
        &self,
        request: &SigningRequest<'_>,
        frames: Vec<Vec<u8>>,
        hashed: bool,
    ) -> Result<MultiSignature, VaultError> {
        let deadline = era_seconds(request.payload_json.get("era"))?;
        let nonce = request
            .payload_json
            .get("nonce")
            .map(json_unsigned)
            .transpose()?
            .unwrap_or(0);
        let (text, stage) = {
            let mut display = lock(&self.display)?;
            display.index = display
                .index
                .checked_add(1)
                .ok_or_else(|| error("Vault signing operation counter exhausted"))?;
            let stage = match request.purpose {
                SignPurpose::ShieldInner => Some("scan 1 of 2 — the shielded transaction"),
                SignPurpose::ShieldCarrier => Some("scan 2 of 2 — the encrypted carrier"),
                _ if display.two_stage && display.index == 1 => {
                    Some("scan 1 of 2 — the shielded transaction")
                }
                _ if display.two_stage => Some("scan 2 of 2 — the encrypted carrier"),
                _ => None,
            };
            (display.summary.clone(), stage)
        };
        let summary = json!({"address":self.address,"genesisHash":format!("0x{}",hex::encode(request.context.genesis_hash)),
            "specVersion":request.context.chain_info.spec_version,"nonce":nonce,"hashed":hashed,
            "deadlineSeconds":deadline,"text":text,"stage":stage,"guideUrl":VAULT_GUIDE_URL});
        let (rendered, frames_hex) = rtao_core::offload(move || render_frames(frames))
            .await
            .map_err(|_| error("Vault QR rendering task failed"))??;
        let server = self.session_server().await?;
        self.status(&format!("scan the QR at {} with Polkadot Vault, approve, then hold the phone's signature QR up to your webcam{}", server.http_url(), stage.map(|stage|format!(" ({stage})")).unwrap_or_default()));
        if hashed {
            self.status("note: this call is too large to display — Vault will show only its hash (blind signing)");
        }
        let mut timeouts = self.options.timeouts;
        if let Some(seconds) = deadline {
            timeouts.answer = Duration::try_from_secs_f64(seconds)
                .map_err(|_| error("invalid Vault signing era deadline"))?;
        }
        let scanned = server
            .request_signature(rendered, frames_hex, summary, timeouts)
            .await?;
        self.validate_signature(&scanned, request.payload)
    }
    /// Decode and verify a raw 64-byte or versioned 65-byte signature scan.
    ///
    /// Python-compatible verification honors a version prefix and the core's
    /// `<Bytes>` fallback. A versioned scan may therefore select either supported
    /// scheme independently of the default configured for unversioned scans.
    pub fn validate_signature(
        &self,
        scanned: &str,
        payload: &[u8],
    ) -> Result<MultiSignature, VaultError> {
        let text = scanned
            .trim_matches(|ch: char| ch.is_whitespace() || matches!(ch, '\u{1c}'..='\u{1f}'));
        let text = text.strip_prefix("0x").unwrap_or(text);
        let signature = decode_python_hex(text).map_err(|_| VaultError(format!("the scanned QR is not a signature (got {} chars of non-hex data); scan the QR Vault shows *after* you approve the transaction", text.chars().count())))?;
        if !matches!(signature.length, 64 | 65) {
            return Err(VaultError(format!(
                "the scanned QR holds {} bytes, not a 64/65-byte signature",
                signature.length
            )));
        }
        let signature = &signature.bytes[..signature.length];
        let (crypto, raw) = if signature.len() == 65 {
            (
                CryptoType::try_from(signature[0]).map_err(|_| self.bad_signature())?,
                &signature[1..],
            )
        } else {
            (self.options.crypto_type, signature)
        };
        if !rtao_core::keys::verify(payload, raw, &self.address, u8::from(crypto)).unwrap_or(false)
        {
            return Err(self.bad_signature());
        }
        let bytes = raw.try_into().map_err(|_| self.bad_signature())?;
        Ok(MultiSignature { crypto, bytes })
    }
    fn bad_signature(&self) -> VaultError {
        VaultError(format!(
            "the scanned signature does not verify against this transaction for {} — was a stale QR scanned, or did a different key sign?",
            self.address
        ))
    }
    /// Flip the current page to the chain submission's result, best effort.
    pub async fn report_transaction_result(&self, success: bool) -> Result<(), VaultError> {
        let server = self.server.lock().await.clone();
        if let Some(server) = server {
            server.report_result(success).await?;
        }
        Ok(())
    }
    /// Stop the owned session, including pending scans, without leaving a daemon.
    pub async fn close(&self) -> Result<(), VaultError> {
        let server = self.server.lock().await.take();
        if let Some(server) = server {
            let closing = Closing(server);
            // Match Python's short grace period for painting the final result.
            tokio::time::sleep(Duration::from_millis(250)).await;
            closing.0.stop().await?;
        }
        Ok(())
    }
    fn status(&self, message: &str) {
        if let Some(status) = &self.options.on_status {
            status(message);
        }
    }
}

fn render_frames(frames: Vec<Vec<u8>>) -> Result<(Vec<String>, Vec<String>), VaultError> {
    let mut rendered = Vec::with_capacity(frames.len());
    let mut frames_hex = Vec::with_capacity(frames.len());
    let mut rendered_length = 0usize;
    for frame in frames {
        let svg = svg_data_uri(&frame)?;
        let hex = hex::encode(frame);
        rendered_length = rendered_length
            .checked_add(svg.len())
            .and_then(|length| length.checked_add(hex.len()))
            .ok_or_else(|| error("Vault rendered request length overflow"))?;
        if rendered_length > REQUEST_LIMIT - 4096 {
            return Err(error("Vault rendered request exceeds the size limit"));
        }
        rendered.push(svg);
        frames_hex.push(hex);
    }
    Ok((rendered, frames_hex))
}
impl Drop for VaultSigner {
    fn drop(&mut self) {
        if let Some(server) = self.server.get_mut().take() {
            server.shutdown_now();
        }
    }
}
struct Closing(Arc<VaultSessionServer>);
impl Drop for Closing {
    fn drop(&mut self) {
        self.0.shutdown_now();
    }
}
impl Signer for VaultSigner {
    fn account(&self) -> AccountId {
        AccountId(self.public_key)
    }
    fn crypto_type(&self) -> CryptoType {
        self.options.crypto_type
    }
    fn caps(&self) -> SignerCaps {
        SignerCaps::METADATA_HASH | SignerCaps::UNSIGNED_TX | SignerCaps::OUT_OF_BAND
    }
    fn metadata_constants(&self) -> Option<(u8, String)> {
        Some((9, "TAO".to_owned()))
    }
    fn prepare_shielded(&self) -> BoxFuture<'_, Result<(), SignerError>> {
        Box::pin(async move {
            self.warm_up()
                .await
                .map_err(|error| SignerError::Vault(error.to_string()))
        })
    }
    fn sign<'a>(
        &'a self,
        request: &'a SigningRequest<'a>,
    ) -> BoxFuture<'a, Result<MultiSignature, SignerError>> {
        Box::pin(async move {
            self.sign_unsigned_extrinsic(request)
                .await
                .map_err(|error| SignerError::Vault(error.to_string()))
        })
    }
}
fn chain_info(context: &SigningContext) -> rtao_core::digest::ChainInfo {
    let mut info = context.chain_info.clone();
    info.decimals = 9;
    info.token_symbol = "TAO".to_owned();
    info
}
fn json_unsigned(value: &Value) -> Result<u64, VaultError> {
    value
        .as_u64()
        .or_else(|| {
            value.as_str().and_then(|text| {
                text.strip_prefix("0x")
                    .and_then(|hex| u64::from_str_radix(hex, 16).ok())
            })
        })
        .ok_or_else(|| error("invalid unsigned integer in Vault transaction summary"))
}
pub(super) fn era_seconds(era: Option<&Value>) -> Result<Option<f64>, VaultError> {
    match era {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(era)) => {
            let period = era
                .get("period")
                .map(json_unsigned)
                .transpose()?
                .unwrap_or(0);
            Ok((period != 0).then_some(period as f64 * 12.0))
        }
        Some(Value::String(era)) if matches!(era.as_str(), "00" | "0x00") => Ok(None),
        Some(Value::String(era)) => {
            let raw = hex::decode(era.strip_prefix("0x").unwrap_or(era))
                .map_err(|_| error("invalid SCALE era in Vault transaction"))?;
            if raw.len() != 2 || raw[0] == 0 {
                return Err(error("invalid SCALE era in Vault transaction"));
            }
            let encoded = u16::from_le_bytes([raw[0], raw[1]]);
            let period = 2u64 << (encoded & 15);
            let phase = u64::from(encoded >> 4) * (period >> 12).max(1);
            if period < 4 || phase >= period {
                return Err(error("invalid SCALE era in Vault transaction"));
            }
            Ok(Some(period as f64 * 12.0))
        }
        _ => Err(error("invalid era in Vault transaction")),
    }
}
struct ScannedSignature {
    bytes: [u8; 65],
    length: usize,
}
fn decode_python_hex(text: &str) -> Result<ScannedSignature, ()> {
    // bytes.fromhex permits ASCII whitespace between complete byte pairs.
    // Count all decoded bytes for accurate diagnostics but retain at most 65.
    if text.len() > super::MESSAGE_LIMIT {
        return Err(());
    }
    let mut output = ScannedSignature {
        bytes: [0; 65],
        length: 0,
    };
    let mut high = None;
    for byte in text.bytes() {
        if matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) && high.is_none() {
            continue;
        }
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return Err(()),
        };
        if let Some(first) = high.take() {
            if output.length < output.bytes.len() {
                output.bytes[output.length] = (first << 4) | digit;
            }
            output.length += 1;
        } else {
            high = Some(digit);
        }
    }
    if high.is_some() { Err(()) } else { Ok(output) }
}
