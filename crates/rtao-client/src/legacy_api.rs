// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/runtime_api.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Frozen archive runtime APIs whose results contain pre-encoded SCALE bytes.

use rtao_core::{
    CoreError,
    codec::{
        Value,
        decode::{Cursor, compact_len},
    },
    runtime::{Primitive, TypeSpec},
    value_ext::ValueExt,
};
use rtao_runtime::RuntimeHandle;

use crate::ClientError;

#[derive(Clone, Copy)]
pub(crate) enum Accounts {
    None,
    One,
    Many,
}
#[derive(Clone, Copy)]
pub(crate) struct LegacyMethod {
    params: &'static [&'static str],
    output: &'static str,
    accounts: Accounts,
    stake: bool,
}

pub(crate) fn method(api: &str, name: &str) -> Option<LegacyMethod> {
    let (params, output, accounts, stake): (&[_], _, _, _) = match (api, name) {
        ("DelegateInfoRuntimeApi", "get_delegated") => {
            (&["Vec<u8>"], "Vec<DelegateInfo>", Accounts::One, false)
        }
        ("DelegateInfoRuntimeApi", "get_delegates") => {
            (&[], "Vec<DelegateInfo>", Accounts::None, false)
        }
        ("NeuronInfoRuntimeApi", "get_neuron_lite") => {
            (&["u16", "u16"], "NeuronInfoLite", Accounts::None, false)
        }
        ("NeuronInfoRuntimeApi", "get_neurons_lite") => {
            (&["u16"], "Vec<NeuronInfoLite>", Accounts::None, false)
        }
        ("NeuronInfoRuntimeApi", "get_neuron") => {
            (&["u16", "u16"], "NeuronInfo", Accounts::None, false)
        }
        ("NeuronInfoRuntimeApi", "get_neurons") => {
            (&["u16"], "Vec<NeuronInfo>", Accounts::None, false)
        }
        ("StakeInfoRuntimeApi", "get_stake_info_for_coldkey") => {
            (&["Vec<u8>"], "Vec<StakeInfo>", Accounts::One, true)
        }
        ("StakeInfoRuntimeApi", "get_stake_info_for_coldkeys") => (
            &["Vec<Vec<u8>>"],
            "Vec<(Vec<u8>, Vec<StakeInfo>)>",
            Accounts::Many,
            false,
        ),
        ("SubnetInfoRuntimeApi", "get_subnet_hyperparams") => (
            &["u16"],
            "Option<SubnetHyperparameters>",
            Accounts::None,
            false,
        ),
        ("SubnetInfoRuntimeApi", "get_subnet_info") => {
            (&["u16"], "Option<SubnetInfo>", Accounts::None, false)
        }
        ("SubnetInfoRuntimeApi", "get_subnet_info_v2") => {
            (&["u16"], "Option<SubnetInfoV2>", Accounts::None, false)
        }
        ("SubnetInfoRuntimeApi", "get_subnets_info" | "get_subnets_info_v2") => {
            (&[], "Vec<Option<SubnetInfo>>", Accounts::None, false)
        }
        _ => return None,
    };
    Some(LegacyMethod {
        params,
        output,
        accounts,
        stake,
    })
}

pub(crate) fn is_legacy(runtime: &RuntimeHandle, api: &str, name: &str) -> bool {
    runtime
        .runtime_api(api, name)
        .is_none_or(|info| runtime.core().type_ident(info.output) == "Vec<u8>")
}

fn account(value: &Value) -> Result<Value, ClientError> {
    match value {
        Value::Str(address) if !address.starts_with("0x") => {
            rtao_core::keys::public_key_from_ss58(address)
                .map(|key| Value::Bytes(key.to_vec()))
                .map_err(CoreError::from)
                .map_err(ClientError::from)
        }
        _ => {
            let bytes = value
                .value_bytes()
                .filter(|bytes| bytes.len() == 32)
                .ok_or_else(|| {
                    ClientError::Protocol("legacy API account must contain 32 bytes".into())
                })?;
            Ok(Value::Bytes(bytes))
        }
    }
}

