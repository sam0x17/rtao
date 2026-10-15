// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/serving.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Publishing axon and metrics addresses as chain data.

use std::net::IpAddr;

use rtao_client::View;
use rtao_runtime::{Call, calls::subtensor_module as calls};
use rtao_tx::{BuildCx, Built, Intent, IntentError};

pub use crate::{ResetAxon, ServeAxon, ServeAxonTls, ServePrometheus};

// py: sdk/python/bittensor/intents/serving.py:52-64
impl Intent for ServeAxon {
    fn summary(&self) -> String {
        format!(
            "serve axon {}:{} on netuid {}",
            self.ip, self.port, self.netuid
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        Ok(Built::new(self.chain_call()?.compose(cx.runtime)?))
    }
}

impl ServeAxon {
    fn chain_call(&self) -> Result<Call, IntentError> {
        let (ip, ip_type) = address(&self.ip)?;
        Ok(calls::serve_axon(
            self.netuid,
            self.version,
            ip,
            self.port,
            ip_type,
            self.protocol,
            0,
            0,
        ))
    }
}

// py: sdk/python/bittensor/intents/serving.py:102-115
impl Intent for ServeAxonTls {
    fn summary(&self) -> String {
        format!(
            "serve TLS axon {}:{} on netuid {}",
            self.ip, self.port, self.netuid
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        Ok(Built::new(self.chain_call()?.compose(cx.runtime)?))
    }
}

impl ServeAxonTls {
    fn chain_call(&self) -> Result<Call, IntentError> {
        // Python validates the address before decoding the certificate.
        let (ip, ip_type) = address(&self.ip)?;
        let certificate = certificate_bytes(&self.certificate)?;
        Ok(calls::serve_axon_tls(
            self.netuid,
            self.version,
            ip,
            self.port,
            ip_type,
            self.protocol,
            0,
            0,
            certificate,
        ))
    }
}

// py: sdk/python/bittensor/intents/serving.py:142-151
impl Intent for ServePrometheus {
    fn summary(&self) -> String {
        format!(
            "serve prometheus {}:{} on netuid {}",
            self.ip, self.port, self.netuid
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        Ok(Built::new(self.chain_call()?.compose(cx.runtime)?))
    }
}

impl ServePrometheus {
    fn chain_call(&self) -> Result<Call, IntentError> {
        let (ip, ip_type) = address(&self.ip)?;
        Ok(calls::serve_prometheus(
            self.netuid,
            self.version,
            ip,
            self.port,
            ip_type,
        ))
    }
}

// py: sdk/python/bittensor/intents/serving.py:176-188
impl Intent for ResetAxon {
    fn summary(&self) -> String {
        format!("reset axon on netuid {}", self.netuid)
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        Ok(Built::new(self.chain_call().compose(cx.runtime)?))
    }
}

impl ResetAxon {
    fn chain_call(&self) -> Call {
        calls::serve_axon(self.netuid, 0, 0, 1, 4, 4, 0, 0)
    }
}

// CPython 3.12.11 Lib/ipaddress.py: ip_address, IPv6Address._split_scope_id.
// A scope is valid only on IPv6, must be nonempty, and cannot contain `%`.
// int(IPv6Address) deliberately discards the scope identifier.
fn address(input: &str) -> Result<(u128, u8), IntentError> {
    let invalid = || {
        IntentError::Validation(format!(
            "{} does not appear to be an IPv4 or IPv6 address",
            rtao_primitives::pyfmt::string_repr(input)
        ))
    };
    // IPv6Address rejects a prefix slash even when it occurs in the scope.
    if input.contains('/') {
        return Err(invalid());
    }
    let (host, scoped) = match input.split_once('%') {
        Some((host, scope)) if !scope.is_empty() && !scope.contains('%') => (host, true),
        Some(_) => return Err(invalid()),
        None => (input, false),
    };
    match host.parse::<IpAddr>().map_err(|_| invalid())? {
        IpAddr::V4(ip) if !scoped => Ok((u128::from(u32::from(ip)), 4)),
        IpAddr::V6(ip) => Ok((u128::from(ip), 6)),
        IpAddr::V4(_) => Err(invalid()),
    }
}

// CPython 3.12.11 Objects/bytesobject.c: _PyBytes_FromHex behavior.
// Python removeprefix("0x") is case-sensitive and runs before fromhex.
fn certificate_bytes(input: &str) -> Result<Vec<u8>, IntentError> {
    let input = input.strip_prefix("0x").unwrap_or(input);
    let invalid = |position| {
        IntentError::Validation(format!(
            "non-hexadecimal number found in fromhex() arg at position {position}"
        ))
    };
    // CPython rejects the first non-ASCII character before inspecting any hex.
    if let Some(position) = input.chars().position(|ch| !ch.is_ascii()) {
        return Err(invalid(position));
    }
    let input = input.as_bytes();
    let mut bytes = Vec::with_capacity(input.len() / 2);
    let mut offset = 0;
    while offset < input.len() {
        while input
            .get(offset)
            .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c))
        {
            offset += 1;
        }
        if offset == input.len() {
            break;
        }
        let upper = nibble(input[offset]).ok_or_else(|| invalid(offset))?;
        offset += 1;
        let lower = input
            .get(offset)
            .copied()
            .and_then(nibble)
            .ok_or_else(|| invalid(offset))?;
        offset += 1;
        bytes.push((upper << 4) | lower);
    }
    // D-017: NeuronCertificate permits one algorithm byte and at most 64 key
    // bytes. Check after decoding so Python's fromhex diagnostics keep priority.
    if bytes.len() > 65 {
        return Err(IntentError::Validation(
            "TLS certificate exceeds 65 bytes including its algorithm byte; the chain would silently discard it".into(),
        ));
    }
    Ok(bytes)
}

fn nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use rtao_primitives::NetUid;
    use rtao_runtime::RuntimeHandle;

