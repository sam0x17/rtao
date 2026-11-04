//! Typed `Commitments.set_commitment` (D-114).
//!
//! A neuron publishes up to `Commitments.MaxFields` data fields for its
//! hotkey on a subnet: raw bytes, a hash, a timelocked ciphertext or the bonds
//! reset flag. [`SetCommitment`] is an rtao addition (the v11 Python SDK has
//! no intent for it), so it is not part of the generated Python catalog or
//! [`crate::AnyIntent`]; it runs through [`rtao_tx::Executor::execute`] like
//! any typed intent, with the hotkey as signer and the ordinary fee, nonce,
//! policy and journal handling. Timelocked fields come from
//! `rtao_tx::timelock::Timelocked::commitment_field`.

use rtao_client::{View, ViewExt};
use rtao_primitives::NetUid;
use rtao_runtime::models::{CommitmentInfo, Data};
use rtao_tx::{BuildCx, Built, Intent, IntentError, IntentInterface, PreflightCx};
use serde_json::{Value, json};

/// Most bytes a [`Data::Raw`] field holds.
pub const MAX_RAW_BYTES: usize = 128;
/// Most bytes a [`Data::BigRaw`] field holds.
pub const MAX_BIG_RAW_BYTES: usize = 512;

/// Publish this hotkey's commitment fields on a subnet.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetCommitment {
    /// Subnet the commitment belongs to.
    pub netuid: NetUid,
    /// Fields in order; they replace the hotkey's previous commitment.
    pub fields: Vec<Data>,
}

impl SetCommitment {
    /// A commitment of the given fields, checked by [`Intent::validate`].
    pub fn new(netuid: NetUid, fields: Vec<Data>) -> Result<Self, IntentError> {
        let intent = Self { netuid, fields };
        intent.validate()?;
        Ok(intent)
    }
    /// A commitment of one raw field: [`Data::Raw`] up to [`MAX_RAW_BYTES`],
    /// [`Data::BigRaw`] up to [`MAX_BIG_RAW_BYTES`].
    pub fn raw(netuid: NetUid, bytes: impl Into<Vec<u8>>) -> Result<Self, IntentError> {
        let bytes = bytes.into();
        let field = if bytes.len() <= MAX_RAW_BYTES {
            Data::Raw(bytes)
        } else {
            Data::BigRaw(bytes)
        };
        Self::new(netuid, vec![field])
    }
}

// rtao's own record, not a Python catalog entry: Python has no such intent.
static SET_COMMITMENT_SPEC: rtao_catalog::IntentSpec = rtao_catalog::IntentSpec {
    name: "set_commitment",
    summary: "Publish this hotkey's commitment fields on a subnet",
    description: "Commitments.set_commitment signed by the hotkey: up to MaxFields raw, hash, timelocked or flag fields that replace the hotkey's previous commitment on the subnet. An rtao addition with no Python intent.",
    python_class: "rtao_intents.commitments.SetCommitment",
    signer: "hotkey",
    origin: "signed",
    verify: None,
    wraps: &[("Commitments", "set_commitment")],
    fields: &[],
    all_amount_fields: &[],
    mev_shield_default: false,
    mev_shield_required: false,
    catalog_json: "{}",
    class_vars_json: "{}",
};

impl IntentInterface for SetCommitment {
    const SPEC: &'static rtao_catalog::IntentSpec = &SET_COMMITMENT_SPEC;
    type Role = rtao_signer::Hotkey;
    type Origin = rtao_tx::Signed;
    type Output = ();
    const WRAPS: &'static [&'static rtao_runtime::CallDescriptor] =
        &[&rtao_runtime::calls::commitments::SET_COMMITMENT];
    fn to_json(&self) -> Value {
        json!({
            "op": SET_COMMITMENT_SPEC.name,
            "netuid": self.netuid.0,
            "fields": serde_json::to_value(&self.fields).unwrap_or(Value::Null),
        })
    }
}

