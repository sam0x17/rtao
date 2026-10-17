//! R1-R4: re-derivation of signed bytes from the SignerPayloadJSON view.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::*;
use rtao_remote_signer::server::{
    EraInfo, RebuildError, decode_era,
    rebuild::{build_payload_json, decode_compact, parse_payload_json, rebuild},
};
use rtao_testkit::fixtures::decode_hex;

#[test]
fn r1_v1_json_view_rebuilds_the_golden_extension_bytes_and_payload() {
    let vector = v1();
    let rebuilt = rebuild(&runtime(), &vector.body()).unwrap();
    assert_eq!(rebuilt.extra, decode_hex("0x25000c0000").unwrap());
    assert_eq!(rebuilt.additional, vector.additional);
    assert_eq!(
        rebuilt.payload,
        decode_hex(&golden().signature_payloads[1].payload_hex).unwrap()
    );
    assert_eq!(
        rebuilt.era,
        EraInfo::Mortal {
            period: 64,
            phase: 2
        }
    );
    assert_eq!(rebuilt.params.nonce, 3);
    assert_eq!(rebuilt.params.tip, 0);
    assert_eq!(rebuilt.params.metadata_hash, None);
    assert_eq!(rebuilt.params.era_block_hash, hex32(ERA_BLOCK_HEX));
    // The server-built JSON view equals the client's, key for key.
    assert_eq!(rebuilt.payload_json, vector.payload_json);
}

#[test]
fn r2_v2_immortal_json_view_rebuilds_the_golden_payload() {
    let vector = v2();
    let rebuilt = rebuild(&runtime(), &vector.body()).unwrap();
    assert_eq!(rebuilt.extra, decode_hex("0x001c0000").unwrap());
    assert_eq!(rebuilt.era, EraInfo::Immortal);
    assert_eq!(rebuilt.payload.len(), 114);
    assert_eq!(
        rebuilt.payload,
        decode_hex(&golden().signature_payloads[0].payload_hex).unwrap()
    );
    let json =
        build_payload_json(&runtime(), &vector.call, alice_account(), &rebuilt.params).unwrap();
    assert_eq!(json, vector.payload_json);
}

fn alice_account() -> rtao_primitives::AccountId {
    use rtao_signer::Signer;
    alice().account()
}

#[test]
fn r3_era_decode_table() {
    assert_eq!(decode_era(&[0]).unwrap(), EraInfo::Immortal);
    assert_eq!(
        decode_era(&[0x25, 0x00]).unwrap(),
        EraInfo::Mortal {
            period: 64,
            phase: 2
        }
    );
    assert_eq!(
        decode_era(&[0x03, 0x00]).unwrap(),
        EraInfo::Mortal {
            period: 16,
            phase: 0
        }
    );
    // Low nibble 15: period 2 << 15 = 65536, the largest allowed.
    assert_eq!(
        decode_era(&[0x0f, 0x00]).unwrap(),
        EraInfo::Mortal {
            period: 65_536,
            phase: 0
        }
    );
    // Low nibble 0: period 2 is below the minimum.
    assert!(decode_era(&[0x00, 0x01]).is_err());
    // Period 4 (low nibble 1) with phase 4 (high bits 4): phase >= period.
    assert!(decode_era(&[0x41, 0x00]).is_err());
    assert!(decode_era(&[]).is_err());
    assert!(decode_era(&[1, 2, 3]).is_err());
    // V4: the core's era birth for current 66 and period 64 is 66 (phase 2),
    // so the anchor block number must satisfy n % 64 == 2.
    assert_eq!(rtao_core::extrinsic::era_birth(64, 66), 66);
    assert!(
        EraInfo::Mortal {
            period: 64,
            phase: 2
        }
        .born_at(66)
    );
    assert!(
        EraInfo::Mortal {
            period: 64,
            phase: 2
        }
        .born_at(130)
    );
    assert!(
        !EraInfo::Mortal {
            period: 64,
            phase: 2
        }
        .born_at(64)
    );
    assert!(!EraInfo::Immortal.born_at(0));
    // Re-encoding {period 64, phase 2} gives 0x2500.
    let mut encoded = Vec::new();
    runtime()
        .core()
        .encode_era_value(
            &EraInfo::Mortal {
                period: 64,
                phase: 2,
            }
            .to_value(),
            &mut encoded,
        )
        .unwrap();
    assert_eq!(encoded, vec![0x25, 0x00]);
}