    fn runtime() -> RuntimeHandle {
        RuntimeHandle::parse(
            include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
            478,
            1,
            42,
        )
        .unwrap()
    }

    #[test]
    fn ip_integer_and_version_match_both_families_and_scopes() {
        for (text, integer, family) in [
            ("0.0.0.0", 0, 4),
            ("203.0.113.9", 0xcb00_7109, 4),
            ("255.255.255.255", u128::from(u32::MAX), 4),
            ("::", 0, 6),
            ("::1", 1, 6),
            ("2001:db8::1", 0x2001_0db8_0000_0000_0000_0000_0000_0001, 6),
            ("::ffff:192.0.2.1", 0xffff_c000_0201, 6),
            ("fe80::1%eth0", 0xfe80_0000_0000_0000_0000_0000_0000_0001, 6),
            ("::1% ", 1, 6),
            ("::1%é", 1, 6),
        ] {
            assert_eq!(address(text).unwrap(), (integer, family), "{text}");
        }
    }

    #[test]
    fn invalid_addresses_keep_python_error_message() {
        for text in [
            "",
            "127.1",
            "2130706433",
            "127.00.0.1",
            "256.0.0.1",
            " 127.0.0.1",
            "[::1]",
            "::1/128",
            "::1%",
            "::1%eth0%x",
            "::1%eth0/64",
            "127.0.0.1%eth0",
            "::ffff:192.000.2.1",
            "::1'\n",
            "é",
        ] {
            assert_eq!(
                address(text).unwrap_err().to_string(),
                format!(
                    "{} does not appear to be an IPv4 or IPv6 address",
                    rtao_primitives::pyfmt::string_repr(text)
                ),
                "{text}"
            );
        }
    }

    #[test]
    fn certificate_accepts_prefix_case_and_whitespace_between_bytes() {
        for (text, expected) in [
            ("", vec![]),
            ("0x", vec![]),
            (" \t\n\r\x0b\x0c", vec![]),
            ("0x00aBFf", vec![0, 0xab, 0xff]),
            ("0x\t01 02\n03\r04\x0b05\x0c06", vec![1, 2, 3, 4, 5, 6]),
        ] {
            assert_eq!(certificate_bytes(text).unwrap(), expected, "{text:?}");
        }
    }

    #[test]
    fn certificate_length_counts_decoded_bytes_at_the_chain_boundary() {
        let mut tls = ServeAxonTls {
            netuid: NetUid(1),
            ip: "::1".into(),
            port: 443,
            certificate: format!("0x01 {}", "ab \t\n".repeat(64)),
            protocol: 4,
            version: 1,
        };
        let mut expected = vec![1];
        expected.extend_from_slice(&[0xab; 64]);
        assert_eq!(certificate_bytes(&tls.certificate).unwrap(), expected);
        let runtime = runtime();
        let expected = calls::serve_axon_tls(NetUid(1), 1, 1, 443, 6, 4, 0, 0, expected)
            .compose(&runtime)
            .unwrap();
        assert_eq!(
            tls.chain_call()
                .unwrap()
                .compose(&runtime)
                .unwrap()
                .as_bytes(),
            expected.as_bytes()
        );

        tls.certificate.push_str("ab");
        assert_eq!(
            tls.chain_call().unwrap_err().to_string(),
            "TLS certificate exceeds 65 bytes including its algorithm byte; the chain would silently discard it"
        );
    }

    #[test]
    fn tls_validation_orders_ip_then_hex_syntax_then_certificate_length() {
        let long_hex = "ab".repeat(66);
        let mut tls = ServeAxonTls {
            netuid: NetUid(1),
            ip: "not an address".into(),
            port: 443,
            certificate: long_hex.clone(),
            protocol: 4,
            version: 1,
        };
        assert_eq!(
            tls.chain_call().unwrap_err().to_string(),
            "'not an address' does not appear to be an IPv4 or IPv6 address"
        );

        tls.ip = "::1".into();
        for (text, position) in [
            (format!("0x{long_hex}0g"), 133),
            (format!("{long_hex}0"), 133),
            (format!("{long_hex}ggé"), 134),
        ] {
            tls.certificate = text;
            assert_eq!(
                tls.chain_call().unwrap_err().to_string(),
                format!("non-hexadecimal number found in fromhex() arg at position {position}")
            );
        }
        tls.certificate = long_hex;
        assert_eq!(
            tls.chain_call().unwrap_err().to_string(),
            "TLS certificate exceeds 65 bytes including its algorithm byte; the chain would silently discard it"
        );
    }

