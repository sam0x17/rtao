//! The commitment enum's metadata-dependent Raw0 through Raw128 variants.
// SPDX-License-Identifier: Apache-2.0

use std::marker::PhantomData;

use rtao_core::codec::Value;
use rtao_primitives::IntoCoreValue;
use scale_decode::{
    Error, IntoVisitor, TypeResolver,
    visitor::{
        Visitor,
        types::{Composite, Variant},
    },
};

use super::{ToDynamic, bytes_dynamic};

/// A commitment field, including the runtime's length-indexed raw encodings.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Data {
    /// An absent commitment field.
    None,
    /// At most 128 raw bytes; metadata names this variant `Raw<length>`.
    Raw(Vec<u8>),
    /// A BLAKE2b-256 commitment hash.
    BlakeTwo256([u8; 32]),
    /// A SHA-256 commitment hash.
    Sha256([u8; 32]),
    /// A Keccak-256 commitment hash.
    Keccak256([u8; 32]),
    /// A SHA3-256 commitment hash.
    ShaThree256([u8; 32]),
    /// Timelock ciphertext awaiting a drand pulse.
    TimelockEncrypted {
        /// Ciphertext bytes stored by the runtime.
        encrypted: Vec<u8>,
        /// The drand round required for reveal.
        reveal_round: u64,
    },
    /// A commitment flag requesting a subnet bonds reset.
    ResetBondsFlag,
    /// A raw field stored with a SCALE vector length, bounded to 512 on chain.
    BigRaw(Vec<u8>),
    /// Retained ciphertext whose reveal attempt failed.
    TimelockRevealFailed {
        /// Ciphertext retained for inspection.
        encrypted: Vec<u8>,
        /// The drand round used in the failed attempt.
        reveal_round: u64,
    },
}

fn tagged(name: &str, value: Value) -> Value {
    Value::record(vec![(name.into(), value)])
}

impl ToDynamic for Data {
    fn to_dynamic(&self) -> Value {
        match self {
            Self::None => Value::Str("None".into()),
            Self::ResetBondsFlag => Value::Str("ResetBondsFlag".into()),
            Self::Raw(bytes) => tagged(
                &format!("Raw{}", bytes.len()),
                if bytes.is_empty() {
                    Value::List(Vec::new())
                } else {
                    Value::hex(bytes)
                },
            ),
            Self::BlakeTwo256(bytes) => tagged("BlakeTwo256", Value::hex(bytes)),
            Self::Sha256(bytes) => tagged("Sha256", Value::hex(bytes)),
            Self::Keccak256(bytes) => tagged("Keccak256", Value::hex(bytes)),
            Self::ShaThree256(bytes) => tagged("ShaThree256", Value::hex(bytes)),
            Self::BigRaw(bytes) => tagged("BigRaw", bytes_dynamic(bytes)),
            Self::TimelockEncrypted {
                encrypted,
                reveal_round,
            }
            | Self::TimelockRevealFailed {
                encrypted,
                reveal_round,
            } => tagged(
                if matches!(self, Self::TimelockEncrypted { .. }) {
                    "TimelockEncrypted"
                } else {
                    "TimelockRevealFailed"
                },
                Value::record(vec![
                    ("encrypted".into(), bytes_dynamic(encrypted)),
                    ("reveal_round".into(), reveal_round.to_dynamic()),
                ]),
            ),
        }
    }
}

impl IntoCoreValue for Data {
    fn into_core_value(self) -> Value {
        match self {
            Self::None => Value::Str("None".into()),
            Self::ResetBondsFlag => Value::Str("ResetBondsFlag".into()),
            Self::Raw(bytes) => tagged(&format!("Raw{}", bytes.len()), Value::Bytes(bytes)),
            Self::BlakeTwo256(bytes) => tagged("BlakeTwo256", Value::Bytes(bytes.to_vec())),
            Self::Sha256(bytes) => tagged("Sha256", Value::Bytes(bytes.to_vec())),
            Self::Keccak256(bytes) => tagged("Keccak256", Value::Bytes(bytes.to_vec())),
            Self::ShaThree256(bytes) => tagged("ShaThree256", Value::Bytes(bytes.to_vec())),
            Self::BigRaw(bytes) => tagged("BigRaw", Value::Bytes(bytes)),
            Self::TimelockEncrypted {
                encrypted,
                reveal_round,
            } => tagged(
                "TimelockEncrypted",
                Value::record(vec![
                    ("encrypted".into(), Value::Bytes(encrypted)),
                    ("reveal_round".into(), reveal_round.into_core_value()),
                ]),
            ),
            Self::TimelockRevealFailed {
                encrypted,
                reveal_round,
            } => tagged(
                "TimelockRevealFailed",
                Value::record(vec![
                    ("encrypted".into(), Value::Bytes(encrypted)),
                    ("reveal_round".into(), reveal_round.into_core_value()),
                ]),
            ),
        }
    }
}

