//! [`KeyBuf`]: a stack byte buffer for building keys without allocating.

use core::fmt;
use core::ops::Deref;

use crate::vec::CapacityError;

const HEX: &[u8; 16] = b"0123456789abcdef";

/// A fixed-capacity byte buffer on the stack.
///
/// Use it to build names and keys (`user/17`, `blob/0x2a`) where a `String`
/// and `format!` would allocate. Every push is all or nothing: on overflow the
/// buffer is left unchanged and an error is returned (a `write!` of several
/// pieces may still stop after the pieces that fit).
///
/// ```
/// use core::fmt::Write;
/// use rtao_arena::KeyBuf;
///
/// let mut key = KeyBuf::<32>::new();
/// key.push_str("user/")?;
/// key.push_u64(17)?;
/// assert_eq!(key.as_str(), Some("user/17"));
///
/// let mut other = KeyBuf::<16>::new();
/// write!(other, "{}/{}", "blob", 3)?;
/// assert_eq!(other.as_bytes(), b"blob/3");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Copy)]
pub struct KeyBuf<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> Default for KeyBuf<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> KeyBuf<N> {
    /// An empty buffer.
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }

    /// Bytes written.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether nothing is written.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The capacity `N`.
    pub const fn capacity(&self) -> usize {
        N
    }

    /// Bytes still free.
    pub const fn remaining(&self) -> usize {
        N - self.len
    }

    /// Empties the buffer.
    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// Shortens the buffer to `len` bytes (no-op when already shorter).
    pub fn truncate(&mut self, len: usize) {
        self.len = self.len.min(len);
    }

    /// The written bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    /// The written bytes as text, if they are UTF-8.
    pub fn as_str(&self) -> Option<&str> {
        core::str::from_utf8(self.as_bytes()).ok()
    }

    fn error() -> CapacityError {
        CapacityError { capacity: N }
    }

    /// Appends `bytes`.
    pub fn push(&mut self, bytes: &[u8]) -> Result<(), CapacityError> {
        let end = self.len + bytes.len();
        let slot = self.bytes.get_mut(self.len..end).ok_or_else(Self::error)?;
        slot.copy_from_slice(bytes);
        self.len = end;
        Ok(())
    }

    /// Appends one byte.
    pub fn push_byte(&mut self, byte: u8) -> Result<(), CapacityError> {
        self.push(&[byte])
    }

    /// Appends `text`.
    pub fn push_str(&mut self, text: &str) -> Result<(), CapacityError> {
        self.push(text.as_bytes())
    }

    /// Appends `value` in decimal.
    pub fn push_u64(&mut self, value: u64) -> Result<(), CapacityError> {
        let mut digits = [0u8; 20];
        let mut i = digits.len();
        let mut v = value;
        loop {
            i -= 1;
            digits[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.push(&digits[i..])
    }

    /// Appends `value` in lower-case hexadecimal without a prefix or padding.
    pub fn push_hex_u64(&mut self, value: u64) -> Result<(), CapacityError> {
        let mut digits = [0u8; 16];
        let mut i = digits.len();
        let mut v = value;
        loop {
            i -= 1;
            digits[i] = HEX[(v & 0xf) as usize];
            v >>= 4;
            if v == 0 {
                break;
            }
        }
        self.push(&digits[i..])
    }

    /// Appends every byte of `bytes` as two lower-case hex digits.
    pub fn push_hex(&mut self, bytes: &[u8]) -> Result<(), CapacityError> {
        if bytes.len().saturating_mul(2) > self.remaining() {
            return Err(Self::error());
        }
        for byte in bytes {
            self.bytes[self.len] = HEX[(byte >> 4) as usize];
            self.bytes[self.len + 1] = HEX[(byte & 0xf) as usize];
            self.len += 2;
        }
        Ok(())
    }
}

impl<const N: usize> Deref for KeyBuf<N> {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl<const N: usize> AsRef<[u8]> for KeyBuf<N> {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl<const N: usize> PartialEq for KeyBuf<N> {
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl<const N: usize> Eq for KeyBuf<N> {}

impl<const N: usize> core::hash::Hash for KeyBuf<N> {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.as_bytes().hash(state);
    }
}

impl<const N: usize> fmt::Write for KeyBuf<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.push_str(s).map_err(|_| fmt::Error)
    }
}

impl<const N: usize> fmt::Debug for KeyBuf<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.as_str() {
            Some(text) => fmt::Debug::fmt(text, f),
            None => fmt::Debug::fmt(self.as_bytes(), f),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use core::fmt::Write;

    #[test]
    fn pushes_are_all_or_nothing() {
        let mut key = KeyBuf::<8>::new();
        key.push_str("abc").unwrap();
        assert!(key.push_str("defghi").is_err());
        assert_eq!(key.as_bytes(), b"abc");
        key.push_u64(0).unwrap();
        key.push_hex_u64(0x2a).unwrap();
        assert_eq!(key.as_str(), Some("abc02a"));
        assert!(key.push_hex(&[1, 2]).is_err());
        key.push_hex(&[0xff]).unwrap();
        assert_eq!(key.as_str(), Some("abc02aff"));
        assert_eq!(key.remaining(), 0);
        assert!(write!(key, "x").is_err());
        let mut big = KeyBuf::<32>::new();
        big.push_u64(u64::MAX).unwrap();
        assert_eq!(big.as_str(), Some("18446744073709551615"));
    }
}