    #[test]
    fn certificate_diagnostics_use_python_positions_and_validation_order() {
        for (text, position) in [
            ("0", 1),
            ("0x0", 1),
            ("0X00", 1),
            ("0x0x00", 1),
            ("0 0", 1),
            ("00 0g", 4),
            (" 0x00", 2),
            ("é", 0),
            ("ggé", 2),
            ("0x00😀", 2),
            ("00\u{a0}11", 2),
            ("00\x1c11", 2),
        ] {
            assert_eq!(
                certificate_bytes(text).unwrap_err().to_string(),
                format!("non-hexadecimal number found in fromhex() arg at position {position}"),
                "{text:?}"
            );
        }
    }

    #[test]
    fn serving_composes_exact_chain_bytes_and_reset_placeholders() {
        let runtime = runtime();
        let intent = ServeAxon {
            netuid: NetUid(3),
            ip: "203.0.113.9".into(),
            port: 8080,
            protocol: 4,
            version: 1,
        };
        let mut expected = vec![7, 4];
        expected.extend_from_slice(&3u16.to_le_bytes());
        expected.extend_from_slice(&1u32.to_le_bytes());
        expected.extend_from_slice(&0xcb00_7109u128.to_le_bytes());
        expected.extend_from_slice(&8080u16.to_le_bytes());
        expected.extend_from_slice(&[4, 4, 0, 0]);
        assert_eq!(
            intent
                .chain_call()
                .unwrap()
                .compose(&runtime)
                .unwrap()
                .as_bytes(),
            expected
        );
        let reset = ResetAxon { netuid: NetUid(3) };
        let expected = calls::serve_axon(NetUid(3), 0, 0, 1, 4, 4, 0, 0)
            .compose(&runtime)
            .unwrap();
        assert_eq!(
            reset.chain_call().compose(&runtime).unwrap().as_bytes(),
            expected.as_bytes()
        );
    }

    #[test]
    fn tls_and_metrics_compose_ipv6_and_preserve_summary_text() {
        let runtime = runtime();
        let tls = ServeAxonTls {
            netuid: NetUid(3),
            ip: "::1%eth0".into(),
            port: 443,
            certificate: "0x01 ab".into(),
            protocol: 9,
            version: 7,
        };
        let expected = calls::serve_axon_tls(NetUid(3), 7, 1, 443, 6, 9, 0, 0, vec![1, 0xab])
            .compose(&runtime)
            .unwrap();
        assert_eq!(
            tls.chain_call()
                .unwrap()
                .compose(&runtime)
                .unwrap()
                .as_bytes(),
            expected.as_bytes()
        );
        assert_eq!(tls.summary(), "serve TLS axon ::1%eth0:443 on netuid 3");
        let metrics = ServePrometheus {
            netuid: NetUid(3),
            ip: "::1".into(),
            port: 9090,
            version: 11,
        };
        let expected = calls::serve_prometheus(NetUid(3), 11, 1, 9090, 6)
            .compose(&runtime)
            .unwrap();
        assert_eq!(
            metrics
                .chain_call()
                .unwrap()
                .compose(&runtime)
                .unwrap()
                .as_bytes(),
            expected.as_bytes()
        );
        assert_eq!(metrics.summary(), "serve prometheus ::1:9090 on netuid 3");
    }

    #[test]
    fn validation_happens_at_build_with_ip_before_certificate() {
        let mut tls = ServeAxonTls {
            netuid: NetUid(1),
            ip: "not an address".into(),
            port: 0,
            certificate: "not hex".into(),
            protocol: 0,
            version: 0,
        };
        assert!(tls.validate().is_ok());
        assert!(
            tls.chain_call()
                .unwrap_err()
                .to_string()
                .contains("IPv4 or IPv6")
        );
        tls.ip = "0.0.0.0".into();
        assert!(
            tls.chain_call()
                .unwrap_err()
                .to_string()
                .contains("fromhex()")
        );
        tls.certificate.clear();
        assert!(tls.chain_call().is_ok());
    }

    #[test]
    fn summaries_keep_raw_address_port_and_netuid() {
        let axon = ServeAxon {
            netuid: NetUid(9),
            ip: "2001:DB8::1".into(),
            port: 80,
            protocol: 0,
            version: 0,
        };
        assert_eq!(axon.summary(), "serve axon 2001:DB8::1:80 on netuid 9");
        assert_eq!(
            ResetAxon { netuid: NetUid(9) }.summary(),
            "reset axon on netuid 9"
        );
    }
}
