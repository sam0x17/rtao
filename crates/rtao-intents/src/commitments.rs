// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/commitments.py @ a68c6e8c65411a842fbf69269dc385aa13041391 (Apache-2.0); modified by rtao.
//! Typed `Commitments.set_commitment` (D-114) and channel burn configuration.
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
use rtao_primitives::{
    Amount, Balance, DynBalance, Money, NetUid, Spend, Tao, TaoBalance, U64F64, UnitError,
    hyperparams::burn_increase_multiplier,
};
use rtao_runtime::calls::{admin_utils, commitments as calls};
use rtao_runtime::models::{CommitmentInfo, Data};
use rtao_runtime::runtime_apis::subnet_registration_runtime_api::GET_COMMIT_BURN;
use rtao_tx::timelock::{Reveal, seal_at};
use rtao_tx::{BuildCx, Built, Intent, IntentError, IntentInterface, PreflightCx};
use serde_json::{Value, json};

pub use crate::{SetChannelCommitment, SetCommitBurn};

impl SetChannelCommitment {
    fn max_burn(&self) -> Result<Balance<Tao>, IntentError> {
        match &self.max_burn_tao {
            Amount::Exact(amount) => Ok(amount.resolve(NetUid::ROOT)?.tao()?),
            Amount::All => Err(UnitError::AllNotAllowed.into()),
        }
    }

    fn payload(&self) -> Result<Vec<u8>, IntentError> {
        let bytes = match self.encoding.as_str() {
            "utf8" => self.data.as_bytes().to_vec(),
            "hex" => rtao_primitives::pyfmt::bytes_fromhex(
                self.data.strip_prefix("0x").unwrap_or(&self.data),
            )
            .map_err(|_| {
                IntentError::validation(
                    "Commitment data must be valid hexadecimal with encoding='hex'.",
                )
            })?,
            _ => {
                return Err(IntentError::validation(
                    "Commitment encoding must be 'utf8' or 'hex'.",
                ));
            }
        };
        if self.reveal_round.is_some() {
            if bytes.is_empty() {
                return Err(IntentError::validation(
                    "A timelocked commitment cannot have an empty payload.",
                ));
            }
        } else if bytes.len() > MAX_BIG_RAW_BYTES {
            return Err(IntentError::validation(
                "A raw channel commitment is limited to 512 bytes. Use timelock encryption or a content hash.",
            ));
        }
        Ok(bytes)
    }
}

impl Intent for SetChannelCommitment {
    fn summary(&self) -> String {
        let mode = if self.reveal_round.is_some() {
            "timelocked"
        } else {
            "plaintext"
        };
        format!(
            "publish {mode} commitment on netuid {}, mechanism {}, channel {}",
            self.netuid, self.mechid, self.channel
        )
    }

    fn validate(&self) -> Result<(), IntentError> {
        self.max_burn()?;
        self.payload()?;
        Ok(())
    }

    fn spend(&self) -> Spend {
        self.max_burn().map_or(Spend::Unbounded, Spend::Bounded)
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let max_burn = self.max_burn()?;
        let payload = self.payload()?;
        let hotkey = cx.hotkey_address(
            self.hotkey_ss58
                .as_ref()
                .map(rtao_primitives::Ss58Address::account),
        )?;
        let fields = match self.reveal_round {
            Some(round) => {
                let seal = seal_at(payload, Reveal::Round(round), cx.clock.now())
                    .await
                    .map_err(|error| IntentError::validation(error.to_string()))?;
                vec![
                    seal.commitment_field()
                        .await
                        .map_err(|error| IntentError::validation(error.to_string()))?,
                ]
            }
            None if payload.is_empty() => vec![],
            None if payload.len() <= MAX_RAW_BYTES => vec![Data::Raw(payload)],
            None => vec![Data::BigRaw(payload)],
        };
        let call = calls::set_commitment_to(
            self.netuid,
            self.mechid,
            self.channel,
            hotkey,
            CommitmentInfo { fields },
            TaoBalance(max_burn.rao()),
        );
        let mut built = Built::new(call.compose(cx.runtime)?);
        if let Some(round) = self.reveal_round {
            built.extras.insert("reveal_round".into(), json!(round));
        }
        Ok(built)
    }

