// Derived from RaoFoundation/subtensor sdk/python/bittensor/metagraph.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Chain IP addresses rendered as HTTP axon targets.

use std::net::{Ipv4Addr, Ipv6Addr};

pub use url::Url;

use crate::{AccountId, Uid};

/// A registered neuron's HTTP endpoint and receiver identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AxonTarget {
    /// UID used to associate a response with its neuron.
    pub uid: Uid,
    /// Receiver hotkey used to bind the signed request.
    pub hotkey: AccountId,
    /// HTTP base URL, including brackets around IPv6 addresses.
    pub base_url: Url,
}

/// Render a served chain IP address as an HTTP base URL.
///
/// Zero IP means the hotkey is not serving. Like Python's metagraph endpoint
/// renderer, `ip_type == 6` selects IPv6 and other values select IPv4. An IPv4
/// value outside its 32-bit range cannot describe an endpoint and yields `None`.
pub fn axon_base_url(ip: u128, ip_type: u8, port: u16) -> Option<Url> {
    Url::parse(&format!("http://{}", axon_endpoint(ip, ip_type, port)?)).ok()
}

/// Python's served `ip:port` string, with hexadecimal IPv6 and brackets.
pub fn axon_endpoint(ip: u128, ip_type: u8, port: u16) -> Option<String> {
    if ip == 0 {
        return None;
    }
    Some(if ip_type == 6 {
        format!("[{}]:{port}", ipv6_text(Ipv6Addr::from(ip)))
    } else {
        format!("{}:{port}", Ipv4Addr::from(u32::try_from(ip).ok()?))
    })
}

// Python's ipaddress renders mapped addresses with hex groups, while Rust's
// Ipv6Addr Display uses dotted IPv4 for these addresses. Compress the first
// longest zero run exactly as ipaddress does.
fn ipv6_text(ip: Ipv6Addr) -> String {
    let segments = ip.segments();
    let mut longest = (0usize, 0usize);
    let mut run = (0usize, 0usize);
    for (index, segment) in segments.iter().enumerate() {
        if *segment == 0 {
            if run.1 == 0 {
                run.0 = index;
            }
            run.1 += 1;
            if run.1 > longest.1 {
                longest = run;
            }
        } else {
            run.1 = 0;
        }
    }
    if longest.1 < 2 {
        return segments
            .iter()
            .map(|segment| format!("{segment:x}"))
            .collect::<Vec<_>>()
            .join(":");
    }
    let before = segments
        .iter()
        .take(longest.0)
        .map(|segment| format!("{segment:x}"))
        .collect::<Vec<_>>()
        .join(":");
    let after = segments
        .iter()
        .skip(longest.0 + longest.1)
        .map(|segment| format!("{segment:x}"))
        .collect::<Vec<_>>()
        .join(":");
    format!("{before}::{after}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_endpoint_ipv4_ipv6_and_unserved_cases() {
        assert_eq!(
            axon_base_url(u128::from(u32::from(Ipv4Addr::new(127, 0, 0, 1))), 4, 8091)
                .map(|url| url.to_string()),
            Some("http://127.0.0.1:8091/".into())
        );
        assert_eq!(
            axon_base_url(u128::from(Ipv6Addr::LOCALHOST), 6, 8091).map(|url| url.to_string()),
            Some("http://[::1]:8091/".into())
        );
        assert_eq!(
            axon_base_url(u128::from(u32::MAX), 0, 0).map(|url| url.to_string()),
            Some("http://255.255.255.255:0/".into())
        );
        assert!(axon_base_url(0, 4, 8091).is_none());
        assert!(axon_base_url(0, 6, 8091).is_none());
        assert!(axon_base_url(u128::from(u32::MAX) + 1, 4, 8091).is_none());
        assert_eq!(
            axon_endpoint(
                u128::from(Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x7f00, 1)),
                6,
                8091
            ),
            Some("[::ffff:7f00:1]:8091".into())
        );
        assert_eq!(
            axon_endpoint(u128::from(Ipv6Addr::new(1, 0, 0, 2, 0, 0, 3, 4)), 6, 8091),
            Some("[1::2:0:0:3:4]:8091".into())
        );
    }
}
