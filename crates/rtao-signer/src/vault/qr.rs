// Derived from RaoFoundation/subtensor sdk/python/bittensor/vault/qr.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
use std::fmt::Write;

use base64::Engine;
use qrcodegen::{QrCode, QrCodeEcc, QrSegment, Version};

use super::{VaultError, error};

const QUIET_ZONE: i32 = 4;

/// Render a UOS frame as a base64 SVG data URI for the pinned Vault page.
///
/// Like Python's `qrcode.add_data(frame, optimize=0)`, the symbol has one
/// numeric, alphanumeric or byte segment, without segment splitting. Error
/// correction is M, boosting is disabled and the quiet zone is four modules.
/// Encoders may select different valid masks from Python's encoder.
pub fn svg_data_uri(frame: &[u8]) -> Result<String, VaultError> {
    let svg = render_svg(frame)?;
    Ok(format!(
        "data:image/svg+xml;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(svg)
    ))
}

pub(super) fn render_svg(frame: &[u8]) -> Result<String, VaultError> {
    // Version 40/M carries at most 5596 numeric, 3391 alphanumeric or 2331
    // byte characters. Bound scanning and bit-vector allocation for public input.
    if frame.len() > 5596 {
        return Err(error("Vault frame exceeds QR capacity"));
    }
    let segment = match std::str::from_utf8(frame) {
        Ok(text) if !text.is_empty() && QrSegment::is_numeric(text) => {
            QrSegment::make_numeric(text)
        }
        Ok(text) if QrSegment::is_alphanumeric(text) => {
            if frame.len() > 3391 {
                return Err(error("Vault frame exceeds QR alphanumeric-mode capacity"));
            }
            QrSegment::make_alphanumeric(text)
        }
        _ => {
            if frame.len() > 2331 {
                return Err(error("Vault frame exceeds QR byte-mode capacity"));
            }
            QrSegment::make_bytes(frame)
        }
    };
    let qr = QrCode::encode_segments_advanced(
        &[segment],
        QrCodeEcc::Medium,
        Version::MIN,
        Version::MAX,
        None,
        false,
    )
    .map_err(|_| error("Vault frame exceeds QR capacity"))?;
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