fn field_label(field: &Data) -> String {
    match field {
        Data::None => "empty".into(),
        Data::Raw(bytes) => format!("{} raw bytes", bytes.len()),
        Data::BigRaw(bytes) => format!("{} raw bytes (big)", bytes.len()),
        Data::BlakeTwo256(_) => "BLAKE2b-256 hash".into(),
        Data::Sha256(_) => "SHA-256 hash".into(),
        Data::Keccak256(_) => "Keccak-256 hash".into(),
        Data::ShaThree256(_) => "SHA3-256 hash".into(),
        Data::TimelockEncrypted { reveal_round, .. } => {
            format!("timelocked until drand round {reveal_round}")
        }
        Data::ResetBondsFlag => "bonds reset flag".into(),
        Data::TimelockRevealFailed { .. } => "failed timelock reveal".into(),
    }
}

impl Intent for SetCommitment {
    fn summary(&self) -> String {
        let fields = self.fields.iter().map(field_label).collect::<Vec<_>>();
        format!(
            "set this hotkey's commitment on subnet {} to [{}]",
            self.netuid.0,
            fields.join(", ")
        )
    }

    fn validate(&self) -> Result<(), IntentError> {
        for (index, field) in self.fields.iter().enumerate() {
            match field {
                Data::Raw(bytes) if bytes.len() > MAX_RAW_BYTES => {
                    return Err(IntentError::validation(format!(
                        "commitment field {index}: a raw field holds at most {MAX_RAW_BYTES} bytes, got {}; use BigRaw up to {MAX_BIG_RAW_BYTES}",
                        bytes.len()
                    )));
                }
                Data::BigRaw(bytes) if bytes.len() > MAX_BIG_RAW_BYTES => {
                    return Err(IntentError::validation(format!(
                        "commitment field {index}: a big raw field holds at most {MAX_BIG_RAW_BYTES} bytes, got {}",
                        bytes.len()
                    )));
                }
                Data::TimelockEncrypted { encrypted, .. } if encrypted.is_empty() => {
                    return Err(IntentError::validation(format!(
                        "commitment field {index}: a timelocked field needs ciphertext"
                    )));
                }
                Data::TimelockRevealFailed { .. } => {
                    return Err(IntentError::validation(format!(
                        "commitment field {index}: a failed timelock reveal is recorded by the chain, not submitted"
                    )));
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        let call = rtao_runtime::calls::commitments::set_commitment(
            self.netuid,
            CommitmentInfo {
                fields: self.fields.clone(),
            },
        );
        Ok(Built::new(call.compose(cx.runtime)?))
    }

    /// Refuse more fields than the live `Commitments.MaxFields`.
    async fn blocks<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        let maximum = cx
            .view
            .constant(&rtao_runtime::constants::commitments::MAX_FIELDS)
            .await?;
        Ok(
            (u32::try_from(self.fields.len()).unwrap_or(u32::MAX) > maximum)
                .then(|| {
                    format!(
                        "{} commitment fields exceed the chain's maximum of {maximum}",
                        self.fields.len()
                    )
                })
                .into_iter()
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtao_client::{BlockRef, Client, ClientConfig, Network, Snapshot};
    use rtao_primitives::AccountId;
    use rtao_runtime::{CallBytes, RuntimeHandle};
    use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
    use rtao_tx::{OriginView, SystemClock};

    const HOTKEY: AccountId = AccountId([5; 32]);

    async fn fixture() -> anyhow::Result<(FakeChain, Client, Snapshot)> {
        let chain = FakeChain::from_state(FakeState::try_default()?);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
        let snapshot = client.at(BlockRef::Finalized).await?;
        Ok((chain, client, snapshot))
    }

    fn decode(call: &CallBytes, runtime: &RuntimeHandle) -> anyhow::Result<serde_json::Value> {
        let decoded = runtime
            .core()
            .decode_spec(&rtao_core::runtime::TypeSpec::Call, call.as_bytes(), true)
            .map_err(rtao_core::CoreError::from)?;
        Ok(rtao_core::codec::value::to_corpus_json(&decoded))
    }

    #[test]
    fn constructors_pick_the_raw_variant_and_validate_lengths() -> anyhow::Result<()> {
        let small = SetCommitment::raw(NetUid(3), vec![1u8; MAX_RAW_BYTES])?;
        assert_eq!(small.fields, vec![Data::Raw(vec![1; MAX_RAW_BYTES])]);
        let big = SetCommitment::raw(NetUid(3), vec![2u8; MAX_RAW_BYTES + 1])?;
        assert!(matches!(&big.fields[0], Data::BigRaw(bytes) if bytes.len() == 129));
        assert!(SetCommitment::raw(NetUid(3), vec![0u8; MAX_BIG_RAW_BYTES + 1]).is_err());
        for invalid in [
            Data::Raw(vec![0; MAX_RAW_BYTES + 1]),
            Data::TimelockEncrypted {
                encrypted: vec![],
                reveal_round: 1,
            },
            Data::TimelockRevealFailed {
                encrypted: vec![1],
                reveal_round: 1,
            },
        ] {
            assert!(SetCommitment::new(NetUid(3), vec![invalid]).is_err());
        }
        let intent = SetCommitment::new(
            NetUid(3),
            vec![
                Data::Sha256([7; 32]),
                Data::TimelockEncrypted {
                    encrypted: vec![9],
                    reveal_round: 42,
                },
            ],
        )?;
        assert_eq!(
            intent.summary(),
            "set this hotkey's commitment on subnet 3 to [SHA-256 hash, timelocked until drand round 42]"
        );
        let json = intent.to_json();
        assert_eq!(json["op"], "set_commitment");
        assert_eq!(json["netuid"], 3);
        assert_eq!(
            serde_json::from_value::<SetCommitment>(serde_json::json!({
                "netuid": json["netuid"],
                "fields": json["fields"],
            }))?,
            intent
        );
        assert_eq!(intent.touches_netuids().as_slice(), &[NetUid(3)]);
        assert_eq!(SetCommitment::SPEC.signer, "hotkey");
        Ok(())
    }

    #[tokio::test]
    async fn builds_the_commitments_call_without_reads_and_blocks_extra_fields()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let intent = SetCommitment::new(
            NetUid(3),
            vec![Data::Raw(b"model:v1".to_vec()), Data::ResetBondsFlag],
        )?;
        let requests = chain.requests().len();
        let built = intent
            .build(&BuildCx {
                view: &snapshot,
                origin: OriginView::Account(HOTKEY),
                runtime,
                clock: &SystemClock,
                spec: SetCommitment::SPEC,
            })
            .await?;
        assert_eq!(chain.requests().len(), requests, "building reads nothing");
        let decoded = decode(&built.call, runtime)?;
        assert_eq!(decoded["call_module"], "Commitments");
        assert_eq!(decoded["call_function"], "set_commitment");
        assert_eq!(decoded["call_args"][0]["value"], 3);
        assert_eq!(
            CallBytes::decode(built.call.as_bytes(), runtime)?,
            built.call
        );
        let maximum = snapshot
            .constant(&rtao_runtime::constants::commitments::MAX_FIELDS)
            .await?;
        let preflight = |intent: SetCommitment| {
            let snapshot = snapshot.clone();
            let client = client.clone();
            let call = built.call.clone();
            async move {
                intent
                    .blocks(&PreflightCx {
                        view: &snapshot,
                        runtime: &snapshot.block().runtime,
                        clock: &SystemClock,
                        spec: SetCommitment::SPEC,
                        client: &client,
                        dispatch_origin: HOTKEY,
                        fee_payer: HOTKEY,
                        call: &call,
                    })
                    .await
            }
        };
        assert!(preflight(intent.clone()).await?.is_empty());
        let too_many =
            SetCommitment::new(NetUid(3), vec![Data::None; usize::try_from(maximum)? + 1])?;
        let blocks = preflight(too_many).await?;
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].contains("maximum"), "{blocks:?}");
        client.close().await;
        Ok(())
    }
}
