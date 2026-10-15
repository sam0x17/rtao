// Derived from RaoFoundation/subtensor sdk/python/bittensor/hyperparams.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Hyperparameter vocabulary and shared raw-value conversion.

mod planner;
pub use planner::{HparamPlan, HyperparamPlanner, WindowWait, owner_rate_key};

pub use crate::SetHyperparameter;
/// Extracted descriptor for a hyperparameter's units, description and bounds.
pub use rtao_primitives::hyperparams::Hyperparam as Hyperparameter;
pub use rtao_primitives::hyperparams::{HyperparamError, ProportionInput, RawInteger};

/// A raw integer, human unit value, boolean, or named consensus mode.
#[derive(Clone, Debug)]
pub struct HparamArg(pub ProportionInput<'static>);

macro_rules! hparam_input {
    ($($ty:ty),+ $(,)?) => {$(
        impl From<$ty> for HparamArg {
            fn from(value: $ty) -> Self { Self(value.into()) }
        }
    )+};
}
hparam_input!(
    u8, u16, u32, u64, u128, i8, i16, i32, i64, i128, f64, bool, String
);
impl From<&str> for HparamArg {
    fn from(value: &str) -> Self {
        Self(value.to_owned().into())
    }
}
impl From<RawInteger> for HparamArg {
    fn from(value: RawInteger) -> Self {
        Self(ProportionInput::Raw(value.into()))
    }
}

/// Resolve the authenticated owner allowlist before conversion or chain reads.
pub fn owner_parameter(
    name: &str,
) -> Result<&'static rtao_catalog::owner_map::OwnerHyperparam, rtao_tx::IntentError> {
    rtao_catalog::owner_map::OWNER_HYPERPARAMETERS
        .iter()
        .find(|row| row.name == name)
        .ok_or_else(|| {
            rtao_tx::IntentError::InvalidArgument(format!(
                "unknown or owner-unsettable hyperparameter {name:?}"
            ))
        })
}

/// Convert one setting into its SCALE value shape using the shared units table.
pub fn owner_value(
    row: &rtao_catalog::owner_map::OwnerHyperparam,
    value: HparamArg,
) -> Result<rtao_core::DynValue, rtao_tx::IntentError> {
    use rtao_catalog::owner_map::OwnerValueShape;
    use rtao_core::{DynValue, codec::Value};
    if matches!(row.shape, OwnerValueShape::EpochConsensus) {
        let ProportionInput::Text(text) = value.0 else {
            return Err(rtao_tx::IntentError::InvalidArgument(
                "epoch_consensus must be Yuma or Null".into(),
            ));
        };
        if !matches!(text.as_ref(), "Yuma" | "Null") {
            return Err(rtao_tx::IntentError::InvalidArgument(
                "epoch_consensus must be Yuma or Null".into(),
            ));
        }
        return Ok(DynValue::new(Value::Str(text.into_owned())));
    }
    let raw = to_raw(row.name, value.0)
        .map_err(|error| rtao_tx::IntentError::InvalidArgument(error.to_string()))?;
    let integer = match raw {
        RawInteger::Unsigned(n) => Value::Uint(n),
        RawInteger::Signed(n) => Value::Int(n),
    };
    let raw = match row.shape {
        OwnerValueShape::Bool => Value::Bool(raw.as_u128() == Some(1)),
        OwnerValueShape::FixedBits => Value::record(vec![("bits".into(), integer)]),
        _ => integer,
    };
    Ok(DynValue::new(raw))
}

/// Compose an owner setter while retaining the other side of an alpha pair.
pub fn owner_call(
    row: &rtao_catalog::owner_map::OwnerHyperparam,
    netuid: rtao_primitives::NetUid,
    raw: &rtao_core::DynValue,
    current: &rtao_core::DynValue,
) -> Result<rtao_runtime::Call, rtao_tx::IntentError> {
    use rtao_catalog::owner_map::OwnerValueShape;
    use rtao_core::codec::Value;
    let descriptor = rtao_runtime::calls::ALL
        .iter()
        .find(|call| call.pallet == "AdminUtils" && call.name == row.setter)
        .copied()
        .ok_or_else(|| {
            rtao_tx::IntentError::InvalidArgument(format!("missing owner call {}", row.setter))
        })?;
    let mut values = if let OwnerValueShape::AlphaPair { index } = row.shape {
        let Value::Tuple(pair) = current.as_core() else {
            return Err(rtao_tx::IntentError::InvalidArgument(
                "AlphaValues is not a pair".into(),
            ));
        };
        if pair.len() != 2 {
            return Err(rtao_tx::IntentError::InvalidArgument(
                "AlphaValues is not a pair".into(),
            ));
        }
        let mut pair = pair.clone();
        let side = pair
            .get_mut(usize::from(index))
            .ok_or_else(|| rtao_tx::IntentError::InvalidArgument("invalid alpha side".into()))?;
        *side = raw.as_core().clone();
        pair
    } else {
        vec![raw.as_core().clone()]
    };
    if values.len() != row.params.len() {
        return Err(rtao_tx::IntentError::InvalidArgument(
            "owner setter argument shape changed".into(),
        ));
    }
    let mut fields = vec![("netuid".into(), Value::Uint(u128::from(netuid.0)))];
    fields.extend(
        row.params
            .iter()
            .zip(values.drain(..))
            .map(|(name, value)| ((*name).into(), value)),
    );
    Ok(rtao_runtime::Call::new(
        descriptor,
        Value::record(fields),
        vec![],
    ))
}

