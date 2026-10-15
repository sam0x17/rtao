//! Differential checks over real and synthesized runtime API responses.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use bytes::Bytes;
use rtao_core::{
    codec::{Value, decode::Cursor},
    runtime::Runtime,
};
use rtao_metagraph::{
    DecodePath, DecodePreference, Metagraph, MetagraphOptions, MetagraphView, SnapshotInfo,
};
use rtao_primitives::{AccountId, NetUid, StakeWeight, Uid};
use rtao_runtime::RuntimeHandle;

fn runtime() -> RuntimeHandle {
    RuntimeHandle::parse(
        include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
        478,
        1,
        42,
    )
    .unwrap()
}
fn options(decoder: DecodePreference) -> MetagraphOptions {
    MetagraphOptions {
        decoder,
        commitments: false,
        view: true,
    }
}

#[test]
fn direct_decode_view_opt_out_falls_back_and_refuses_forced_view() {
    let runtime = runtime();
    let raw = Bytes::from_static(include_bytes!("../../../vendor/bench/finney-1.scale"));
    for (decoder, expected) in [
        (DecodePreference::Auto, DecodePath::Typed),
        (DecodePreference::Typed, DecodePath::Typed),
        (DecodePreference::Dynamic, DecodePath::Dynamic),
    ] {
        let graph = Metagraph::decode(
            raw.clone(),
            &runtime,
            NetUid(1),
            SnapshotInfo::default(),
            MetagraphOptions {
                view: false,
                ..options(decoder)
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(graph.decode_path(), expected);
    }
    let error = Metagraph::decode(
        raw,
        &runtime,
        NetUid(1),
        SnapshotInfo::default(),
        MetagraphOptions {
            view: false,
            ..options(DecodePreference::View)
        },
    )
    .unwrap_err();
    assert!(
        matches!(error, rtao_metagraph::MetagraphError::Shape(reason)
        if reason == "metagraph view is disabled")
    );
}

#[test]
#[ignore = "run in a separate process with RTAO_METAGRAPH_VIEW=0"]
fn environment_view_opt_out_overrides_direct_decode_options() {
    assert_eq!(std::env::var("RTAO_METAGRAPH_VIEW").as_deref(), Ok("0"));
    let runtime = runtime();
    let raw = Bytes::from_static(include_bytes!("../../../vendor/bench/finney-1.scale"));
    let graph = Metagraph::decode(
        raw.clone(),
        &runtime,
        NetUid(1),
        SnapshotInfo::default(),
        options(DecodePreference::Auto),
    )
    .unwrap()
    .unwrap();
    assert_eq!(graph.decode_path(), DecodePath::Typed);
    let error = Metagraph::decode(
        raw,
        &runtime,
        NetUid(1),
        SnapshotInfo::default(),
        options(DecodePreference::View),
    )
    .unwrap_err();
    assert!(
        matches!(error, rtao_metagraph::MetagraphError::Shape(reason)
        if reason == "metagraph view is disabled")
    );
}
#[test]
fn direct_decode_environment_opt_out_is_checked_in_a_child_process() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .env("RTAO_METAGRAPH_VIEW", "0")
        .args([
            "--ignored",
            "--exact",
            "environment_view_opt_out_overrides_direct_decode_options",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn dynamic(runtime: &RuntimeHandle, bytes: &[u8]) -> Value {
    let output = runtime
        .runtime_api("SubnetInfoRuntimeApi", "get_metagraph")
        .unwrap()
        .output;
    let mut cursor = Cursor::new(bytes);
    let value = runtime.core().decode_id(output, &mut cursor).unwrap();
    assert_eq!(cursor.remaining(), 0);
    value
}
fn differential(bytes: &[u8], netuid: NetUid) {
    let runtime = runtime();
    let mut json = Vec::new();
    for (preference, expected) in [
        (DecodePreference::View, DecodePath::View),
        (DecodePreference::Typed, DecodePath::Typed),
        (DecodePreference::Dynamic, DecodePath::Dynamic),
    ] {
        let graph = Metagraph::decode(
            Bytes::copy_from_slice(bytes),
            &runtime,
            netuid,
            SnapshotInfo::default(),
            options(preference),
        )
        .unwrap()
        .unwrap();
        assert_eq!(graph.decode_path(), expected);
        assert!(!graph.is_empty());
        let key = *graph.hotkey(Uid(0)).unwrap();
        assert_eq!(graph.by_hotkey(&key).unwrap().uid, Uid(0));
        assert!(graph.neuron(Uid(u16::MAX)).is_none());
        let owned = graph.to_owned_neurons().unwrap();
        assert_eq!(
            graph.validators().count(),
            owned
                .iter()
                .filter(|neuron| neuron.validator_permit)
                .count()
        );
        assert_eq!(graph.runtime_value().unwrap(), dynamic(&runtime, bytes));
        json.push(graph.to_json().unwrap());
    }
    assert_eq!(json[0], json[1]);
    assert_eq!(json[1], json[2]);
    let view = MetagraphView::parse(bytes, &runtime).unwrap().unwrap();
    assert_eq!(view.to_dynamic(), dynamic(&runtime, bytes));
    assert_eq!(view.hotkey_bytes().len(), view.len() * 32);
    let start = view.hotkey_bytes().as_ptr() as usize;
    let source = bytes.as_ptr() as usize;
    assert!(start >= source && start + view.hotkey_bytes().len() <= source + bytes.len());
}

#[test]
fn all_three_paths_match_committed_finney_payloads() {
    for (bytes, netuid) in [
        (
            include_bytes!("../../../vendor/bench/finney-1.scale").as_slice(),
            NetUid(1),
        ),
        (
            include_bytes!("../../../vendor/bench/finney-3.scale").as_slice(),
            NetUid(3),
        ),
        (
            include_bytes!("../../../vendor/bench/finney-64.scale").as_slice(),
            NetUid(64),
        ),
    ] {
        differential(bytes, netuid);
    }
}

fn set(graph: &mut Value, name: &str, value: Value) {
    let Value::Dict(fields) = graph else {
        panic!("record required");
    };
    if let Some((_, field)) = fields
        .iter_mut()
        .find(|(key, _)| matches!(key, Value::Str(key) if key == name))
    {
        *field = value;
    } else {
        fields.push((Value::Str(name.into()), value));
    }
}

#[test]
fn stake_weight_accessors_preserve_alpha_tagged_total_stake_json() {
    let runtime = runtime();
    let mut original = dynamic(
        &runtime,
        include_bytes!("../../../vendor/bench/finney-1.scale"),
    );
    set(&mut original, "netuid", Value::Int(3));
    let total_stake = 1_234_500_000_000u64;
    let Value::Dict(fields) = &mut original else {
        panic!("record required");
    };
    for (name, amount) in [
        ("emission", 125_000_000),
        ("alpha_stake", 2_500_000_000),
        ("tao_stake", 3_750_000_000),
        ("total_stake", total_stake),
    ] {
        let (_, column) = fields
            .iter_mut()
            .find(|(key, _)| matches!(key, Value::Str(key) if key == name))
            .unwrap();
        let Value::List(values) = column else {
            panic!("stake column required");
        };
        values[0] = Value::Int(i128::from(amount));
    }
    let output = runtime
        .runtime_api("SubnetInfoRuntimeApi", "get_metagraph")
        .unwrap()
        .output;
    // Python _build tags total_stake with (netuid, symbol), just like alpha_stake
    // and emission; tao_stake keeps root units (metagraph.py:363-366).
    // These literal strings follow Balance.__repr__ (balance.py:147-152).
    for (symbol, total_json, emission_json, alpha_json) in [
        ("λ", "1,234.500000000λ", "0.125000000λ", "2.500000000λ"),
        ("", "1,234.500000000α₃", "0.125000000α₃", "2.500000000α₃"),
    ] {
        let mut value = original.clone();
        set(
            &mut value,
            "symbol",
            Value::List(
                symbol
                    .bytes()
                    .map(|byte| Value::Int(i128::from(byte)))
                    .collect(),
            ),
        );
        let mut raw = Vec::new();
        runtime.core().encode_id(output, &value, &mut raw).unwrap();
        for (decoder, expected_path) in [
            (DecodePreference::View, DecodePath::View),
            (DecodePreference::Typed, DecodePath::Typed),
            (DecodePreference::Dynamic, DecodePath::Dynamic),
        ] {
            let graph = Metagraph::decode(
                Bytes::copy_from_slice(&raw),
                &runtime,
                NetUid(3),
                SnapshotInfo::default(),
                options(decoder),
            )
            .unwrap()
            .unwrap();
            assert_eq!(graph.decode_path(), expected_path);
            assert_eq!(graph.header.netuid, NetUid(3));
            let row = graph.neuron(Uid(0)).unwrap();
            let borrowed: StakeWeight = row.total_stake().unwrap();
            let owned = row.to_owned().unwrap();
            let owned_weight: StakeWeight = owned.total_stake;
            assert_eq!(borrowed, StakeWeight(total_stake));
            assert_eq!(owned_weight, borrowed);
            let neuron_json = owned.to_json();
            assert_eq!(neuron_json["total_stake"], total_json);
            assert_eq!(neuron_json["emission"], emission_json);
            assert_eq!(neuron_json["alpha_stake"], alpha_json);
            assert_eq!(neuron_json["tao_stake"], "τ3.750000000");
            let graph_json = graph.to_json().unwrap();
            assert_eq!(graph_json["neurons"][0], neuron_json);
            assert_eq!(graph_json["raw"]["total_stake"][0], total_stake);
        }
    }
}

#[test]
fn synthetic_2500_uid_payload_matches_all_paths() {
    differential(
        include_bytes!("../../../vendor/bench/synthetic-2500.scale"),
        NetUid(1),
    );
}

#[test]
fn padding_legacy_collateral_mechanism_and_text_follow_python() {
    let runtime = runtime();
    let mut value = dynamic(
        &runtime,
        include_bytes!("../../../vendor/bench/finney-1.scale"),
    );
    set(
        &mut value,
        "hotkeys",
        Value::List(vec![
            Value::Str(AccountId([7; 32]).to_string()),
            Value::Str(AccountId([8; 32]).to_string()),
        ]),
    );
    for name in [
        "coldkeys",
        "active",
        "validator_permit",
        "rank",
        "last_update",
        "emission",
    ] {
        set(&mut value, name, Value::List(vec![]));
    }
    for name in ["collateral_locked", "collateral_min", "collateral_earned"] {
        set(&mut value, name, Value::Null);
    }
    set(
        &mut value,
        "name",
        Value::List(vec![Value::Int(255), Value::Int(65)]),
    );
    set(&mut value, "symbol", Value::Str("0xcebb".into()));
    set(&mut value, "netuid", Value::Int(8193));
    let graph = Metagraph::from_dynamic(NetUid(1), value, SnapshotInfo::default()).unwrap();
    assert_eq!(graph.header.mechid.0, 2);
    assert_eq!(graph.header.name, "\u{fffd}A");
    assert_eq!(graph.header.symbol, "λ");
    let json = graph.to_json().unwrap();
    assert_eq!(json["neurons"][1]["coldkey"], "None");
    assert_eq!(json["neurons"][1]["active"], false);
    assert_eq!(json["neurons"][1]["last_update"], 0);
    assert_eq!(json["neurons"][1]["rank"], 0.0);
    assert!(json["neurons"][1]["collateral_locked"].is_null());
    assert!(
        json["neurons"][1]["emission"]
            .as_str()
            .unwrap()
            .contains('λ')
    );
}

#[test]
fn parser_rejects_truncated_input_and_trailing_bytes() {
    let runtime = runtime();
    let bytes = include_bytes!("../../../vendor/bench/finney-1.scale");
    for len in (0..bytes.len())
        .step_by(97)
        .chain(bytes.len().saturating_sub(512)..bytes.len())
    {
        assert!(
            MetagraphView::parse(&bytes[..len], &runtime).is_err(),
            "length {len}"
        );
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(MetagraphView::parse(&trailing, &runtime).is_err());
    assert!(MetagraphView::parse(&[0, 0], &runtime).is_err());
    assert!(MetagraphView::parse(&[2], &runtime).is_err());
    assert!(MetagraphView::parse(&[0], &runtime).unwrap().is_none());
}

#[test]
fn native_domain_rejects_negative_and_oversized_balance_columns() {
    let runtime = runtime();
    let original = dynamic(
        &runtime,
        include_bytes!("../../../vendor/bench/finney-1.scale"),
    );
    for amount in [Value::Int(-1), Value::Uint(u128::MAX)] {
        let mut value = original.clone();
        set(&mut value, "emission", Value::List(vec![amount]));
        let graph = Metagraph::from_dynamic(NetUid(1), value, SnapshotInfo::default()).unwrap();
        assert!(graph.to_owned_neurons().is_err());
    }
}

#[test]
fn changed_moving_price_fraction_is_rejected_before_curated_projection() {
    let raw = include_bytes!("../../../vendor/bench/finney-1.scale");
    let original = runtime();
    let original_value = dynamic(&original, raw);
    let mut core = Runtime::parse(
        include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
        478,
        1,
        42,
    )
    .unwrap();
    let metagraph = core
        .types
        .types
        .iter()
        .find(|ty| {
            ty.ty
                .path
                .segments
                .last()
                .is_some_and(|name| name == "Metagraph")
        })
        .unwrap();
    let scale_info::TypeDef::Composite(fields) = &metagraph.ty.type_def else {
        panic!("record required");
    };
    let fixed = fields
        .fields
        .iter()
        .find(|field| field.name.as_deref() == Some("moving_price"))
        .unwrap()
        .ty
        .id;
    let fraction32 = core.types.resolve(fixed).unwrap().type_params[0]
        .ty
        .unwrap()
        .id;
    let fraction64 = core
        .types
        .types
        .iter()
        .find(|ty| {
            ty.ty.path.segments == ["substrate_fixed", "FixedI128"]
                && ty.ty.type_params[0].ty.unwrap().id != fraction32
        })
        .unwrap()
        .ty
        .type_params[0]
        .ty
        .unwrap()
        .id;
    core.types
        .types
        .iter_mut()
        .find(|ty| ty.id == fixed)
        .unwrap()
        .ty
        .type_params[0]
        .ty = Some(fraction64.into());
    let changed = RuntimeHandle::new(core).unwrap();
    // The SCALE bits still decode identically, while their numeric scale changed.
    assert_eq!(dynamic(&changed, raw), original_value);
    for decoder in [DecodePreference::Auto, DecodePreference::Dynamic] {
        let error = Metagraph::decode(
            Bytes::from_static(raw),
            &changed,
            NetUid(1),
            SnapshotInfo::default(),
            options(decoder),
        )
        .unwrap_err();
        assert!(
            matches!(error, rtao_metagraph::MetagraphError::Shape(reason)
            if reason == "moving_price semantic type is not signed I96F32")
        );
    }
    for decoder in [DecodePreference::View, DecodePreference::Typed] {
        assert!(
            Metagraph::decode(
                Bytes::from_static(raw),
                &changed,
                NetUid(1),
                SnapshotInfo::default(),
                options(decoder),
            )
            .is_err()
        );
    }
}

#[test]
fn added_fields_select_typed_fallback_and_preserve_raw_json() {
    let metadata = include_bytes!("../../../vendor/metadata/finney/478.v15.scale");
    let mut core = Runtime::parse(metadata, 478, 1, 42).unwrap();
    let boolean = core
        .types
        .types
        .iter()
        .find(|ty| {
            matches!(
                ty.ty.type_def,
                scale_info::TypeDef::Primitive(scale_info::TypeDefPrimitive::Bool)
            )
        })
        .unwrap()
        .id;
    let metagraph = core
        .types
        .types
        .iter_mut()
        .find(|ty| {
            ty.ty
                .path
                .segments
                .last()
                .is_some_and(|name| name == "Metagraph")
        })
        .unwrap();
    let scale_info::TypeDef::Composite(fields) = &mut metagraph.ty.type_def else {
        panic!("record required");
    };
    fields.fields.push(scale_info::Field {
        name: Some("future_field".into()),
        ty: boolean.into(),
        type_name: Some("bool".into()),
        docs: vec![],
    });
    let original = runtime();
    let mut value = dynamic(
        &original,
        include_bytes!("../../../vendor/bench/finney-1.scale"),
    );
    set(&mut value, "future_field", Value::Bool(true));
    let runtime = RuntimeHandle::new(core).unwrap();
    let output = runtime
        .runtime_api("SubnetInfoRuntimeApi", "get_metagraph")
        .unwrap()
        .output;
    let mut bytes = Vec::new();
    runtime
        .core()
        .encode_id(output, &value, &mut bytes)
        .unwrap();
    assert!(MetagraphView::parse(&bytes, &runtime).is_err());
    let graph = Metagraph::decode(
        Bytes::from(bytes),
        &runtime,
        NetUid(1),
        SnapshotInfo::default(),
        options(DecodePreference::Auto),
    )
    .unwrap()
    .unwrap();
    assert_eq!(graph.decode_path(), DecodePath::Typed);
    assert_eq!(graph.to_json().unwrap()["raw"]["future_field"], true);
}

#[test]
fn renamed_declared_field_selects_dynamic_fallback() {
    let metadata = include_bytes!("../../../vendor/metadata/finney/478.v15.scale");
    let mut core = Runtime::parse(metadata, 478, 1, 42).unwrap();
    let metagraph = core
        .types
        .types
        .iter_mut()
        .find(|ty| {
            ty.ty
                .path
                .segments
                .last()
                .is_some_and(|name| name == "Metagraph")
        })
        .unwrap();
    let scale_info::TypeDef::Composite(fields) = &mut metagraph.ty.type_def else {
        panic!("record required");
    };
    fields
        .fields
        .iter_mut()
        .find(|field| field.name.as_deref() == Some("total_stake"))
        .unwrap()
        .name = Some("future_total_stake".into());
    let runtime = RuntimeHandle::new(core).unwrap();
    let graph = Metagraph::decode(
        Bytes::from_static(include_bytes!("../../../vendor/bench/finney-1.scale")),
        &runtime,
        NetUid(1),
        SnapshotInfo::default(),
        options(DecodePreference::Auto),
    )
    .unwrap()
    .unwrap();
    assert_eq!(graph.decode_path(), DecodePath::Dynamic);
    assert_eq!(graph.to_owned_neurons().unwrap()[0].total_stake.0, 0);
}

#[test]
fn repeated_hotkeys_use_pythons_first_row() {
    let runtime = runtime();
    let mut value = dynamic(
        &runtime,
        include_bytes!("../../../vendor/bench/finney-1.scale"),
    );
    let hotkey = AccountId([7; 32]);
    set(
        &mut value,
        "hotkeys",
        Value::List(vec![
            Value::Str(hotkey.to_string()),
            Value::Str(hotkey.to_string()),
        ]),
    );
    let graph = Metagraph::from_dynamic(NetUid(1), value, SnapshotInfo::default()).unwrap();
    assert_eq!(graph.by_hotkey(&hotkey).unwrap().uid, Uid(0));
}

#[test]
fn wide_reserves_round_the_exact_price_quotient_once() {
    let runtime = runtime();
    let mut value = dynamic(
        &runtime,
        include_bytes!("../../../vendor/bench/finney-1.scale"),
    );
    set(&mut value, "tao_in", Value::Int(9_007_199_254_740_993));
    set(&mut value, "alpha_in", Value::Int(3));
    let graph = Metagraph::from_dynamic(NetUid(1), value, SnapshotInfo::default()).unwrap();
    assert_eq!(graph.header.price, Some(3_002_399_751_580_331.0));
    assert_ne!(
        graph.header.price,
        Some(9_007_199_254_740_993u64 as f64 / 3.0)
    );
}

#[test]
fn mapped_ipv6_uses_pythons_hex_text_and_served_targets_keep_receivers() {
    let runtime = runtime();
    let mut value = dynamic(
        &runtime,
        include_bytes!("../../../vendor/bench/finney-1.scale"),
    );
    let ip = u128::from(std::net::Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x7f00, 1));
    set(
        &mut value,
        "axons",
        Value::List(vec![Value::record(vec![
            ("ip".into(), Value::Int(i128::try_from(ip).unwrap())),
            ("ip_type".into(), Value::Int(6)),
            ("port".into(), Value::Int(8091)),
        ])]),
    );
    let graph = Metagraph::from_dynamic(NetUid(1), value, SnapshotInfo::default()).unwrap();
    assert_eq!(
        graph.to_owned_neurons().unwrap()[0].axon.as_deref(),
        Some("[::ffff:7f00:1]:8091")
    );
    let targets = graph.axon_targets().collect::<Vec<_>>();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].uid, Uid(0));
    assert_eq!(&targets[0].hotkey, graph.hotkey(Uid(0)).unwrap());
    assert_eq!(targets[0].base_url.port(), Some(8091));
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config { cases: 256, failure_persistence: None, ..Default::default() })]
    #[test]
    fn malformed_bytes_never_panic(bytes in proptest::collection::vec(proptest::num::u8::ANY, 0..512)) {
        let runtime = runtime();
        let _result = MetagraphView::parse(&bytes, &runtime);
    }
}