/// Visitor that resolves commitment variants by their live metadata names.
pub struct DataVisitor<R>(PhantomData<R>);

fn first<T: IntoVisitor, R: TypeResolver>(fields: &mut Composite<'_, '_, R>) -> Result<T, Error> {
    fields
        .decode_item(T::into_visitor::<R>())
        .ok_or_else(|| Error::custom_str("commitment variant has no payload"))?
}

fn timelock<R: TypeResolver>(fields: &mut Composite<'_, '_, R>) -> Result<(Vec<u8>, u64), Error> {
    let mut encrypted = None;
    let mut reveal_round = None;
    for field in fields {
        let field = field?;
        match field.name() {
            Some("encrypted") => encrypted = Some(field.decode_as_type::<Vec<u8>>()?),
            Some("reveal_round") => reveal_round = Some(field.decode_as_type::<u64>()?),
            _ => {}
        }
    }
    Ok((
        encrypted.ok_or_else(|| Error::custom_str("timelock field encrypted is missing"))?,
        reveal_round.ok_or_else(|| Error::custom_str("timelock field reveal_round is missing"))?,
    ))
}

impl<R: TypeResolver> Visitor for DataVisitor<R> {
    type Value<'scale, 'resolver> = Data;
    type Error = Error;
    type TypeResolver = R;

    fn visit_variant<'scale, 'resolver>(
        self,
        value: &mut Variant<'scale, 'resolver, R>,
        _type_id: R::TypeId,
    ) -> Result<Data, Error> {
        let name = value.name();
        let fields = value.fields();
        if let Some(length) = name
            .strip_prefix("Raw")
            .and_then(|suffix| suffix.parse::<usize>().ok())
        {
            if length > 128 || name != format!("Raw{length}") {
                return Err(Error::custom_str("invalid commitment raw variant name"));
            }
            let raw = first::<Vec<u8>, R>(fields)?;
            if raw.len() != length {
                return Err(Error::custom_str(
                    "commitment raw payload length differs from metadata name",
                ));
            }
            return Ok(Data::Raw(raw));
        }
        match name {
            "None" => Ok(Data::None),
            "ResetBondsFlag" => Ok(Data::ResetBondsFlag),
            "BlakeTwo256" => Ok(Data::BlakeTwo256(first::<[u8; 32], R>(fields)?)),
            "Sha256" => Ok(Data::Sha256(first::<[u8; 32], R>(fields)?)),
            "Keccak256" => Ok(Data::Keccak256(first::<[u8; 32], R>(fields)?)),
            "ShaThree256" => Ok(Data::ShaThree256(first::<[u8; 32], R>(fields)?)),
            "BigRaw" => Ok(Data::BigRaw(first::<Vec<u8>, R>(fields)?)),
            "TimelockEncrypted" | "TimelockRevealFailed" => {
                let (encrypted, reveal_round) = timelock(fields)?;
                Ok(if name == "TimelockEncrypted" {
                    Data::TimelockEncrypted {
                        encrypted,
                        reveal_round,
                    }
                } else {
                    Data::TimelockRevealFailed {
                        encrypted,
                        reveal_round,
                    }
                })
            }
            _ => Err(Error::custom_string(format!(
                "unknown commitment variant {name}"
            ))),
        }
    }
}

impl IntoVisitor for Data {
    type AnyVisitor<R: TypeResolver> = DataVisitor<R>;
    fn into_visitor<R: TypeResolver>() -> Self::AnyVisitor<R> {
        DataVisitor(PhantomData)
    }
}