impl LegacyMethod {
    pub(crate) fn encode(
        self,
        runtime: &RuntimeHandle,
        args: &[Value],
    ) -> Result<Vec<u8>, ClientError> {
        if args.len() != self.params.len() {
            return Err(ClientError::Protocol(format!(
                "legacy runtime API expects {} parameters, got {}",
                self.params.len(),
                args.len()
            )));
        }
        let mut args = args.to_vec();
        match self.accounts {
            Accounts::None => {}
            Accounts::One => {
                args[0] = account(&args[0])?;
            }
            Accounts::Many => {
                let (Value::List(accounts) | Value::Tuple(accounts)) = &args[0] else {
                    return Err(ClientError::Protocol(
                        "legacy API accounts must be a list".into(),
                    ));
                };
                args[0] = Value::List(accounts.iter().map(account).collect::<Result<_, _>>()?);
            }
        }
        let mut bytes = Vec::new();
        for (ty, arg) in self.params.iter().zip(&args) {
            let spec = runtime.core().type_spec(ty).map_err(CoreError::from)?;
            bytes.extend(
                runtime
                    .core()
                    .encode_spec(&spec, arg)
                    .map_err(CoreError::from)?,
            );
        }
        Ok(bytes)
    }
    pub(crate) fn decode(
        self,
        runtime: &RuntimeHandle,
        bytes: &[u8],
    ) -> Result<Value, ClientError> {
        let outer = runtime
            .core()
            .decode_spec(&TypeSpec::Bytes, bytes, true)
            .map_err(CoreError::from)?;
        let inner = outer.value_bytes().ok_or_else(|| {
            ClientError::Protocol("legacy API output is not encoded bytes".into())
        })?;
        if self.stake {
            decode_stake(runtime, &inner)
        } else {
            let spec = runtime
                .core()
                .type_spec(self.output)
                .map_err(CoreError::from)?;
            runtime
                .core()
                .decode_spec(&spec, &inner, true)
                .map_err(CoreError::from)
                .map_err(ClientError::from)
        }
    }
}

