//! Executable offline examples shared with the generated getting-started guide.

#[docify::export]
#[test]
fn checked_native_balances() -> Result<(), rtao::primitives::UnitError> {
    use rtao::primitives::{Balance, Tao, UnitError};

    let amount = Balance::<Tao>::parse("1.25")?;
    assert_eq!(amount.rao(), 1_250_000_000);

    let one_rao = Balance::<Tao>::from_rao(1);
    let maximum = Balance::<Tao>::from_rao(u64::MAX);
    assert_eq!(maximum.checked_add(one_rao), Err(UnitError::Overflow));
    assert_eq!(
        Balance::<Tao>::ZERO.checked_sub(one_rao),
        Err(UnitError::Underflow)
    );
    Ok(())
}

#[docify::export]
#[test]
fn python_compatible_number_formatting() {
    use rtao::primitives::pyfmt;

    assert_eq!(pyfmt::float_repr(0.00001), "1e-05");
    assert_eq!(pyfmt::float_repr(-0.0), "-0.0");
}
