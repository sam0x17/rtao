// Derived from RaoFoundation/subtensor sdk/python/scripts/record_golden.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Replay every M0 golden section through rtao's argument conversions.

use std::{error::Error, path::PathBuf};

use rtao_core::{
    codec::Value,
    extrinsic::{TxParams, multisig_account_id},
    runtime::Runtime,
};
use rtao_primitives::{AccountId, CryptoType, H256, IntoCoreValue, NetUid, TaoBalance, Uid};
use rtao_testkit::fixtures::{Call, Golden, decode_fixed_hex, decode_hex};
use serde_json::Value as Json;

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;

fn golden() -> Result<Golden> {
    Ok(Golden::load(
        std::env::var_os("RTAO_RECORDED_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendor/upstream")
            })
            .join("golden.json"),
    )?)
}

fn expected_cases(count: usize, family: &str) -> Result<usize> {
    let Some(root) = std::env::var_os("RTAO_RECORDED_ROOT") else {
        return Ok(count);
    };
    let dropped: Vec<Json> =
        serde_json::from_slice(&std::fs::read(PathBuf::from(root).join("dropped.json"))?)?;
    count
        .checked_sub(dropped.iter().filter(|row| row["family"] == family).count())
        .ok_or_else(|| {
            format!("recorder dropped more {family} vectors than its watched source contains")
                .into()
        })
}

fn string(value: &Json) -> Result<&str> {
    value
        .as_str()
        .ok_or_else(|| "expected string fixture argument".into())
}
fn integer(value: &Json) -> Result<u64> {
    value
        .as_u64()
        .ok_or_else(|| "expected u64 fixture argument".into())
}
fn field<'a>(value: &'a Json, name: &str) -> Result<&'a Json> {
    value
        .get(name)
        .ok_or_else(|| format!("missing fixture argument {name}").into())
}
fn account(value: &Json) -> Result<AccountId> {
    Ok(string(value)?.parse()?)
}
fn netuid(value: &Json) -> Result<NetUid> {
    Ok(NetUid(u16::try_from(integer(value)?)?))
}
fn uids(value: &Json) -> Result<Vec<Uid>> {
    value
        .as_array()
        .ok_or("expected uid list")?
        .iter()
        .map(|item| Ok(Uid(u16::try_from(integer(item)?)?)))
        .collect()
}
fn weights(value: &Json) -> Result<Vec<u16>> {
    value
        .as_array()
        .ok_or("expected weight list")?
        .iter()
        .map(|item| Ok(u16::try_from(integer(item)?)?))
        .collect()
}
fn record(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::record(
        fields
            .into_iter()
            .map(|(name, value)| (name.into(), value))
            .collect(),
    )
    .into_core_value()
}

// The upstream recorder stores these two descriptions instead of nested call
// arguments. Reconstruct their calls from the pinned SS58 key vectors, never
// by splitting expected call bytes or using a fixture data_hex as input.
fn labelled_call(label: &str, fixture: &Golden, runtime: &Runtime) -> Result<Vec<u8>> {
    let (value, key) = match label {
        "transfer 1 to bob" => (
            1_u64,
            "0x8eaf04151687736326c9fea17e25fc5287613693c912909cb226aa4794f26a48",
        ),
        "transfer 2 to charlie" => (
            2_u64,
            "0x90b5ab205c6974c9ea841be688864633dc9ca8a357843eeacf2314649965fe22",
        ),
        _ => return Err(format!("unknown upstream nested-call description {label:?}").into()),
    };
    let dest: AccountId = fixture
        .ss58
        .iter()
        .find(|entry| entry.public_key_hex == key)
        .ok_or("nested-call destination missing from SS58 fixtures")?
        .address
        .parse()?;
    Ok(runtime.compose_call(
        "Balances",
        "transfer_keep_alive",
        &record([
            ("dest", dest.into_core_value()),
            ("value", value.into_core_value()),
        ]),
    )?)
}