#[test]
fn r4_long_calls_hash_their_payload_and_still_rebuild() {
    let vector = with_call(remark(&[7; 300]));
    assert_eq!(vector.payload.len(), 32);
    let rebuilt = rebuild(&runtime(), &vector.body()).unwrap();
    assert_eq!(rebuilt.payload, vector.payload);
}

#[test]
fn inconsistent_views_are_tampered_and_wrong_types_are_malformed() {
    let base = v1().body();
    let tampered = |edit: &dyn Fn(&mut rtao_remote_signer::wire::SignRequest)| {
        let mut body = base.clone();
        edit(&mut body);
        rebuild(&runtime(), &body).unwrap_err()
    };
    assert!(matches!(
        tampered(&|body| body
            .payload_json
            .insert("nonce".into(), "0x00000004".into())
            .map(drop)
            .unwrap_or(())),
        RebuildError::Tampered(_)
    ));
    assert!(matches!(
        tampered(&|body| body
            .payload_json
            .insert("nonce".into(), 4.into())
            .map(drop)
            .unwrap_or(())),
        RebuildError::Malformed(_)
    ));
    assert!(matches!(
        tampered(&|body| body
            .payload_json
            .insert("tip".into(), "0x0001".into())
            .map(drop)
            .unwrap_or(())),
        RebuildError::Malformed(_)
    ));
    assert!(matches!(
        tampered(&|body| body.payload_json.remove("mode").map(drop).unwrap_or(())),
        RebuildError::Tampered(_)
    ));
    assert!(matches!(
        tampered(&|body| body
            .payload_json
            .insert("assetId".into(), "0x00".into())
            .map(drop)
            .unwrap_or(())),
        RebuildError::Tampered(_)
    ));
    assert!(matches!(
        tampered(&|body| body
            .payload_json
            .insert("address".into(), bob_ss58().into())
            .map(drop)
            .unwrap_or(())),
        RebuildError::Tampered(_)
    ));
    assert!(matches!(
        tampered(&|body| body
            .payload_json
            .insert("version".into(), 5.into())
            .map(drop)
            .unwrap_or(())),
        RebuildError::Tampered(_)
    ));
    assert!(matches!(
        tampered(&|body| body
            .payload_json
            .insert("signedExtensions".into(), serde_json::json!(["CheckNonce"]))
            .map(drop)
            .unwrap_or(())),
        RebuildError::Tampered(_)
    ));
    // Immortal era anchored somewhere other than genesis.
    let mut immortal = v2().body();
    immortal
        .payload_json
        .insert("blockHash".into(), ERA_BLOCK_HEX.into());
    assert!(matches!(
        rebuild(&runtime(), &immortal).unwrap_err(),
        RebuildError::Tampered(_)
    ));
    let (params, _) = parse_payload_json(&runtime(), &base).unwrap();
    assert_eq!(params.genesis_hash, genesis());
}

fn bob_ss58() -> String {
    use rtao_signer::Signer;
    bob().account().to_ss58(42)
}

#[test]
fn compact_decoding_is_canonical_and_complete() {
    assert_eq!(decode_compact(&[0x00]).unwrap(), 0);
    assert_eq!(decode_compact(&[0xfc]).unwrap(), 63);
    assert_eq!(decode_compact(&[0x01, 0x01]).unwrap(), 64);
    assert!(decode_compact(&[0x05, 0x00]).is_err());
    assert_eq!(decode_compact(&[0xe5, 0xc0]).unwrap(), 12_345);
    assert!(decode_compact(&[0x00, 0x00]).is_err());
    assert!(decode_compact(&[]).is_err());
    assert_eq!(
        decode_compact(&[0x03, 0x00, 0x00, 0x00, 0x40]).unwrap(),
        1 << 30
    );
    assert!(decode_compact(&[0x03, 0xff, 0xff, 0xff, 0x00]).is_err());
}
