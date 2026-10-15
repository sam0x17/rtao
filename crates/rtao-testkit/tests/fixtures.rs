//! Verify fixture loaders reject bad framing and retain all upstream sections.

use rtao_testkit::fixtures::{Golden, ShapeCorpus, decode_fixed_hex, decode_hex, metadata_bytes};
use std::{error::Error, path::PathBuf};

type Result = std::result::Result<(), Box<dyn Error>>;
fn path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor/upstream")
        .join(name)
}

#[test]
fn golden_metadata_loaders_preserve_runtime_identity() -> Result {
    let fixture = Golden::load(path("golden.json"))?;
    let v15 = fixture.runtime()?;
    let v14 = fixture.runtime_v14()?;
    assert!(v15.is_v15);
    assert!(!v14.is_v15);
    for runtime in [v15, v14] {
        assert_eq!(runtime.spec_version, fixture.network.spec_version);
        assert_eq!(
            runtime.transaction_version,
            fixture.network.transaction_version
        );
        assert_eq!(runtime.ss58_format, fixture.network.ss58_format);
        assert_eq!(runtime.metadata_bytes.get(..4), Some(b"meta".as_slice()));
    }
    for name in [
        "constants",
        "storage_values",
        "query_maps",
        "runtime_calls",
        "events",
        "block",
        "payment_info",
        "receipt",
    ] {
        assert!(fixture.sections.contains_key(name), "loader dropped {name}");
    }
    let corpus = ShapeCorpus::load(path("corpus.json"))?;
    assert_eq!(corpus.spec_version, fixture.network.spec_version);
    assert_eq!(corpus.types.len(), 797);
    assert_eq!(corpus.skipped.len(), 5);
    assert_eq!(
        corpus
            .types
            .iter()
            .map(|entry| entry.samples.len())
            .sum::<usize>(),
        2899
    );
    Ok(())
}

#[test]
fn opaque_metadata_rejects_absence_truncation_and_trailing_bytes() -> Result {
    assert_eq!(metadata_bytes("0x01106d657461")?, b"meta");
    assert_eq!(metadata_bytes("0x6d657461")?, b"meta");
    for malformed in [
        "0x00",
        "0x02106d657461",
        "0x01106d6574",
        "0x01106d65746100",
        "0x011078787878",
        "0x0141006d657461",
    ] {
        assert!(
            metadata_bytes(malformed).is_err(),
            "accepted malformed metadata: {malformed}"
        );
    }
    assert!(decode_hex("0102").is_err());
    assert!(decode_hex("0x0").is_err());
    assert!(decode_hex("0xgg").is_err());
    assert!(decode_fixed_hex::<32>("0x00").is_err());
    Ok(())
}

#[test]
fn golden_loader_retains_arbitrary_width_numbers_in_unmodeled_sections() -> Result {
    let input = br#"{
        "network": {"genesis_hash":"0x", "spec_version":461,
                    "transaction_version":1, "ss58_format":42},
        "metadata": {"block_hash":"0x", "v15_hex":"0x", "v14_hex":"0x"},
        "calls": [], "storage_keys": [], "signature_payloads": [],
        "extrinsics": [], "multisig": [], "ss58": [],
        "future_read": {
            "u128":170141183460469231731687303715884105728,
            "i128":-170141183460469231731687303715884105728,
            "u256":115792089237316195423570985008687907853269984665640564039457584007913129639935,
            "ratio":0.1
        }
    }"#;
    let fixture = Golden::from_slice(input)?;
    let original: serde_json::Value = serde_json::from_slice(input)?;
    rtao_primitives::json::compare(
        original
            .get("future_read")
            .ok_or("missing original section")?,
        fixture
            .sections
            .get("future_read")
            .ok_or("missing loaded section")?,
    )?;
    Ok(())
}