fn compose(case: &Call, fixture: &Golden, runtime: &Runtime) -> Result<Vec<u8>> {
    let params = &case.params;
    let args = match (case.module.as_str(), case.function.as_str()) {
        ("Balances", "transfer_keep_alive" | "transfer_allow_death") => record([
            ("dest", account(field(params, "dest")?)?.into_core_value()),
            (
                "value",
                TaoBalance(integer(field(params, "value")?)?).into_core_value(),
            ),
        ]),
        ("SubtensorModule", "add_stake" | "remove_stake") => {
            let amount = if case.function == "add_stake" {
                "amount_staked"
            } else {
                "amount_unstaked"
            };
            record([
                (
                    "hotkey",
                    account(field(params, "hotkey")?)?.into_core_value(),
                ),
                (
                    "netuid",
                    netuid(field(params, "netuid")?)?.into_core_value(),
                ),
                (
                    amount,
                    TaoBalance(integer(field(params, amount)?)?).into_core_value(),
                ),
            ])
        }
        ("SubtensorModule", "burned_register") => record([
            (
                "netuid",
                netuid(field(params, "netuid")?)?.into_core_value(),
            ),
            (
                "hotkey",
                account(field(params, "hotkey")?)?.into_core_value(),
            ),
        ]),
        ("SubtensorModule", "set_weights") => record([
            (
                "netuid",
                netuid(field(params, "netuid")?)?.into_core_value(),
            ),
            ("dests", uids(field(params, "dests")?)?.into_core_value()),
            (
                "weights",
                weights(field(params, "weights")?)?.into_core_value(),
            ),
            (
                "version_key",
                integer(field(params, "version_key")?)?.into_core_value(),
            ),
        ]),
        ("System", "remark") => record([(
            "remark",
            Value::Bytes(decode_hex(string(field(params, "remark")?)?)?).into_core_value(),
        )]),
        ("Sudo", "sudo") => {
            let inner = field(params, "call")?;
            let nested = Call {
                module: string(field(inner, "module")?)?.into(),
                function: string(field(inner, "function")?)?.into(),
                params: Json::Object(
                    inner
                        .as_object()
                        .ok_or("expected nested call")?
                        .iter()
                        .filter(|(name, _)| {
                            name.as_str() != "module" && name.as_str() != "function"
                        })
                        .map(|(name, value)| (name.clone(), value.clone()))
                        .collect(),
                ),
                data_hex: String::new(),
            };
            record([(
                "call",
                Value::Bytes(compose(&nested, fixture, runtime)?).into_core_value(),
            )])
        }
        ("Utility", "batch") => {
            let calls: Result<Vec<Value>> = field(params, "calls")?
                .as_array()
                .ok_or("expected batch list")?
                .iter()
                .map(|label| {
                    Ok(Value::Bytes(labelled_call(
                        string(label)?,
                        fixture,
                        runtime,
                    )?))
                })
                .collect();
            record([("calls", calls?.into_core_value())])
        }
        ("Proxy", "proxy") => record([
            ("real", account(field(params, "real")?)?.into_core_value()),
            (
                "force_proxy_type",
                Some(string(field(params, "force_proxy_type")?)?).into_core_value(),
            ),
            (
                "call",
                Value::Bytes(labelled_call(
                    string(field(params, "call")?)?,
                    fixture,
                    runtime,
                )?)
                .into_core_value(),
            ),
        ]),
        names => return Err(format!("golden call has no typed reconstruction: {names:?}").into()),
    };
    Ok(runtime.compose_call(&case.module, &case.function, &args)?)
}

fn tx_params(
    fixture: &Golden,
    era: &Json,
    nonce: u64,
    tip: u128,
    birth: Option<&str>,
) -> Result<TxParams> {
    let era = match era {
        Json::String(value) if value == "00" => value.as_str().into_core_value(),
        Json::Object(_) => record([
            ("period", integer(field(era, "period")?)?.into_core_value()),
            (
                "current",
                integer(field(era, "current")?)?.into_core_value(),
            ),
        ]),
        _ => return Err("unsupported golden era shape".into()),
    };
    let genesis_hash = H256(decode_fixed_hex(&fixture.network.genesis_hash)?);
    let era_block_hash = H256(decode_fixed_hex(
        birth.unwrap_or(&fixture.network.genesis_hash),
    )?);
    Ok(TxParams {
        era,
        nonce,
        tip,
        tip_asset_id: None,
        genesis_hash: genesis_hash.0,
        era_block_hash: era_block_hash.0,
        metadata_hash: None,
    })
}

fn transfer_for(fixture: &Golden, runtime: &Runtime, expected: &str) -> Result<Vec<u8>> {
    let case = fixture
        .calls
        .iter()
        .find(|call| call.data_hex == expected)
        .ok_or("signing vector has no source call")?;
    let bytes = compose(case, fixture, runtime)?;
    assert_eq!(bytes, decode_hex(expected)?);
    Ok(bytes)
}

#[test]
fn golden_calls() -> Result {
    let fixture = golden()?;
    assert_eq!(fixture.calls.len(), expected_cases(10, "CALL_CASES")?);
    for runtime in [fixture.runtime()?, fixture.runtime_v14()?] {
        for (index, case) in fixture.calls.iter().enumerate() {
            assert_eq!(
                compose(case, &fixture, &runtime)?,
                decode_hex(&case.data_hex)?,
                "call #{index}: {}.{} (V15={})",
                case.module,
                case.function,
                runtime.is_v15
            );
        }
    }
    Ok(())
}

