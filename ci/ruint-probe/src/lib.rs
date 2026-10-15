//! Security regressions against the dependency with `std` and `alloc` disabled.
//!
//! CI runs this separate workspace under an external timeout: the affected
//! `to_base_be` constructor can loop before returning an iterator.

#![no_std]

#[cfg(test)]
mod tests {
    use core::fmt::{self, Write};
    use ruint::Uint;

    // The local compatibility change must remain callable during evaluation of
    // constants, for both odd and even non-limb-aligned byte lengths.
    const ODD_BYTES: [u8; 9] =
        Uint::<65, 2>::from_limbs([0x0102_0304_0506_0708, 1]).to_be_bytes::<9>();
    const EVEN_BYTES: [u8; 20] =
        Uint::<160, 3>::from_limbs([0x0102_0304_0506_0708, 0x1112_1314_1516_1718, 0x2122_2324])
            .to_be_bytes::<20>();
    const EMPTY_BYTES: [u8; 0] = Uint::<0, 0>::ZERO.to_be_bytes::<0>();
    const ONE_BYTE: [u8; 1] = Uint::<7, 1>::from_limbs([0x55]).to_be_bytes::<1>();

    #[test]
    fn compatibility_const_byte_order() {
        assert_eq!(ODD_BYTES, [1, 1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(
            EVEN_BYTES,
            [
                0x21, 0x22, 0x23, 0x24, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 1, 2, 3, 4,
                5, 6, 7, 8,
            ]
        );
        assert_eq!(EMPTY_BYTES, []);
        assert_eq!(ONE_BYTE, [0x55]);
    }

    #[test]
    fn security_shift_flags_include_masked_and_discarded_limbs() {
        let top_bit = Uint::<160, 3>::from_limbs([0, 0, 1 << 31]);
        assert_eq!(top_bit.overflowing_shl(1), (Uint::ZERO, true));
        assert_eq!(top_bit.checked_shl(1), None);
        assert_eq!(top_bit.saturating_shl(1), Uint::MAX);

        let high_limb = Uint::<256, 4>::from_limbs([0, 0, 0, 1]);
        assert_eq!(high_limb.overflowing_shl(64), (Uint::ZERO, true));
        assert_eq!(high_limb.checked_shl(64), None);
        assert_eq!(high_limb.saturating_shl(64), Uint::MAX);

        let low_limb = Uint::<256, 4>::ONE;
        assert_eq!(low_limb.overflowing_shr(64), (Uint::ZERO, true));
        assert_eq!(low_limb.checked_shr(64), None);
    }

    fn check_binary_digits<const BITS: usize, const LIMBS: usize>() {
        // A masked top bit must stop highest_power's checked-shift loop.
        // The 1.17.2 implementation can hang in to_base_be here.
        let mut digits = Uint::<BITS, LIMBS>::MAX.to_base_be(2);
        for _ in 0..BITS {
            assert_eq!(digits.next(), Some(1));
        }
        assert_eq!(digits.next(), None);
    }

    #[test]
    fn security_no_alloc_base_conversion_terminates_at_partial_limbs() {
        check_binary_digits::<65, 2>();
        check_binary_digits::<160, 3>();
        check_binary_digits::<161, 3>();
        check_binary_digits::<255, 4>();
    }

    struct StackText {
        bytes: [u8; 80],
        len: usize,
    }

    impl Write for StackText {
        fn write_str(&mut self, text: &str) -> fmt::Result {
            let end = self.len.checked_add(text.len()).ok_or(fmt::Error)?;
            let target = self.bytes.get_mut(self.len..end).ok_or(fmt::Error)?;
            target.copy_from_slice(text.as_bytes());
            self.len = end;
            Ok(())
        }
    }

    #[test]
    fn security_no_alloc_formatting_keeps_full_value() {
        let mut text = StackText {
            bytes: [0; 80],
            len: 0,
        };
        assert!(write!(&mut text, "{}", Uint::<160, 3>::MAX).is_ok());
        assert_eq!(
            &text.bytes[..text.len],
            b"1461501637330902918203684832716283019655932542975"
        );
    }

    #[test]
    fn security_big_shift_amount_never_truncates() {
        type U = Uint<256, 4>;
        for amount in [U::from(1u64 << 32), U::from_limbs([0, 0, 1, 0])] {
            assert_eq!(U::ONE << amount, U::ZERO);
            assert_eq!(U::ONE >> amount, U::ZERO);
            assert_eq!(U::ZERO << amount, U::ZERO);
            assert_eq!(U::ZERO >> amount, U::ZERO);
        }
    }
}