/// Convert a raw or human input using the authenticated catalog table.
#[doc(alias = "bittensor.hyperparams.to_raw")]
pub fn to_raw<'a>(
    name: &str,
    value: impl Into<ProportionInput<'a>>,
) -> Result<RawInteger, HyperparamError> {
    rtao_catalog::hyperparams::TABLE.to_raw(name, value)
}

fn json_arg(value: &serde_json::Value) -> Result<HparamArg, rtao_tx::IntentError> {
    use serde_json::Value;
    match value {
        Value::Bool(value) => Ok((*value).into()),
        Value::String(value) => Ok(value.clone().into()),
        Value::Number(value) => {
            // Keep integer tokens exact, including u128 fixed-point values.
            let text = value.to_string();
            if text.contains(['.', 'e', 'E']) {
                value.as_f64().map(HparamArg::from).ok_or_else(|| {
                    rtao_tx::IntentError::Validation("hyperparameter number is out of range".into())
                })
            } else {
                Ok(text.into())
            }
        }
        _ => Err(rtao_tx::IntentError::Validation(
            "hyperparameter value must be an integer, float, string or boolean".into(),
        )),
    }
}

fn canonical_value(intent: &SetHyperparameter) -> Result<serde_json::Value, rtao_tx::IntentError> {
    owner_parameter(&intent.name)?;
    let value = json_arg(&intent.value)?;
    if intent.name == "epoch_consensus" {
        owner_value(owner_parameter(&intent.name)?, value)?;
        return Ok(intent.value.clone());
    }
    let raw = to_raw(&intent.name, value.0)
        .map_err(|error| rtao_tx::IntentError::Validation(error.to_string()))?;
    Ok(serde_json::from_str(&raw.to_string())?)
}

pub(crate) fn canonicalize(intent: &mut SetHyperparameter) -> Result<(), rtao_tx::IntentError> {
    intent.value = canonical_value(intent)?;
    Ok(())
}

pub(crate) fn intent_json(intent: &SetHyperparameter) -> serde_json::Value {
    let mut args = crate::args_with_op("set_hyperparameter", intent);
    if let Ok(raw) = canonical_value(intent) {
        args["value"] = raw;
    }
    args
}

pub(crate) fn planner_error(error: rtao_tx::TxError) -> rtao_tx::IntentError {
    use rtao_tx::{IntentError, TxError};
    match error {
        TxError::Intent(error) => error,
        TxError::Chain(error) => IntentError::Chain(error),
        TxError::Signer(error) => IntentError::Signer(error),
        TxError::Rpc(error) => IntentError::Client(rtao_client::ClientError::Rpc(error)),
        error => IntentError::Chain(rtao_tx::ChainError::with_code(
            error.to_string(),
            None,
            error.code(),
        )),
    }
}