fn decode_stake(runtime: &RuntimeHandle, bytes: &[u8]) -> Result<Value, ClientError> {
    let mut cursor = Cursor::new(bytes);
    cursor.strict = true;
    let count = compact_len(&mut cursor).map_err(CoreError::from)?;
    // Every legacy row contains two 32-byte accounts and at least one compact byte.
    if count > cursor.remaining() / 65 {
        return Err(ClientError::Protocol(
            "legacy stake result count exceeds remaining bytes".into(),
        ));
    }
    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        let hotkey = runtime
            .core()
            .decode_value(&TypeSpec::AccountId, &mut cursor)
            .map_err(CoreError::from)?;
        let coldkey = runtime
            .core()
            .decode_value(&TypeSpec::AccountId, &mut cursor)
            .map_err(CoreError::from)?;
        let stake = runtime
            .core()
            .decode_value(
                &TypeSpec::Compact(Box::new(TypeSpec::Primitive(Primitive::U64))),
                &mut cursor,
            )
            .map_err(CoreError::from)?;
        if stake
            .as_u128()
            .is_none_or(|stake| u64::try_from(stake).is_err())
        {
            return Err(ClientError::Protocol(
                "legacy stake exceeds chain u64 balance".into(),
            ));
        }
        values.push(Value::record(vec![
            ("netuid".into(), Value::Int(0)),
            ("hotkey".into(), hotkey),
            ("coldkey".into(), coldkey),
            ("stake".into(), stake),
            ("locked".into(), Value::Int(0)),
            ("emission".into(), Value::Int(0)),
            ("drain".into(), Value::Int(0)),
            ("is_registered".into(), Value::Bool(false)),
        ]));
    }
    if cursor.remaining() != 0 {
        return Err(ClientError::Protocol(
            "legacy stake result contains trailing bytes".into(),
        ));
    }
    Ok(Value::List(values))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use parity_scale_codec::{Compact, Encode};
    use rtao_primitives::AccountId;
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
    fn frozen_account_encoders_use_compact_raw_keys() {
        let runtime = runtime();
        let address = AccountId([0x17; 32]).to_ss58(42);
        let one = method("DelegateInfoRuntimeApi", "get_delegated").unwrap();
        assert_eq!(
            one.encode(&runtime, &[Value::Str(address.clone())])
                .unwrap(),
            vec![0x17u8; 32].encode()
        );
        let many = method("StakeInfoRuntimeApi", "get_stake_info_for_coldkeys").unwrap();
        assert_eq!(
            many.encode(
                &runtime,
                &[Value::List(vec![
                    Value::Str(address),
                    Value::Bytes(vec![0x18; 32])
                ])]
            )
            .unwrap(),
            vec![vec![0x17u8; 32], vec![0x18u8; 32]].encode()
        );
        assert!(one.encode(&runtime, &[]).is_err());
        assert!(one.encode(&runtime, &[Value::Bytes(vec![7; 31])]).is_err());
    }
    #[test]
    fn double_encoded_stake_rows_upgrade_exact_legacy_shape() {
        let runtime = runtime();
        let definition = method("StakeInfoRuntimeApi", "get_stake_info_for_coldkey").unwrap();
        let raw = (Compact(1u32), [0x17u8; 32], [0x18u8; 32], Compact(u64::MAX)).encode();
        let value = definition.decode(&runtime, &raw.encode()).unwrap();
        assert_eq!(
            DynJson(value),
            serde_json::json!([{"netuid":0,"hotkey":AccountId([0x17;32]).to_ss58(42),"coldkey":AccountId([0x18;32]).to_ss58(42),"stake":u64::MAX,"locked":0,"emission":0,"drain":0,"is_registered":false}])
        );
        for malformed in [vec![], vec![4], vec![5, 0], vec![0, 0]] {
            assert!(definition.decode(&runtime, &malformed.encode()).is_err());
        }
        let mut trailing = raw;
        trailing.push(0);
        assert!(definition.decode(&runtime, &trailing.encode()).is_err());
        let mut overflow = (Compact(1u32), [0x17u8; 32], [0x18u8; 32]).encode();
        overflow.extend(Compact(u128::from(u64::MAX) + 1).encode());
        assert!(definition.decode(&runtime, &overflow.encode()).is_err());
        let mut outer = vec![0u8].encode();
        outer.push(0);
        assert!(definition.decode(&runtime, &outer).is_err());
    }
    #[allow(non_snake_case)]
    fn DynJson(value: Value) -> serde_json::Value {
        rtao_core::DynValue::new(value).to_json()
    }
    #[test]
    fn every_frozen_method_keeps_its_recorded_legacy_output() {
        for (api, name) in [
            ("DelegateInfoRuntimeApi", "get_delegated"),
            ("DelegateInfoRuntimeApi", "get_delegates"),
            ("NeuronInfoRuntimeApi", "get_neuron_lite"),
            ("NeuronInfoRuntimeApi", "get_neurons_lite"),
            ("NeuronInfoRuntimeApi", "get_neuron"),
            ("NeuronInfoRuntimeApi", "get_neurons"),
            ("StakeInfoRuntimeApi", "get_stake_info_for_coldkey"),
            ("StakeInfoRuntimeApi", "get_stake_info_for_coldkeys"),
            ("SubnetInfoRuntimeApi", "get_subnet_hyperparams"),
            ("SubnetInfoRuntimeApi", "get_subnet_info"),
            ("SubnetInfoRuntimeApi", "get_subnet_info_v2"),
            ("SubnetInfoRuntimeApi", "get_subnets_info"),
            ("SubnetInfoRuntimeApi", "get_subnets_info_v2"),
        ] {
            assert!(method(api, name).is_some());
        }
        assert_eq!(
            method("SubnetInfoRuntimeApi", "get_subnets_info_v2")
                .unwrap()
                .output,
            "Vec<Option<SubnetInfo>>"
        );
        assert!(method("AccountNonceApi", "account_nonce").is_none());
    }
}
