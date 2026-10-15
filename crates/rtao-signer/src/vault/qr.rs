// Derived from RaoFoundation/subtensor sdk/python/bittensor/vault/qr.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
use std::fmt::Write;

use base64::Engine;
use qrcodegen::{QrCode, QrCodeEcc, QrSegment, Version};

use super::{VaultError, error};

const QUIET_ZONE: i32 = 4;

/// Render a UOS frame as a base64 SVG data URI for the pinned Vault page.
///
/// The symbol has exactly one byte-mode segment, error correction M and a
/// four-module quiet zone. Error-correction boosting and text optimization are
/// disabled. Encoders may select different valid masks from Python's encoder.
pub fn svg_data_uri(frame: &[u8]) -> Result<String, VaultError> {
    let svg = render_svg(frame)?;
    Ok(format!(
        "data:image/svg+xml;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(svg)
    ))
}

pub(super) fn render_svg(frame: &[u8]) -> Result<String, VaultError> {
    // Version 40/M carries at most 2331 bytes in one byte segment. Refuse
    // before expanding an arbitrarily large public input into its bit vector.
    if frame.len() > 2331 {
        return Err(error("Vault frame exceeds QR byte-mode capacity"));
    }
    let segments = [QrSegment::make_bytes(frame)];
    let qr = QrCode::encode_segments_advanced(
        &segments,
        QrCodeEcc::Medium,
        Version::MIN,
        Version::MAX,
        None,
        false,
    )
    .map_err(|_| error("Vault frame exceeds QR byte-mode capacity"))?;
    let size = qr.size() + 2 * QUIET_ZONE;
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {size} {size}\" shape-rendering=\"crispEdges\"><rect width=\"{size}\" height=\"{size}\" fill=\"#fff\"/><path fill=\"#000\" d=\""
    );
    for y in 0..qr.size() {
        for x in 0..qr.size() {
            if qr.get_module(x, y) {
                write!(svg, "M{},{}h1v1h-1z", x + QUIET_ZONE, y + QUIET_ZONE)
                    .map_err(|_| error("cannot render Vault QR SVG"))?;
            }
        }
    }
    svg.push_str("\"/></svg>");
    Ok(svg)
}