    async fn effects<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        let quote = cx
            .view
            .call(&GET_COMMIT_BURN, (self.netuid, self.mechid, self.channel))
            .await?;
        let maximum = format!(
            "maximum accepted burn: {}",
            DynBalance::from(self.max_burn()?)
        );
        let mut lines = vec![self.summary()];
        if let Some(quote) = quote {
            let quote = quote.to_json();
            let burn = quote
                .get("burn")
                .and_then(|value| {
                    value.as_u64().or_else(|| {
                        value
                            .get("amount")
                            .or_else(|| value.get("value"))
                            .and_then(Value::as_u64)
                    })
                })
                .ok_or_else(|| {
                    IntentError::validation("The commitment burn quote has no native TAO amount.")
                })?;
            let price = DynBalance::from_rao(burn, NetUid::ROOT);
            lines.push(format!(
                "current commitment burn: {price} (the execution-time price may differ)"
            ));
            lines.push(maximum);
        } else {
            lines.push(maximum);
            lines.push(
                "commitment burn is disabled or unconfigured; only the transaction fee applies"
                    .into(),
            );
        }
        Ok(lines)
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(vec!["max_burn is only a price limit, not a tip: the runtime charges its current decayed price and never retains the unused difference".into()])
    }
}

enum BurnParameter {
    Enabled(bool),
    Minimum(TaoBalance),
    Maximum(TaoBalance),
    HalfLife(u16),
    Increase(U64F64),
}

impl SetCommitBurn {
    fn parameter(&self) -> Result<BurnParameter, IntentError> {
        let value = &self.value;
        match self.name.as_str() {
            "enabled" => {
                let enabled = match value {
                    Value::Bool(enabled) => Some(*enabled),
                    Value::Number(number) => match number.as_u64() {
                        Some(0) => Some(false),
                        Some(1) => Some(true),
                        _ => None,
                    },
                    Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
                        "true" | "yes" | "1" => Some(true),
                        "false" | "no" | "0" => Some(false),
                        _ => None,
                    },
                    _ => None,
                };
                enabled.map(BurnParameter::Enabled).ok_or_else(|| {
                    IntentError::validation("Commit-burn enabled must be true/false or 1/0.")
                })
            }
            "min_burn" | "max_burn" => {
                let amount: Money = serde_json::from_value(value.clone())
                    .map_err(|err| IntentError::validation(err.to_string()))?;
                let amount = TaoBalance(amount.rao()?);
                Ok(match self.name.as_str() {
                    "min_burn" => BurnParameter::Minimum(amount),
                    _ => BurnParameter::Maximum(amount),
                })
            }
            "half_life" => {
                let count = match value {
                    Value::Number(number) => number.as_u64().and_then(|n| u16::try_from(n).ok()),
                    Value::String(text) => {
                        let text = text.trim();
                        text.parse::<u16>().ok().filter(|n| n.to_string() == text)
                    }
                    _ => None,
                };
                count
                    .filter(|n| *n > 0)
                    .map(BurnParameter::HalfLife)
                    .ok_or_else(|| {
                        IntentError::validation("half_life must be an integer in 1..65535 blocks.")
                    })
            }
            "increase_mult" => {
                let multiplier = match value {
                    Value::Number(number) if number.is_f64() => number.as_f64().and_then(|value| {
                        burn_increase_multiplier(&rtao_primitives::pyfmt::float_repr(value))
                    }),
                    Value::Number(number) => number
                        .as_u128()
                        .filter(|bits| ((1u128 << 64)..=(3u128 << 64)).contains(bits))
                        .map(U64F64::from_bits),
                    Value::String(text) => burn_increase_multiplier(text),
                    _ => None,
                };
                multiplier.map(BurnParameter::Increase).ok_or_else(|| {
                    IntentError::validation(
                        "increase_mult must be a decimal in 1..3 or an exact raw U64F64 integer.",
                    )
                })
            }
            other => Err(IntentError::validation(format!(
                "Unknown commit-burn parameter {other:?}. Choose enabled, min_burn, max_burn, half_life, or increase_mult."
            ))),
        }
    }
}