#[cfg(test)]
mod tests {
    use parity_scale_codec::Encode;
    use rtao_core::{codec::decode::Cursor, runtime::Runtime};
    use scale_decode::DecodeAsType;

    use super::*;

    fn runtime() -> Result<Runtime, Box<dyn std::error::Error>> {
        Ok(Runtime::parse(
            include_bytes!("../../../../vendor/metadata/finney/478.v15.scale"),
            478,
            1,
            42,
        )?)
    }

    fn data_id(runtime: &Runtime) -> Result<u32, Box<dyn std::error::Error>> {
        runtime
            .types
            .types
            .iter()
            .find(|ty| {
                ty.ty
                    .path
                    .segments
                    .first()
                    .is_some_and(|name| name == "pallet_commitments")
                    && ty
                        .ty
                        .path
                        .segments
                        .last()
                        .is_some_and(|name| name == "Data")
            })
            .map(|ty| ty.id)
            .ok_or_else(|| "commitment Data is absent from pinned metadata".into())
    }

    fn assert_decodes_and_encodes(
        runtime: &Runtime,
        id: u32,
        bytes: &[u8],
        expected: Data,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut remaining = bytes;
        let decoded = Data::decode_as_type(&mut remaining, id, &runtime.types)?;
        assert_eq!(decoded, expected);
        assert!(remaining.is_empty());
        let mut cursor = Cursor::new(bytes);
        assert_eq!(decoded.to_dynamic(), runtime.decode_id(id, &mut cursor)?);
        assert_eq!(cursor.remaining(), 0);
        let mut encoded = Vec::new();
        runtime.encode_id(id, &decoded.into_core_value(), &mut encoded)?;
        assert_eq!(encoded, bytes);
        Ok(())
    }

    #[test]
    fn commitments_raw0_through_raw128_match_real_metadata_and_core()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = runtime()?;
        let id = data_id(&runtime)?;
        for length in 0..=128u8 {
            let payload: Vec<_> = (0..length).map(|index| index.wrapping_mul(37)).collect();
            let mut bytes = vec![length + 1];
            bytes.extend_from_slice(&payload);
            assert_decodes_and_encodes(&runtime, id, &bytes, Data::Raw(payload))?;
        }
        Ok(())
    }

    #[test]
    fn commitment_hash_flags_and_literal_hex_text_round_trip()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = runtime()?;
        let id = data_id(&runtime)?;
        assert_decodes_and_encodes(&runtime, id, &[0], Data::None)?;
        assert_decodes_and_encodes(&runtime, id, &[135], Data::ResetBondsFlag)?;
        for (index, data) in [
            (130, Data::BlakeTwo256([0xa5; 32])),
            (131, Data::Sha256([0xa5; 32])),
            (132, Data::Keccak256([0xa5; 32])),
            (133, Data::ShaThree256([0xa5; 32])),
        ] {
            let mut bytes = vec![index];
            bytes.extend_from_slice(&[0xa5; 32]);
            assert_decodes_and_encodes(&runtime, id, &bytes, data)?;
        }
        let payload = b"0x00".to_vec();
        let mut big_raw = vec![136];
        big_raw.extend(payload.encode());
        assert_decodes_and_encodes(&runtime, id, &big_raw, Data::BigRaw(payload.clone()))?;
        for index in [134, 137] {
            let mut bytes = vec![index];
            bytes.extend(payload.encode());
            bytes.extend(u64::MAX.encode());
            let expected = if index == 134 {
                Data::TimelockEncrypted {
                    encrypted: payload.clone(),
                    reveal_round: u64::MAX,
                }
            } else {
                Data::TimelockRevealFailed {
                    encrypted: payload.clone(),
                    reveal_round: u64::MAX,
                }
            };
            assert_decodes_and_encodes(&runtime, id, &bytes, expected)?;
        }
        Ok(())
    }

    #[test]
    fn malformed_commitment_variants_fail_without_losing_cursor_bounds()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = runtime()?;
        let id = data_id(&runtime)?;
        for bytes in [vec![138], vec![2], vec![130, 1], vec![134, 0]] {
            assert!(Data::decode_as_type(&mut bytes.as_slice(), id, &runtime.types).is_err());
            assert!(runtime.decode_id(id, &mut Cursor::new(&bytes)).is_err());
        }
        Ok(())
    }
}
