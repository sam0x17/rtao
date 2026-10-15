// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_shape_corpus.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Replay every pinned shape through scalar and batch codecs and primitive visitors.

use std::{collections::BTreeSet, error::Error, path::PathBuf};

use rtao_core::{
    DynValue,
    codec::{Value, batch::PARALLEL_THRESHOLD, decode::Cursor},
    runtime::{Runtime, TypeSpec},
};
use rtao_primitives::{
    AccountId, AlphaBalance, DefaultAccount, H256, I32F32, I64F64, I96F32, IntoCoreValue, MechId,
    NetUid, NetUidStorageIndex, PerU16, Perbill, Percent, Permill, TaoBalance, U64F64, json,
};
use rtao_testkit::fixtures::{Golden, ShapeCorpus, ShapeSample, ShapeType, decode_hex};
use scale_decode::DecodeAsType;
use scale_info::TypeDef;

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;
fn fixture_path(name: &str) -> PathBuf {
    if let Some(root) = std::env::var_os("RTAO_RECORDED_ROOT") {
        return PathBuf::from(root).join(if name == "corpus.json" {
            "shape_corpus/corpus.json"
        } else {
            name
        });
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor/upstream")
        .join(name)
}

fn registry_name(runtime: &Runtime, id: u32) -> Result<String> {
    let ty = runtime.resolve(id)?;
    if !ty.path.segments.is_empty() {
        return Ok(ty.path.segments.join("::"));
    }
    Ok(match ty.type_def {
        TypeDef::Primitive(_) => "primitive",
        TypeDef::Composite(_) => "composite",
        TypeDef::Variant(_) => "variant",
        TypeDef::Sequence(_) => "sequence",
        TypeDef::Array(_) => "array",
        TypeDef::Tuple(_) => "tuple",
        TypeDef::Compact(_) => "compact",
        TypeDef::BitSequence(_) => "bitsequence",
    }
    .into())
}

fn decode_sample(runtime: &Runtime, ty: &ShapeType, sample: &ShapeSample) -> Result<Value> {
    let bytes = decode_hex(&sample.scale_hex)?;
    let mut cursor = Cursor::new(&bytes);
    let nested = runtime.decode_id(ty.id, &mut cursor)?;
    if cursor.remaining() != 0 {
        return Err(format!("{} unread bytes", cursor.remaining()).into());
    }
    // The upstream batch replay uses decode_spec: its top-level zero-field
    // composite is (), while decode_id's nested composite is {}. Exercise
    // both owned upstream paths and require their documented sole difference.
    let value = runtime.decode_spec(&TypeSpec::Id(ty.id), &bytes, true)?;
    if matches!(&runtime.resolve(ty.id)?.type_def, TypeDef::Composite(fields) if fields.fields.is_empty())
    {
        assert_eq!(nested, Value::Dict(Vec::new()));
        assert_eq!(value, Value::Tuple(Vec::new()));
    } else {
        assert_eq!(
            nested, value,
            "root/nested decode diverged for type {}",
            ty.id
        );
    }
    let actual = DynValue::new(value.clone()).to_json();
    json::compare(&sample.decoded, &actual)
        .map_err(|error| format!("{error}; expected {}, actual {actual}", sample.decoded))?;
    Ok(value)
}

// pyt: sdk/python/tests/unit/test_shape_corpus.py::test_corpus_exists_and_covers_registry
#[test]
fn corpus_replay() -> Result {
    let fixture = Golden::load(fixture_path("golden.json"))?;
    let corpus = ShapeCorpus::load(fixture_path("corpus.json"))?;
    let runtime = fixture.runtime()?;
    assert_eq!(corpus.codec, "bittensor-core");
    assert_eq!(corpus.spec_version, runtime.spec_version);
    if std::env::var_os("RTAO_RECORDED_ROOT").is_none() {
        assert_eq!(corpus.types.len(), 797);
        assert_eq!(corpus.skipped.len(), 5);
        assert_eq!(runtime.types.types.len(), 802);
    }
    assert_eq!(corpus.types.len(), corpus.types_covered);
    assert_eq!(runtime.types.types.len(), corpus.types_total);
    assert!(corpus.types_total > 0, "recorded registry is empty");
    let covered_percent = corpus
        .types_covered
        .checked_mul(100)
        .ok_or("covered type percentage overflows usize")?;
    let required_percent = corpus
        .types_total
        .checked_mul(95)
        .ok_or("registry type percentage overflows usize")?;
    assert!(
        covered_percent > required_percent,
        "{} of {} recorded types must cover more than 95% of the registry",
        corpus.types_covered,
        corpus.types_total
    );
    let mut covered = BTreeSet::new();
    let mut samples = 0usize;
    let mut failures = Vec::new();
    for ty in &corpus.types {
        assert!(covered.insert(ty.id), "duplicate corpus type {}", ty.id);
        assert_eq!(
            ty.name,
            registry_name(&runtime, ty.id)?,
            "type {} registry path",
            ty.id
        );
        assert!(!ty.samples.is_empty(), "type {} has no samples", ty.id);
        for (index, sample) in ty.samples.iter().enumerate() {
            samples += 1;
            if let Err(error) = decode_sample(&runtime, ty, sample) {
                failures.push(format!(
                    "type {} ({}) sample {index}: {error}",
                    ty.id, ty.name
                ));
            }
        }
    }
    for ty in &corpus.skipped {
        assert!(covered.insert(ty.id), "duplicate skipped type {}", ty.id);
        assert_eq!(ty.name, registry_name(&runtime, ty.id)?);
        assert_eq!(ty.reason, "uninhabited variant type");
    }
    if std::env::var_os("RTAO_RECORDED_ROOT").is_none() {
        assert_eq!(samples, 2899);
    }
    assert!(samples > 0, "recorded shape corpus contains no samples");
    assert_eq!(
        covered,
        runtime.types.types.iter().map(|ty| ty.id).collect()
    );
    assert!(
        failures.is_empty(),
        "{} of {samples} shape samples failed:\n{}",
        failures.len(),
        failures
            .iter()
            .take(80)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    Ok(())
}

fn check_batch_shapes(
    runtime: &Runtime,
    type_strings: &[String],
    datas: &[Vec<u8>],
    samples: &[(u32, usize, &serde_json::Value)],
    context: &str,
) -> Result {
    let decoded = runtime.decode_batch(type_strings, datas)?;
    assert_eq!(decoded.len(), samples.len(), "{context} output count");
    let mut failures = Vec::new();
    for ((type_id, index, expected), value) in samples.iter().zip(decoded) {
        let actual = DynValue::new(value).to_json();
        if let Err(error) = json::compare(expected, &actual) {
            failures.push(format!(
                "type {type_id} sample {index}: {error}; expected {expected}, actual {actual}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{context}: {} of {} shape samples failed:\n{}",
        failures.len(),
        samples.len(),
        failures
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    Ok(())
}

// pyt: sdk/python/tests/unit/test_shape_corpus.py::test_decoded_shapes_match_corpus
#[test]
fn corpus_batch_replay() -> Result {
    let runtime = Golden::load(fixture_path("golden.json"))?.runtime()?;
    let corpus = ShapeCorpus::load(fixture_path("corpus.json"))?;
    let mut type_strings = Vec::new();
    let mut datas = Vec::new();
    let mut samples = Vec::new();
    for ty in &corpus.types {
        for (index, sample) in ty.samples.iter().enumerate() {
            type_strings.push(format!("scale_info::{}", ty.id));
            datas.push(decode_hex(&sample.scale_hex)?);
            samples.push((ty.id, index, &sample.decoded));
        }
    }
    if std::env::var_os("RTAO_RECORDED_ROOT").is_none() {
        assert_eq!(samples.len(), 2899);
        assert!(samples.len() > PARALLEL_THRESHOLD);
    }
    assert!(
        !samples.is_empty(),
        "recorded shape corpus contains no samples"
    );
    // This is the Python test's single ordered call, including consecutive
    // repeated type strings. Every expected value comes from the recording.
    check_batch_shapes(&runtime, &type_strings, &datas, &samples, "full corpus")?;

    // Revisit every sample below the host fan-out threshold, checking both
    // paths against the recording rather than against each other's output.
    let serial_chunk = PARALLEL_THRESHOLD
        .checked_sub(1)
        .filter(|size| *size > 0)
        .ok_or("batch threshold must exceed one")?;
    for ((type_strings, datas), samples) in type_strings
        .chunks(serial_chunk)
        .zip(datas.chunks(serial_chunk))
        .zip(samples.chunks(serial_chunk))
    {
        check_batch_shapes(&runtime, type_strings, datas, samples, "serial chunk")?;
    }
    // Host builds select rayon at the threshold; portable builds use the
    // same public API serially. These assertions make no scheduling claim.
    for count in [PARALLEL_THRESHOLD, PARALLEL_THRESHOLD + 1] {
        if samples.len() >= count {
            check_batch_shapes(
                &runtime,
                &type_strings[..count],
                &datas[..count],
                &samples[..count],
                &format!("threshold batch of {count}"),
            )?;
        }
    }
    Ok(())
}

fn typed_projection<T: DecodeAsType + IntoCoreValue>(
    runtime: &Runtime,
    ty: &ShapeType,
    sample: &ShapeSample,
) -> Result {
    let bytes = decode_hex(&sample.scale_hex)?;
    let mut input = bytes.as_slice();
    let typed = T::decode_as_type(&mut input, ty.id, &runtime.types)?;
    if !input.is_empty() {
        return Err(format!("typed visitor left {} bytes", input.len()).into());
    }
    let projected = typed.into_core_value();
    let mut encoded = Vec::new();
    runtime.encode_id(ty.id, &projected, &mut encoded)?;
    assert_eq!(
        encoded, bytes,
        "primitive visitor changed SCALE bytes for {}",
        ty.name
    );
    // Accounts and hashes use an encoder-shaped Bytes carrier. Re-decoding
    // the typed projection applies the metadata-defined JSON presentation.
    let mut cursor = Cursor::new(&encoded);
    let normalized = runtime.decode_id(ty.id, &mut cursor)?;
    assert_eq!(cursor.remaining(), 0);
    json::compare(&sample.decoded, &DynValue::new(normalized).to_json())?;
    if ty.id != 0 {
        json::compare(&sample.decoded, &DynValue::new(projected).to_json())?;
    }
    Ok(())
}

fn typenum_unsigned(runtime: &Runtime, id: u32, depth: u8) -> Result<u32> {
    if depth > 32 {
        return Err("unexpected recursive typenum parameter".into());
    }
    let ty = runtime.resolve(id)?;
    match ty.path.segments.last().map(String::as_str) {
        Some("UTerm" | "B0") => Ok(0),
        Some("B1") => Ok(1),
        Some("UInt") => {
            let parameter = |name: &str| {
                ty.type_params
                    .iter()
                    .find(|parameter| parameter.name == name)
                    .and_then(|parameter| parameter.ty.as_ref())
                    .map(|parameter| parameter.id)
                    .ok_or_else(|| format!("typenum UInt lacks {name} parameter"))
            };
            let upper = typenum_unsigned(runtime, parameter("U")?, depth + 1)?;
            let bit = typenum_unsigned(runtime, parameter("B")?, depth + 1)?;
            upper
                .checked_mul(2)
                .and_then(|upper| upper.checked_add(bit))
                .ok_or_else(|| "typenum unsigned parameter exceeds u32".into())
        }
        _ => Err(format!("unexpected fixed-point fraction parameter: {:?}", ty.path).into()),
    }
}

fn fractional_bits(runtime: &Runtime, id: u32) -> Result<u32> {
    let parameter = runtime
        .resolve(id)?
        .type_params
        .iter()
        .find(|parameter| parameter.name == "Frac")
        .and_then(|parameter| parameter.ty.as_ref())
        .ok_or("fixed-point metadata lacks a Frac parameter")?;
    typenum_unsigned(runtime, parameter.id, 0)
}

#[test]
fn corpus_primitive_visitors() -> Result {
    let runtime = Golden::load(fixture_path("golden.json"))?.runtime()?;
    let corpus = ShapeCorpus::load(fixture_path("corpus.json"))?;
    let mut types = BTreeSet::new();
    let mut samples = 0usize;
    for ty in &corpus.types {
        let fixed_precision = match ty.id {
            58 => Some(U64F64::FRACTIONAL_BITS),
            192 => Some(I96F32::FRACTIONAL_BITS),
            204 => Some(I64F64::FRACTIONAL_BITS),
            691 => Some(I32F32::FRACTIONAL_BITS),
            _ => None,
        };
        if let Some(expected) = fixed_precision {
            assert_eq!(
                fractional_bits(&runtime, ty.id)?,
                expected,
                "fixed-point type {} precision",
                ty.id
            );
        }
        // Identical FixedU128/FixedI128 paths have different Frac parameters.
        // Match the pinned type ID as well, never the final path segment alone.
        for sample in &ty.samples {
            match (ty.id, ty.name.as_str()) {
                (0, "sp_core::crypto::AccountId32") => {
                    typed_projection::<AccountId>(&runtime, ty, sample)?;
                    typed_projection::<DefaultAccount>(&runtime, ty, sample)?;
                }
                (6, "subtensor_runtime_common::currency::TaoBalance") => {
                    typed_projection::<TaoBalance>(&runtime, ty, sample)?
                }
                (14, "primitive_types::H256") => typed_projection::<H256>(&runtime, ty, sample)?,
                (41, "subtensor_runtime_common::NetUid") => {
                    typed_projection::<NetUid>(&runtime, ty, sample)?
                }
                (43, "subtensor_runtime_common::currency::AlphaBalance") => {
                    typed_projection::<AlphaBalance>(&runtime, ty, sample)?
                }
                (44, "subtensor_runtime_common::NetUidStorageIndex") => {
                    typed_projection::<NetUidStorageIndex>(&runtime, ty, sample)?
                }
                (46, "sp_arithmetic::per_things::PerU16") => {
                    typed_projection::<PerU16>(&runtime, ty, sample)?
                }
                (58, "substrate_fixed::FixedU128") => {
                    typed_projection::<U64F64>(&runtime, ty, sample)?
                }
                (104, "sp_arithmetic::per_things::Permill") => {
                    typed_projection::<Permill>(&runtime, ty, sample)?
                }
                (192, "substrate_fixed::FixedI128") => {
                    typed_projection::<I96F32>(&runtime, ty, sample)?
                }
                (204, "substrate_fixed::FixedI128") => {
                    typed_projection::<I64F64>(&runtime, ty, sample)?
                }
                (253, "sp_arithmetic::per_things::Percent") => {
                    typed_projection::<Percent>(&runtime, ty, sample)?
                }
                (255, "subtensor_runtime_common::MechId") => {
                    typed_projection::<MechId>(&runtime, ty, sample)?
                }
                (466, "sp_arithmetic::per_things::Perbill") => {
                    typed_projection::<Perbill>(&runtime, ty, sample)?
                }
                (691, "substrate_fixed::FixedI64") => {
                    typed_projection::<I32F32>(&runtime, ty, sample)?
                }
                _ => continue,
            }
            types.insert(ty.id);
            samples += 1;
        }
    }
    assert_eq!(
        types,
        BTreeSet::from([
            0, 6, 14, 41, 43, 44, 46, 58, 104, 192, 204, 253, 255, 466, 691
        ])
    );
    assert_eq!(samples, 45);
    Ok(())
}