impl Intent for SetCommitBurn {
    fn summary(&self) -> String {
        let value = match self.parameter() {
            Ok(BurnParameter::Enabled(true)) => "True".to_owned(),
            Ok(BurnParameter::Enabled(false)) => "False".to_owned(),
            Ok(BurnParameter::Minimum(value) | BurnParameter::Maximum(value)) => {
                value.0.to_string()
            }
            Ok(BurnParameter::HalfLife(value)) => value.to_string(),
            Ok(BurnParameter::Increase(value)) => value.bits.to_string(),
            Err(_) => self.value.to_string(),
        };
        format!(
            "set commit burn {}={value} on netuid {}, mechanism {}, channel {}",
            self.name, self.netuid, self.mechid, self.channel
        )
    }

    fn validate(&self) -> Result<(), IntentError> {
        self.parameter().map(|_| ())
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let (netuid, mechid, channel) = (self.netuid, self.mechid, self.channel);
        let call = match self.parameter()? {
            BurnParameter::Enabled(value) => {
                admin_utils::sudo_set_commit_burn_enabled(netuid, mechid, channel, value)
            }
            BurnParameter::Minimum(value) => {
                admin_utils::sudo_set_commit_burn_min(netuid, mechid, channel, value)
            }
            BurnParameter::Maximum(value) => {
                admin_utils::sudo_set_commit_burn_max(netuid, mechid, channel, value)
            }
            BurnParameter::HalfLife(value) => {
                admin_utils::sudo_set_commit_burn_half_life(netuid, mechid, channel, value)
            }
            BurnParameter::Increase(value) => {
                admin_utils::sudo_set_commit_burn_increase_mult(netuid, mechid, channel, value)
            }
        };
        Ok(Built::new(call.compose(cx.runtime)?))
    }
}

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

    #[tokio::test]
    async fn channel_commitment_uses_coldkey_policy_and_pinned_target_fields() -> anyhow::Result<()>
    {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let coldkey = AccountId([8; 32]);
        struct Wallet;
        impl rtao_signer::KeyView for Wallet {
            fn hotkey_address(&self) -> Result<AccountId, rtao_signer::SignerError> {
                Ok(HOTKEY)
            }
            fn hotkey_public_key(&self) -> Result<[u8; 32], rtao_signer::SignerError> {
                Ok(HOTKEY.0)
            }
            fn coldkey_address(&self) -> Result<AccountId, rtao_signer::SignerError> {
                Ok(AccountId([8; 32]))
            }
        }
        let cases = [
            ("".to_owned(), "utf8", vec![], None),
            (
                "é".to_owned(),
                "utf8",
                vec![Data::Raw(vec![0xc3, 0xa9])],
                None,
            ),
            (
                "0x00 ff".to_owned(),
                "hex",
                vec![Data::Raw(vec![0, 255])],
                Some(AccountId([6; 32])),
            ),
            (
                "x".repeat(128),
                "utf8",
                vec![Data::Raw(vec![b'x'; 128])],
                None,
            ),
            (
                "y".repeat(129),
                "utf8",
                vec![Data::BigRaw(vec![b'y'; 129])],
                None,
            ),
            (
                "z".repeat(512),
                "utf8",
                vec![Data::BigRaw(vec![b'z'; 512])],
                None,
            ),
        ];
        let before = chain.requests().len();
        for (data, encoding, fields, explicit) in cases {
            let args = json!({"netuid":3, "mechid":2, "channel":7, "data":data, "encoding":encoding,
                "max_burn_tao":"0.5", "hotkey_ss58":explicit.map(|id| id.to_ss58(42))});
            let crate::AnyIntent::SetChannelCommitment(intent) =
                crate::AnyIntent::from_tool("set_channel_commitment", args)?
            else {
                anyhow::bail!("expected the typed channel commitment intent");
            };
            let origin =
                explicit.map_or(OriginView::Keys(&Wallet), |_| OriginView::Account(coldkey));
            let built = intent
                .build(&BuildCx {
                    view: &snapshot,
                    origin,
                    runtime,
                    clock: &SystemClock,
                    spec: SetChannelCommitment::SPEC,
                })
                .await?;
            let expected = calls::set_commitment_to(
                NetUid(3),
                rtao_primitives::MechId(2),
                7,
                explicit.unwrap_or(HOTKEY),
                CommitmentInfo { fields },
                TaoBalance(500_000_000),
            )
            .compose(runtime)?;
            assert_eq!(built.call.as_bytes(), expected.as_bytes());
            assert_eq!(&built.call.as_bytes()[..6], &[18, 3, 3, 0, 2, 7]);
            assert_eq!(&built.call.as_bytes()[6..38], &explicit.unwrap_or(HOTKEY).0);
            assert_eq!(
                intent.spend(),
                Spend::Bounded(Balance::from_rao(500_000_000))
            );
            assert!(built.extras.is_empty());
        }
        assert_eq!(chain.requests().len(), before);
        for invalid in [
            json!({"netuid":3,"encoding":"base64"}),
            json!({"netuid":3,"encoding":"hex","data":"0 x"}),
            json!({"netuid":3,"data":"x".repeat(513)}),
            json!({"netuid":3,"reveal_round":5}),
            json!({"netuid":3,"max_burn_tao":"all"}),
            json!({"netuid":3,"max_burn_tao":-1}),
            json!({"netuid":3,"channel":256}),
        ] {
            assert!(crate::AnyIntent::from_tool("set_channel_commitment", invalid).is_err());
        }
        assert_eq!(SetChannelCommitment::SPEC.signer, "coldkey");
        assert_eq!(
            SetChannelCommitment::SPEC.verify,
            Some("channel_commitment")
        );
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn channel_commitment_builds_native_timelock_and_reads_the_pinned_burn_quote()
    -> anyhow::Result<()> {
        use std::time::{Duration, SystemTime, UNIX_EPOCH};
        struct Clock;
        impl rtao_tx::Clock for Clock {
            fn now(&self) -> SystemTime {
                UNIX_EPOCH + Duration::from_secs(1_752_076_800)
            }
        }
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let coldkey = AccountId([8; 32]);
        let round = rtao_tx::timelock::current_round(Some(1_752_076_800.0))? + 20;
        let crate::AnyIntent::SetChannelCommitment(mut intent) = crate::AnyIntent::from_tool(
            "set_channel_commitment",
            json!({"netuid":3, "mechid":2, "channel":7, "data":"trial", "reveal_round":round,
                   "hotkey_ss58":HOTKEY.to_ss58(42), "max_burn_tao":"0.5"}),
        )?
        else {
            anyhow::bail!("expected the typed channel commitment intent");
        };
        let context = BuildCx {
            view: &snapshot,
            origin: OriginView::Account(coldkey),
            runtime,
            clock: &Clock,
            spec: SetChannelCommitment::SPEC,
        };
        let built = intent.build(&context).await?;
        let decoded = decode(&built.call, runtime)?;
        assert_eq!(decoded["call_function"], "set_commitment_to");
        let encrypted = &decoded["call_args"][4]["value"]["fields"][0]["TimelockEncrypted"];
        assert_eq!(encrypted["reveal_round"], round, "{decoded}");
        let ciphertext = encrypted["encrypted"]
            .as_str()
            .and_then(|text| text.strip_prefix("0x"))
            .and_then(|text| hex::decode(text).ok())
            .ok_or_else(|| anyhow::anyhow!("invalid inner ciphertext"))?;
        assert!(ciphertext.ends_with(b"AES_GCM_"));
        assert_eq!(built.extras.get("reveal_round"), Some(&json!(round)));

        let preview = PreflightCx {
            view: &snapshot,
            runtime,
            clock: &Clock,
            spec: SetChannelCommitment::SPEC,
            client: &client,
            dispatch_origin: coldkey,
            fee_payer: coldkey,
            call: &built.call,
        };
        for (quote, expected) in [
            (
                json!({"enabled":true,"burn":2_000_000_000u64,"min_burn":500_000_000u64,
                  "max_burn":10_000_000_000u64,"half_life":360,
                  "increase_mult":{"bits":5u128<<62},"last_update_block":100}),
                "current commitment burn",
            ),
            (Value::Null, "burn is disabled or unconfigured"),
        ] {
            chain.mutate_state(|state| {
                state.seed_runtime("SubnetRegistrationRuntimeApi", "get_commit_burn", quote)
            })?;
            let effects = intent.effects(&preview).await?;
            assert!(effects.iter().any(|line| line.contains(expected)));
            assert!(
                effects
                    .iter()
                    .any(|line| line == "maximum accepted burn: τ0.500000000")
            );
            assert_eq!(intent.warnings(&preview).await?.len(), 1);
        }
        let hash = format!("0x{}", hex::encode(snapshot.block().hash.0));
        assert!(
            chain
                .requests()
                .iter()
                .any(|request| request.method == "state_call"
                    && request.params
                        == json!([
                            "SubnetRegistrationRuntimeApi_get_commit_burn",
                            "0x03000207",
                            hash
                        ]))
        );
        intent.reveal_round = Some(round - 20);
        assert!(intent.build(&context).await.is_err());
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn commit_burn_uses_owner_calls_with_exact_amounts_and_multiplier_bits()
    -> anyhow::Result<()> {
        let (chain, _client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let calls = chain.requests().len();
        const ONE: u128 = 1 << 64;
        let cases = [
            ("enabled", json!(" YES "), 123, vec![1], "True".to_owned()),
            ("enabled", json!(0), 123, vec![0], "False".to_owned()),
            (
                "min_burn",
                json!("0.5"),
                124,
                500_000_000u64.to_le_bytes().to_vec(),
                "500000000".to_owned(),
            ),
            (
                "max_burn",
                json!(2),
                125,
                2_000_000_000u64.to_le_bytes().to_vec(),
                "2000000000".to_owned(),
            ),
            (
                "half_life",
                json!("360"),
                126,
                360u16.to_le_bytes().to_vec(),
                "360".to_owned(),
            ),
            (
                "increase_mult",
                json!(1.25),
                127,
                (5 * (ONE / 4)).to_le_bytes().to_vec(),
                (5 * (ONE / 4)).to_string(),
            ),
            (
                "increase_mult",
                json!("1.26"),
                127,
                (63 * ONE / 50).to_le_bytes().to_vec(),
                (63 * ONE / 50).to_string(),
            ),
            (
                "increase_mult",
                json!("1.0000000000000000001"),
                127,
                (ONE + 1).to_le_bytes().to_vec(),
                (ONE + 1).to_string(),
            ),
            (
                "increase_mult",
                json!(3 * ONE),
                127,
                (3 * ONE).to_le_bytes().to_vec(),
                (3 * ONE).to_string(),
            ),
        ];
        for (name, value, call_index, payload, summary_value) in cases {
            let args = json!({"netuid":3, "mechid":2, "channel":7, "name":name, "value":value});
            let crate::AnyIntent::SetCommitBurn(intent) =
                crate::AnyIntent::from_tool("set_commit_burn", args)?
            else {
                anyhow::bail!("expected the typed commit burn intent");
            };
            let built = intent
                .build(&BuildCx {
                    view: &snapshot,
                    origin: OriginView::Account(HOTKEY),
                    runtime,
                    clock: &SystemClock,
                    spec: SetCommitBurn::SPEC,
                })
                .await?;
            let mut expected = vec![19, call_index, 3, 0, 2, 7];
            expected.extend(payload);
            assert_eq!(built.call.as_bytes(), expected);
            assert_eq!(
                intent.summary(),
                format!(
                    "set commit burn {name}={summary_value} on netuid 3, mechanism 2, channel 7"
                )
            );
            assert_eq!(intent.spend(), rtao_primitives::Spend::None);
        }
        for (name, value) in [
            ("unknown", json!(0)),
            ("enabled", json!(1.0)),
            ("enabled", json!("on")),
            ("min_burn", json!(-1)),
            ("max_burn", json!("18446744073.709551616")),
            ("half_life", json!(0)),
            ("half_life", json!(360.0)),
            ("half_life", json!("0360")),
            ("increase_mult", json!(1)),
            ("increase_mult", json!(true)),
            ("increase_mult", json!(3 * ONE + 1)),
            ("increase_mult", json!("3.00000000000000000001")),
            ("increase_mult", json!("1e-1000000000")),
            ("increase_mult", json!("Infinity")),
        ] {
            let args = json!({"netuid":3, "name":name, "value":value});
            assert!(crate::AnyIntent::from_tool("set_commit_burn", args).is_err());
        }
        assert_eq!(SetCommitBurn::SPEC.signer, "coldkey");
        assert_eq!(SetCommitBurn::SPEC.origin, "subnet_owner");
        assert_eq!(SetCommitBurn::SPEC.verify, Some("commit_burn"));
        assert_eq!(chain.requests().len(), calls);
        Ok(())
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
