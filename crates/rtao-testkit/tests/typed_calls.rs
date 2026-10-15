//! Every recorded call replays through generated builders and the core encoder.
use rtao_core::{DynValue, codec::Value};
use rtao_primitives::{AccountId, AlphaBalance, NetUid, TaoBalance};
use rtao_runtime::{Call, CallBytes, RuntimeHandle, calls};
use rtao_testkit::fixtures::{Golden, decode_hex};
use serde_json::Value as Json;
use std::{error::Error, path::PathBuf};
type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;
fn field<'a>(value: &'a Json, name: &str) -> Result<&'a Json> {
    value
        .get(name)
        .ok_or_else(|| format!("missing {name}").into())
}
fn text(value: &Json) -> Result<&str> {
    value.as_str().ok_or_else(|| "expected string".into())
}
fn integer(value: &Json) -> Result<u64> {
    value.as_u64().ok_or_else(|| "expected integer".into())
}
fn account(value: &Json) -> Result<AccountId> {
    Ok(text(value)?.parse()?)
}
fn netuid(value: &Json) -> Result<NetUid> {
    Ok(NetUid(u16::try_from(integer(value)?)?))
}
fn numbers(value: &Json) -> Result<Vec<u16>> {
    value
        .as_array()
        .ok_or("expected sequence")?
        .iter()
        .map(|v| Ok(u16::try_from(integer(v)?)?))
        .collect()
}
fn destination(account: AccountId) -> DynValue {
    DynValue::new(Value::Bytes(account.0.to_vec()))
}
fn labelled(label: &str, runtime: &RuntimeHandle) -> Result<CallBytes> {
    let (key, value) = match label {
        "transfer 1 to bob" => ("5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty", 1),
        "transfer 2 to charlie" => ("5FLSigC9HGRKVhB9FiEo4Y3koPsNmBmLJbpXg2mp1hXcS59Y", 2),
        _ => return Err(format!("unknown recorded label {label}").into()),
    };
    Ok(
        calls::balances::transfer_keep_alive(destination(key.parse()?), TaoBalance(value))
            .compose(runtime)?,
    )
}
fn build(pallet: &str, name: &str, params: &Json, runtime: &RuntimeHandle) -> Result<Call> {
    Ok(match (pallet, name) {
        ("Balances", "transfer_keep_alive") => calls::balances::transfer_keep_alive(
            destination(account(field(params, "dest")?)?),
            TaoBalance(integer(field(params, "value")?)?),
        ),
        ("Balances", "transfer_allow_death") => calls::balances::transfer_allow_death(
            destination(account(field(params, "dest")?)?),
            TaoBalance(integer(field(params, "value")?)?),
        ),
        ("SubtensorModule", "add_stake") => calls::subtensor_module::add_stake(
            account(field(params, "hotkey")?)?,
            netuid(field(params, "netuid")?)?,
            TaoBalance(integer(field(params, "amount_staked")?)?),
        ),
        ("SubtensorModule", "remove_stake") => calls::subtensor_module::remove_stake(
            account(field(params, "hotkey")?)?,
            netuid(field(params, "netuid")?)?,
            AlphaBalance(integer(field(params, "amount_unstaked")?)?),
        ),
        ("SubtensorModule", "burned_register") => calls::subtensor_module::burned_register(
            netuid(field(params, "netuid")?)?,
            account(field(params, "hotkey")?)?,
        ),
        ("SubtensorModule", "set_weights") => calls::subtensor_module::set_weights(
            netuid(field(params, "netuid")?)?,
            numbers(field(params, "dests")?)?,
            numbers(field(params, "weights")?)?,
            integer(field(params, "version_key")?)?,
        ),
        ("System", "remark") => calls::system::remark(decode_hex(text(field(params, "remark")?)?)?),
        ("Sudo", "sudo") => {
            let nested = field(params, "call")?;
            let inner = build(
                text(field(nested, "module")?)?,
                text(field(nested, "function")?)?,
                nested,
                runtime,
            )?
            .compose(runtime)?;
            calls::sudo::sudo(inner)
        }
        ("Utility", "batch") => calls::utility::batch(
            field(params, "calls")?
                .as_array()
                .ok_or("expected batch calls")?
                .iter()
                .map(|v| labelled(text(v)?, runtime))
                .collect::<Result<Vec<_>>>()?,
        ),
        ("Proxy", "proxy") => calls::proxy::proxy(
            DynValue::new(Value::Str(text(field(params, "real")?)?.to_owned())),
            Some(serde_json::from_value::<rtao_runtime::models::ProxyType>(
                field(params, "force_proxy_type")?.clone(),
            )?),
            labelled(text(field(params, "call")?)?, runtime)?,
        ),
        _ => return Err(format!("recorded builder uncovered {pallet}.{name}").into()),
    })
}
#[test]
fn all_ten_golden_calls_use_generated_builders() -> Result {
    let fixture = Golden::load(
        std::env::var_os("RTAO_RECORDED_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendor/upstream")
            })
            .join("golden.json"),
    )?;
    let runtime = RuntimeHandle::new(fixture.runtime()?)?;
    let dropped = if let Some(root) = std::env::var_os("RTAO_RECORDED_ROOT") {
        let dropped: Vec<Json> =
            serde_json::from_slice(&std::fs::read(PathBuf::from(root).join("dropped.json"))?)?;
        dropped
            .iter()
            .filter(|row| row["family"] == "CALL_CASES")
            .count()
    } else {
        0
    };
    assert_eq!(fixture.calls.len() + dropped, 10);
    for vector in fixture.calls {
        let call = build(&vector.module, &vector.function, &vector.params, &runtime)?;
        let encoded = call
            .compose(&runtime)
            .map_err(|error| format!("{}.{}: {error}", vector.module, vector.function))?;
        assert_eq!(
            encoded.as_bytes(),
            decode_hex(&vector.data_hex)?,
            "{}.{}",
            vector.module,
            vector.function
        );
        assert_eq!(encoded.stamp(), runtime.stamp());
    }
    Ok(())
}