impl rtao_tx::Intent for SetHyperparameter {
    fn summary(&self) -> String {
        let value = canonical_value(self).unwrap_or_else(|_| self.value.clone());
        let value = value
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| value.to_string());
        format!(
            "set hyperparameter {}={value} on netuid {}",
            self.name, self.netuid
        )
    }

    async fn build<V: rtao_client::View + ?Sized>(
        &self,
        cx: &rtao_tx::BuildCx<'_, V>,
    ) -> Result<rtao_tx::Built, rtao_tx::IntentError> {
        let row = owner_parameter(&self.name)?;
        let raw = owner_value(row, json_arg(&self.value)?)?;
        // Read only the paired setting needed by Python's build. Owner/window
        // admission belongs to preflight and shares the public planner below.
        let current = if matches!(
            row.shape,
            rtao_catalog::owner_map::OwnerValueShape::AlphaPair { .. }
        ) {
            use rtao_client::ViewExt;
            let snapshot = cx.view.pin().await?;
            snapshot
                .storage_dyn(
                    row.current.pallet,
                    row.current.name,
                    &[rtao_core::codec::Value::Uint(u128::from(self.netuid.0))],
                )
                .await?
        } else {
            raw.clone()
        };
        Ok(rtao_tx::Built::new(
            owner_call(row, self.netuid, &raw, &current)?.compose(cx.runtime)?,
        ))
    }

    async fn blocks<V: rtao_client::View + ?Sized>(
        &self,
        cx: &rtao_tx::PreflightCx<'_, V>,
    ) -> Result<Vec<String>, rtao_tx::IntentError> {
        Ok(HyperparamPlanner::new(cx.dispatch_origin)
            .plan(cx.view, self.netuid, &self.name, json_arg(&self.value)?)
            .await
            .map_err(planner_error)?
            .reasons)
    }

    fn validate(&self) -> Result<(), rtao_tx::IntentError> {
        canonical_value(self)?;
        Ok(())
    }

    fn semantic_intent(&self) -> &dyn rtao_tx::DynIntent {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtao_tx::{Intent, IntentInterface};
    use serde_json::json;

    #[test]
    fn tools_normalize_values_and_preserve_native_fixed_widths() -> anyhow::Result<()> {
        for (name, value, raw) in [
            ("min_burn", json!("0.7"), json!(700_000_000)),
            ("collateral_lock_share", json!("0.5"), json!(32768)),
            ("registration_allowed", json!(true), json!(1)),
            ("registration_allowed", json!("false"), json!(0)),
            (
                "burn_increase_mult",
                json!("1.5"),
                serde_json::from_str("27670116110564327424")?,
            ),
            ("epoch_consensus", json!("Null"), json!("Null")),
        ] {
            let parsed = crate::AnyIntent::from_tool(
                "set_hyperparameter",
                json!({"netuid":1,"name":name,"value":value}),
            )?;
            assert_eq!(parsed.to_json()["value"], raw, "{name}");
            assert_eq!(
                crate::AnyIntent::from_tool("set_hyperparameter", parsed.to_json())?,
                parsed
            );
            assert!(parsed.as_dyn().summary_dyn().contains(&format!("{name}=")));
        }
        for name in [
            "activity_cutoff",
            "recycle_or_burn",
            "sn_owner_hotkey",
            "unknown",
        ] {
            assert!(
                crate::AnyIntent::from_tool(
                    "set_hyperparameter",
                    json!({"netuid":1,"name":name,"value":1})
                )
                .is_err()
            );
        }
        for value in [json!(true), json!(1), json!("null"), json!({}), json!(null)] {
            assert!(
                crate::AnyIntent::from_tool(
                    "set_hyperparameter",
                    json!({"netuid":1,"name":"epoch_consensus","value":value})
                )
                .is_err()
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn real_builds_keep_alpha_partner_and_shape_boolean_enum_and_fixed_calls()
    -> anyhow::Result<()> {
        use rtao_client::{BlockRef, Client, ClientConfig, Network};
        use rtao_primitives::{AccountId, NetUid};
        use rtao_runtime::calls::admin_utils as calls;
        use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
        use rtao_tx::{BuildCx, OriginView, SystemClock};
        let mut state = FakeState::try_default()?;
        state.seed_python(
            "SubtensorModule",
            "AlphaValues",
            &[json!(1)],
            json!([12000, 60000]),
        )?;
        let fake = FakeChain::from_state(state);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(fake.clone()), config).await?;
        let snapshot = client.at(BlockRef::Best).await?;
        let netuid = NetUid(1);
        for (name, value, expected) in [
            (
                "alpha_low",
                json!(13000),
                calls::sudo_set_alpha_values(netuid, 13000, 60000),
            ),
            (
                "alpha_high",
                json!(61000),
                calls::sudo_set_alpha_values(netuid, 12000, 61000),
            ),
            (
                "registration_allowed",
                json!(true),
                calls::sudo_set_network_registration_allowed(netuid, true),
            ),
            (
                "epoch_consensus",
                json!("Null"),
                calls::sudo_set_epoch_consensus(
                    netuid,
                    rtao_core::DynValue::new(rtao_core::codec::Value::Str("Null".into())),
                ),
            ),
            (
                "min_burn",
                json!("0.7"),
                calls::sudo_set_min_burn(netuid, rtao_primitives::TaoBalance(700_000_000)),
            ),
        ] {
            let intent = SetHyperparameter {
                netuid,
                name: name.into(),
                value,
            };
            let built = intent
                .build(&BuildCx {
                    view: &snapshot,
                    origin: OriginView::Account(AccountId([7; 32])),
                    runtime: &snapshot.block().runtime,
                    clock: &SystemClock,
                    spec: SetHyperparameter::SPEC,
                })
                .await?;
            assert_eq!(
                built.call.as_bytes(),
                expected.compose(&snapshot.block().runtime)?.as_bytes(),
                "{name}"
            );
        }
        assert!(
            fake.state()
                .lock()
                .map_err(|e| anyhow::anyhow!(e.to_string()))?
                .submissions
                .is_empty()
        );
        client.close().await;
        Ok(())
    }

    #[test]
    fn namespace_delegates_and_preserves_native_widths() -> Result<(), HyperparamError> {
        assert_eq!(to_raw("min_burn", "0.7")?.as_u64(), Some(700_000_000));
        assert_eq!(
            to_raw("burn_increase_mult", "1.5")?.as_u128(),
            Some(3u128 << 63)
        );
        assert_eq!(
            to_raw("adjustment_alpha", u64::MAX)?.as_u64(),
            Some(u64::MAX)
        );
        for (name, text) in [
            ("kappa", "0.5"),
            ("registration_allowed", "true"),
            ("rho", "65536"),
            ("adjustment_alpha", "1.0"),
        ] {
            assert_eq!(
                to_raw(name, text),
                rtao_catalog::hyperparams::TABLE.to_raw(name, text)
            );
        }
        Ok(())
    }
}