#[test]
fn golden_storage_keys() -> Result {
    let fixture = golden()?;
    assert_eq!(
        fixture.storage_keys.len(),
        expected_cases(10, "STORAGE_KEY_CASES")?
    );
    for runtime in [fixture.runtime()?, fixture.runtime_v14()?] {
        for (index, case) in fixture.storage_keys.iter().enumerate() {
            let args: Result<Vec<Value>> = case
                .params
                .iter()
                .enumerate()
                .map(|(position, param)| {
                    Ok(
                        match (
                            case.pallet.as_str(),
                            case.storage_function.as_str(),
                            position,
                        ) {
                            ("System", "Account", _)
                            | ("SubtensorModule", "Alpha", 0 | 1)
                            | ("Multisig", "Multisigs", 0) => account(param)?.into_core_value(),
                            ("Multisig", "Multisigs", 1) => {
                                H256(decode_fixed_hex(string(param)?)?).into_core_value()
                            }
                            ("SubtensorModule", "Bonds" | "Keys", 1) => {
                                Uid(u16::try_from(integer(param)?)?).into_core_value()
                            }
                            ("SubtensorModule", _, _) => netuid(param)?.into_core_value(),
                            _ => return Err(format!(
                                "storage vector has no typed key reconstruction: {}.{} #{position}",
                                case.pallet, case.storage_function
                            )
                            .into()),
                        },
                    )
                })
                .collect();
            let entry = runtime
                .storage_entry(&case.pallet, &case.storage_function)
                .ok_or("golden storage entry missing from embedded metadata")?;
            assert_eq!(
                case.value_scale_type,
                format!("scale_info::{}", entry.value_type)
            );
            assert_eq!(
                runtime.storage_key(entry, &args?)?,
                decode_hex(&case.key_hex)?,
                "storage key #{index}: {}.{} (V15={})",
                case.pallet,
                case.storage_function,
                runtime.is_v15
            );
        }
    }
    Ok(())
}

#[test]
fn golden_signature_payloads() -> Result {
    let fixture = golden()?;
    assert_eq!(fixture.signature_payloads.len(), 2);
    for runtime in [fixture.runtime()?, fixture.runtime_v14()?] {
        for (index, case) in fixture.signature_payloads.iter().enumerate() {
            let call = transfer_for(&fixture, &runtime, &case.call_data_hex)?;
            let params = tx_params(
                &fixture,
                &case.era,
                case.nonce,
                case.tip,
                case.era_birth_block_hash.as_deref(),
            )?;
            assert_eq!(
                runtime.signature_payload(&call, &params)?,
                decode_hex(&case.payload_hex)?,
                "signature payload #{index} (V15={})",
                runtime.is_v15
            );
        }
    }
    Ok(())
}

#[test]
fn golden_extrinsics() -> Result {
    let fixture = golden()?;
    assert_eq!(fixture.extrinsics.len(), 2);
    for runtime in [fixture.runtime()?, fixture.runtime_v14()?] {
        for (index, case) in fixture.extrinsics.iter().enumerate() {
            let call = transfer_for(&fixture, &runtime, &case.call_data_hex)?;
            let params = tx_params(&fixture, &case.era, case.nonce, case.tip, None)?;
            let signer: AccountId = case.address.parse()?;
            assert_eq!(signer.0, decode_fixed_hex(&case.public_key_hex)?);
            assert_eq!(
                u8::from(CryptoType::try_from(case.crypto_type)?),
                case.signature_version
            );
            let (bytes, hash) = runtime.encode_signed_extrinsic(
                &call,
                signer.0,
                &decode_hex(&case.signature_hex)?,
                case.signature_version,
                &params,
            )?;
            assert_eq!(
                bytes,
                decode_hex(&case.extrinsic_hex)?,
                "extrinsic #{index} (V15={})",
                runtime.is_v15
            );
            assert_eq!(
                hash,
                decode_fixed_hex(&case.extrinsic_hash)?,
                "extrinsic hash #{index}"
            );
        }
    }
    Ok(())
}

#[test]
fn golden_multisig() -> Result {
    let fixture = golden()?;
    assert_eq!(fixture.multisig.len(), 3);
    for (index, case) in fixture.multisig.iter().enumerate() {
        let accounts: Result<Vec<AccountId>> = case
            .signatories
            .iter()
            .map(|address| Ok(address.parse()?))
            .collect();
        let signatories: Vec<_> = accounts?.iter().map(|account| account.0).collect();
        let (raw, sorted) = multisig_account_id(&signatories, case.threshold)?;
        assert!(sorted.windows(2).all(|pair| pair.first() <= pair.get(1)));
        let derived = AccountId(raw);
        assert_eq!(
            derived.to_ss58(fixture.network.ss58_format),
            case.ss58_address,
            "multisig #{index}"
        );
        assert_eq!(case.ss58_address.parse::<AccountId>()?, derived);
        assert_eq!(derived.into_core_value(), Value::Bytes(raw.to_vec()));
    }
    Ok(())
}

#[test]
fn golden_ss58() -> Result {
    let fixture = golden()?;
    assert_eq!(fixture.ss58.len(), 4);
    for (index, case) in fixture.ss58.iter().enumerate() {
        let account = AccountId(decode_fixed_hex(&case.public_key_hex)?);
        assert_eq!(
            account.to_ss58(fixture.network.ss58_format),
            case.address,
            "SS58 #{index}"
        );
        assert_eq!(case.address.parse::<AccountId>()?, account);
        assert_eq!(
            account.into_core_value(),
            Value::Bytes(decode_hex(&case.public_key_hex)?)
        );
    }
    Ok(())
}
