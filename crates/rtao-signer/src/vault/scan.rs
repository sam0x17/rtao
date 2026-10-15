// Derived from RaoFoundation/subtensor sdk/python/bittensor/vault/scan.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
use super::{VaultPageError, VaultServerOptions, VaultSessionServer, VaultStatus};

const SCAN_PROMPT: &str = "Open the key in Polkadot Vault (tap it in the key list) — the screen shows its address QR. Hold the phone up to this camera.";

/// Explicit page options for importing one Vault address QR.
#[derive(Clone)]
pub struct VaultScanOptions {
    /// Loopback listener and explicit browser-opening behavior.
    pub server: VaultServerOptions,
    /// Optional local status callback.
    pub on_status: Option<VaultStatus>,
    /// Connection and scan deadlines; default scan wait is ten minutes.
    pub timeouts: super::VaultRequestTimeouts,
}
impl Default for VaultScanOptions {
    fn default() -> Self {
        Self {
            server: VaultServerOptions::default(),
            on_status: None,
            timeouts: super::VaultRequestTimeouts {
                answer: std::time::Duration::from_secs(600),
                ..Default::default()
            },
        }
    }
}
impl std::fmt::Debug for VaultScanOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultScanOptions")
            .field("server", &self.server)
            .field("timeouts", &self.timeouts)
            .finish_non_exhaustive()
    }
}

/// Parse the pinned Python address-QR convention without changing its spelling.
///
/// A `substrate:<ss58>:<genesis>` QR yields the first address/genesis fields,
/// adding lowercase `0x` when absent. An empty genesis yields `None`; extra
/// colon fields are ignored, just as in Python. Bare text is returned intact.
/// This parser does not validate an SS58 checksum or a chain identity: callers
/// compare the optional genesis against their selected chain before use.
pub fn parse_vault_address(text: &str) -> (String, Option<String>) {
    if let Some(rest) = text.strip_prefix("substrate:") {
        let mut parts = rest.split(':');
        let address = parts.next().unwrap_or_default().to_owned();
        let genesis = parts.next().filter(|part| !part.is_empty()).map(|part| {
            if part.starts_with("0x") {
                part.to_owned()
            } else {
                format!("0x{part}")
            }
        });
        (address, genesis)
    } else {
        (text.to_owned(), None)
    }
}

/// Open an ephemeral session, scan one address QR and stop its page server.
///
/// Browser opening is opt-in through the options. Cancellation also drops and
/// stops the owned server, so this helper leaves no daemon or temporary files.
#[doc(alias = "bittensor.vault.scan_vault_address")]
pub async fn scan_vault_address(
    options: &VaultScanOptions,
) -> Result<(String, Option<String>), VaultPageError> {
    let server = VaultSessionServer::new(options.server.clone())?;
    server.start().await?;
    if let Some(status) = &options.on_status {
        status(&format!(
            "scan page open at {} — show your Vault key's address QR to the webcam",
            server.http_url()
        ));
    }
    let result = server.request_scan(SCAN_PROMPT, options.timeouts).await;
    let cleanup = server.stop().await;
    let text = result?;
    cleanup?;
    Ok(parse_vault_address(&text))
}
