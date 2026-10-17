//! Python `uuid.uuid1()` / `uuid.uuid4()` text without a uuid dependency.

use std::time::{SystemTime, UNIX_EPOCH};

// 100 ns intervals between 1582-10-15 and 1970-01-01 (RFC 4122).
const GREGORIAN_OFFSET: u64 = 0x01B2_1DD2_1381_4000;

fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    // A failing OS RNG leaves zeros; uuids are identifiers, not secrets.
    let _ = getrandom::fill(&mut bytes);
    bytes
}

fn format(bytes: [u8; 16]) -> String {
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// A time-based version-1 uuid with a random clock sequence and a random
/// multicast node, as Python's `uuid.uuid1()` text (36 lowercase characters).
pub fn uuid1_string() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() / 100)
        .unwrap_or(0);
    let timestamp = u64::try_from(now)
        .unwrap_or(0)
        .wrapping_add(GREGORIAN_OFFSET);
    let noise: [u8; 8] = random();
    let mut bytes = [0u8; 16];
    bytes[0..4].copy_from_slice(&((timestamp & 0xffff_ffff) as u32).to_be_bytes());
    bytes[4..6].copy_from_slice(&(((timestamp >> 32) & 0xffff) as u16).to_be_bytes());
    bytes[6..8].copy_from_slice(&((((timestamp >> 48) & 0x0fff) as u16) | 0x1000).to_be_bytes());
    bytes[8] = (noise[0] & 0x3f) | 0x80;
    bytes[9] = noise[1];
    bytes[10..16].copy_from_slice(&noise[2..8]);
    bytes[10] |= 0x01;
    format(bytes)
}

/// A random version-4 uuid, as Python's `str(uuid.uuid4())`.
pub fn uuid4_string() -> String {
    let mut bytes: [u8; 16] = random();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes() {
        let one = uuid1_string();
        assert_eq!(one.len(), 36);
        assert_eq!(&one[14..15], "1");
        let four = uuid4_string();
        assert_eq!(&four[14..15], "4");
        assert!(matches!(&four[19..20], "8" | "9" | "a" | "b"));
        assert_ne!(uuid4_string(), four);
    }
}
