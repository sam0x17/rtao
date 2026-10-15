// Derived from RaoFoundation/subtensor sdk/python/bittensor/vault/__init__.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Polkadot Vault offline-signing protocol support.
//!
//! Construction is offline. Starting the loopback page or opening a browser
//! occurs only through an explicit async operation. The embedded page and
//! chain-specs image are the pinned upstream assets, served verbatim.

mod qr;
mod scan;
mod server;
mod signer;
pub mod uos;

pub use qr::svg_data_uri;
pub use scan::{VaultScanOptions, parse_vault_address, scan_vault_address};
pub use server::{VaultRequestTimeouts, VaultServerOptions, VaultSessionServer};
pub use signer::{VaultOptions, VaultSigner};

use std::{fmt, sync::MutexGuard, time::Duration};

/// The pinned Python guide linked from the signing page.
pub const VAULT_GUIDE_URL: &str = "https://www.bittensor.com/docs/guides/vault";
/// Optional progress callback. Messages never contain private key material.
pub type VaultStatus = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

pub(crate) const MESSAGE_LIMIT: usize = 1024 * 1024;
pub(crate) const REQUEST_LIMIT: usize = 16 * 1024 * 1024;
pub(crate) const SEND_TIMEOUT: Duration = Duration::from_secs(5);

/// A Vault proof, frame, scan or signing round-trip failure.
#[derive(Debug, Clone, PartialEq, Eq)]
#[doc(alias = "bittensor.vault.VaultError")]
#[doc(alias = "bittensor.vault.signer.VaultError")]
pub struct VaultError(pub(crate) String);
impl fmt::Display for VaultError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for VaultError {}

/// A loopback page, camera, timeout or session-protocol failure.
#[derive(Debug, Clone, PartialEq, Eq)]
#[doc(alias = "bittensor.vault.VaultPageError")]
pub struct VaultPageError(pub(crate) String);
impl fmt::Display for VaultPageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for VaultPageError {}
impl From<VaultPageError> for VaultError {
    fn from(error: VaultPageError) -> Self {
        Self(error.0)
    }
}
impl From<uos::UosError> for VaultError {
    fn from(error: uos::UosError) -> Self {
        Self(error.to_string())
    }
}

pub(crate) fn page_error(message: &str) -> VaultPageError {
    VaultPageError(message.to_owned())
}
pub(crate) fn error(message: &str) -> VaultError {
    VaultError(message.to_owned())
}
pub(crate) fn lock<T>(mutex: &std::sync::Mutex<T>) -> Result<MutexGuard<'_, T>, VaultPageError> {
    mutex
        .lock()
        .map_err(|_| page_error("Vault state lock is poisoned"))
}
pub(crate) fn random_id() -> Result<String, VaultPageError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| page_error("OS randomness is unavailable"))?;
    Ok(hex::encode(bytes))
}

/// Explicitly open an already started session page without a shell.
pub(crate) async fn open_page(url: &str, browser: Option<&str>) -> Result<(), VaultPageError> {
    use std::process::Stdio;
    use tokio::{process::Command, time::timeout};

    let choice = browser
        .map(str::to_owned)
        .or_else(|| std::env::var("BT_EXTENSION_BROWSER").ok());
    let choice = choice.as_deref().map(str::trim).filter(|choice| {
        !choice.is_empty() && !matches!(choice.to_ascii_lowercase().as_str(), "default" | "system")
    });
    let mut command = if let Some(browser) = choice {
        if browser.is_empty() || browser.chars().any(char::is_control) {
            return Err(page_error("invalid browser executable"));
        }
        #[cfg(target_os = "macos")]
        let command = {
            let mut command = Command::new("open");
            let application = match browser.to_ascii_lowercase().as_str() {
                "firefox" | "ff" => "Firefox",
                "chrome" | "google-chrome" => "Google Chrome",
                "chromium" => "Chromium",
                "brave" => "Brave Browser",
                "edge" => "Microsoft Edge",
                "safari" => "Safari",
                _ => browser,
            };
            command.arg("-a").arg(application).arg(url);
            command
        };
        #[cfg(not(target_os = "macos"))]
        let command = {
            let mut command = Command::new(browser);
            command.arg(url);
            command
        };
        command
    } else {
        #[cfg(target_os = "macos")]
        let command = {
            let mut command = Command::new("open");
            command.arg(url);
            command
        };
        #[cfg(target_os = "windows")]
        let command = {
            let mut command = Command::new("rundll32.exe");
            command.arg("url.dll,FileProtocolHandler").arg(url);
            command
        };
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let command = {
            let mut command = Command::new("xdg-open");
            command.arg(url);
            command
        };
        command
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let status = timeout(Duration::from_secs(10), command.status())
        .await
        .map_err(|_| page_error("opening the Vault page timed out"))?
        .map_err(|_| page_error("cannot open the Vault page; open its URL manually"))?;
    if status.success() {
        Ok(())
    } else {
        Err(page_error(
            "cannot open the Vault page; open its URL manually",
        ))
    }
}

#[cfg(test)]
mod tests;
