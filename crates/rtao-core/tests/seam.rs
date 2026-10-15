//! Behavioral checks for the core seam's conversion and offload boundaries.

use std::error::Error;

use rtao_core::{CoreError, DynValue, codec::Value, keys};

#[test]
fn dynamic_json_preserves_shape_and_full_width_numbers() -> Result<(), Box<dyn Error>> {
    let maximum = DynValue::from(Value::U256([255; 32]));
    let digits = "115792089237316195423570985008687907853269984665640564039457584007913129639935";
    assert_eq!(maximum.to_string(), digits);
    assert_eq!(maximum.to_json().to_string(), digits);
    let reparsed: serde_json::Value = serde_json::from_str(&maximum.to_string())?;
    assert_eq!(reparsed.to_string(), digits);
    assert_eq!(
        DynValue::from(Value::Uint(u128::MAX)).to_string(),
        u128::MAX.to_string()
    );
    assert_eq!(
        DynValue::from(Value::Int(i128::MIN)).to_string(),
        i128::MIN.to_string()
    );
    let shaped = DynValue::from(Value::Dict(vec![(
        Value::Bool(true),
        Value::Tuple(vec![Value::Bytes(vec![0, 255]), Value::Null]),
    )]));
    assert_eq!(
        shaped.to_json(),
        serde_json::json!({ "True": ["0x00ff", null] })
    );
    assert_eq!(DynValue::from(Value::Int(-1)).as_u128(), None);
    assert_eq!(DynValue::from(Value::Uint(u128::MAX)).as_i128(), None);
    assert_eq!(DynValue::from(Value::Int(42)).as_u128(), Some(42));
    Ok(())
}

#[test]
fn mapping_preserves_upstream_diagnostics() -> Result<(), Box<dyn Error>> {
    let error = keys::public_key_from_ss58("not-an-address")
        .err()
        .ok_or("invalid SS58 unexpectedly accepted")?;
    let original = error.to_string();
    let mapped = CoreError::from(error);
    assert!(matches!(mapped, CoreError::Crypto(_)));
    assert_eq!(mapped.to_string(), original);

    let error = keys::Keypair::from_seed(&[0; 31], keys::CRYPTO_ED25519)
        .err()
        .ok_or("wrong seed length accepted")?;
    let original = error.to_string();
    assert_eq!(CoreError::from(error).to_string(), original);
    Ok(())
}

#[cfg(feature = "offload")]
#[tokio::test(flavor = "current_thread")]
async fn offload_keeps_blocking_work_off_the_runtime_thread() -> Result<(), Box<dyn Error>> {
    let runtime_thread = std::thread::current().id();
    let work_thread = rtao_core::offload(|| std::thread::current().id()).await?;
    assert_ne!(work_thread, runtime_thread);
    let operation_error = rtao_core::offload(|| keys::public_key_from_ss58("invalid")).await?;
    assert!(operation_error.is_err());
    Ok(())
}
