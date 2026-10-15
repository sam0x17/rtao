//! Regression coverage through Alloy's actual EVM integer dependency.

use alloy_primitives::Uint;

fn expected_shift<const BITS: usize, const LIMBS: usize>(
    value: Uint<BITS, LIMBS>,
    amount: usize,
    left: bool,
) -> (Uint<BITS, LIMBS>, bool) {
    let mut output = [0u64; LIMBS];
    let mut overflow = false;
    for source in 0..BITS {
        if value.as_limbs()[source / 64] & (1u64 << (source % 64)) == 0 {
            continue;
        }
        let destination = if left {
            source.checked_add(amount).filter(|&bit| bit < BITS)
        } else {
            source.checked_sub(amount)
        };
        if let Some(bit) = destination {
            output[bit / 64] |= 1u64 << (bit % 64);
        } else {
            overflow = true;
        }
    }
    (Uint::from_limbs(output), overflow)
}

fn check_width<const BITS: usize, const LIMBS: usize>() {
    let alternating = Uint::<BITS, LIMBS>::from_limbs(core::array::from_fn(|i| {
        let bits = if i % 2 == 0 {
            0xaaaa_aaaa_aaaa_aaaa
        } else {
            0x5555_5555_5555_5555
        };
        if i + 1 == LIMBS {
            bits & Uint::<BITS, LIMBS>::MASK
        } else {
            bits
        }
    }));
    for value in [
        Uint::<BITS, LIMBS>::ZERO,
        Uint::<BITS, LIMBS>::ONE,
        Uint::<BITS, LIMBS>::MAX,
        alternating,
    ] {
        for amount in (0..=BITS + 1).chain([usize::MAX]) {
            let (left, lost_left) = expected_shift(value, amount, true);
            let (right, lost_right) = expected_shift(value, amount, false);
            assert_eq!(value.overflowing_shl(amount), (left, lost_left));
            assert_eq!(value.overflowing_shr(amount), (right, lost_right));
            assert_eq!(value.checked_shl(amount), (!lost_left).then_some(left));
            assert_eq!(value.checked_shr(amount), (!lost_right).then_some(right));
            assert_eq!(value.wrapping_shl(amount), left);
            assert_eq!(value.wrapping_shr(amount), right);
            assert_eq!(
                value.saturating_shl(amount),
                if lost_left {
                    Uint::<BITS, LIMBS>::MAX
                } else {
                    left
                }
            );
        }
    }
}

#[test]
fn shift_contract_matches_independent_bit_oracle() {
    check_width::<0, 0>();
    check_width::<1, 1>();
    check_width::<63, 1>();
    check_width::<64, 1>();
    check_width::<65, 2>();
    check_width::<127, 2>();
    check_width::<128, 2>();
    check_width::<160, 3>();
    check_width::<192, 3>();
    check_width::<255, 4>();
    check_width::<256, 4>();
    check_width::<257, 5>();
}

#[test]
fn big_shift_covers_pointer_and_integer_bounds() {
    type U = Uint<256, 4>;
    for amount in [
        U::from(256u64),
        U::from(1u64 << 32),
        U::from_limbs([0, 0, 1, 0]),
    ] {
        assert_eq!(U::ONE << amount, U::ZERO);
        assert_eq!(U::MAX >> amount, U::ZERO);
        assert_eq!(U::ZERO << amount, U::ZERO);
        assert_eq!(U::ZERO >> amount, U::ZERO);
    }
}

#[test]
fn strict_shifts_reject_discarded_nonzero_bits() {
    assert!(
        std::panic::catch_unwind(|| { Uint::<160, 3>::from_limbs([0, 0, 1 << 31]).strict_shl(1) })
            .is_err()
    );
    assert!(std::panic::catch_unwind(|| Uint::<256, 4>::ONE.strict_shr(64)).is_err());
}

#[cfg(target_pointer_width = "64")]
#[test]
fn primitive_shift_fast_paths_keep_large_shift_amounts() {
    let huge = 1usize << 32;
    for extra in [0, 1, 64, 127, 128, 130] {
        let amount = huge + extra;
        assert_eq!(Uint::<64, 1>::ONE.wrapping_shl(amount), Uint::ZERO);
        assert_eq!(Uint::<64, 1>::MAX.wrapping_shr(amount), Uint::ZERO);
        assert_eq!(Uint::<128, 2>::ONE.wrapping_shl(amount), Uint::ZERO);
        assert_eq!(Uint::<128, 2>::MAX.wrapping_shr(amount), Uint::ZERO);
        assert_eq!(Uint::<256, 4>::ONE.wrapping_shl(amount), Uint::ZERO);
        assert_eq!(Uint::<256, 4>::MAX.wrapping_shr(amount), Uint::ZERO);
        assert_eq!(Uint::<192, 3>::ONE.wrapping_shl(amount), Uint::ZERO);
        assert_eq!(Uint::<192, 3>::MAX.wrapping_shr(amount), Uint::ZERO);
    }
}
