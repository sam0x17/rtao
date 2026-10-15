// Derived from RaoFoundation/subtensor pallets/subtensor/src/utils/{misc,rate_limiting}.rs and pallets/admin-utils/src/lib.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Owner-only hyperparameter planning at one pinned chain state.

use super::{owner_call, owner_parameter, owner_value};
use futures::StreamExt;
use rtao_catalog::owner_map::{OwnerCooldown, OwnerRateKind};
use rtao_client::{BlockRef, Client, ClientError, Finality, View, ViewExt};
use rtao_core::{DynValue, codec::Value};
use rtao_primitives::{AccountId, NetUid};
use rtao_runtime::{CallBytes, storage::subtensor_module as st};
use rtao_tx::{ChainError, IntentError, TxError};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::HparamArg;

/// A composed setting and the earliest block suggested by its pinned state.
#[derive(Clone, Debug)]
pub struct HparamPlan {
    /// Current value of the named setting.
    pub current: DynValue,
    /// Converted SCALE value for the requested setting.
    pub raw: DynValue,
    /// Verified owner setter bytes.
    pub call: CallBytes,
    /// Inclusive next candidate height; waiting always replans at a fresh head.
    pub earliest_block: u64,
    /// Reasons the pinned state currently prohibits execution.
    pub reasons: Vec<String>,
}

/// Whether to return a closed-window plan or wait for a fresh open plan.
#[derive(Clone, Copy, Debug)]
pub enum WindowWait {
    /// Return the plan immediately, including its postponement reasons.
    NoWait,
    /// Replan on advancing best heads until open or the deadline expires.
    UntilOpen {
        /// Absolute monotonic deadline, covering RPC reads and subscriptions.
        deadline: Instant,
    },
}

/// Owner setter planner; no signing or submission happens here.
#[derive(Clone, Debug)]
pub struct HyperparamPlanner {
    owner: AccountId,
}

impl HyperparamPlanner {
    /// Bind transaction-key cooldown reads to the signing owner coldkey.
    pub fn new(owner: AccountId) -> Self {
        Self { owner }
    }

    /// Plan an allowlisted setting against one coherent pinned view.
    pub async fn plan<V: View + ?Sized>(
        &self,
        at: &V,
        netuid: NetUid,
        name: &str,
        value: HparamArg,
    ) -> Result<HparamPlan, TxError> {
        let row = owner_parameter(name)?;
        let raw = owner_value(row, value)?;
        let snapshot = at.pin().await?;
        self.ensure_owner(&snapshot, netuid).await?;
        let pair = snapshot
            .storage_dyn(
                row.current.pallet,
                row.current.name,
                &[Value::Uint(u128::from(netuid.0))],
            )
            .await?;
        let call = owner_call(row, netuid, &raw, &pair)?
            .compose(&snapshot.block().runtime)
            .map_err(ClientError::from)?;
        let current = match row.shape {
            rtao_catalog::owner_map::OwnerValueShape::AlphaPair { index } => {
                let Value::Tuple(values) = pair.as_core() else {
                    return Err(invalid("AlphaValues is not a pair"));
                };
                DynValue::new(
                    values
                        .get(usize::from(index))
                        .cloned()
                        .ok_or_else(|| invalid("missing alpha side"))?,
                )
            }
            _ => pair,
        };
        let (earliest_block, reasons) = self.window(&snapshot, netuid, row.cooldown).await?;
        Ok(HparamPlan {
            current,
            raw,
            call,
            earliest_block,
            reasons,
        })
    }

    /// Return or wait for a plan, refreshing both rules and alpha values at each head.
    ///
    /// Predictions cannot account for manual, capped or safety-net epoch triggers.
    /// A wait treats `earliest_block` as a candidate and checks fresh state before
    /// returning. Cancellation and the deadline also bound in-flight reads.
    pub async fn plan_with_wait(
        &self,
        client: &Client,
        netuid: NetUid,
        name: &str,
        value: HparamArg,
        wait: WindowWait,
        cancel: &CancellationToken,
    ) -> Result<HparamPlan, TxError> {
        let future = async {
            let mut heads = match wait {
                WindowWait::NoWait => None,
                WindowWait::UntilOpen { .. } => Some(client.blocks_strict(Finality::Best).await?),
            };
            loop {
                let snapshot = client.at(BlockRef::Best).await?;
                let plan = self.plan(&snapshot, netuid, name, value.clone()).await?;
                if plan.reasons.is_empty() || matches!(wait, WindowWait::NoWait) {
                    return Ok(plan);
                }
                let stream = heads
                    .as_mut()
                    .ok_or_else(|| invalid("missing owner wait stream"))?;
                match stream.next().await {
                    Some(Ok(_)) => {}
                    Some(Err(rtao_client::HeadError::Reorg(_))) => {}
                    Some(Err(error)) => return Err(ClientError::Protocol(error.to_string()).into()),
                    None => {
                        return Err(
                            ClientError::Protocol("owner wait head stream ended".into()).into()
                        );
                    }
                }
            }
        };
        let deadline = async {
            match wait {
                WindowWait::UntilOpen { deadline } => tokio::time::sleep_until(deadline).await,
                WindowWait::NoWait => futures::future::pending::<()>().await,
            }
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(invalid("owner window wait cancelled")),
            _ = deadline => Err(invalid("owner window wait deadline expired")),
            result = future => result,
        }
    }

    /// Evaluate the freeze and cooldown for an extracted owner operation rule.
    ///
    /// Matching Null batches bypass their cooldown while retaining the freeze.
    /// Call composition and execution belong to the later owner intents.
    pub async fn operation_window<V: View + ?Sized>(
        &self,
        at: &V,
        netuid: NetUid,
        setter: &str,
        null_target: Option<u16>,
    ) -> Result<(u64, Vec<String>), TxError> {
        let rule = rtao_catalog::owner_map::OWNER_OPERATION_RULES
            .iter()
            .find(|row| row.setter == setter)
            .ok_or_else(|| invalid("unknown owner operation"))?;
        let snapshot = at.pin().await?;
        self.ensure_owner(&snapshot, netuid).await?;
        let cooldown = if rule.continuation_storage.is_some() {
            let requested = null_target.ok_or_else(|| invalid("Null pruning target required"))?;
            if snapshot
                .storage(&st::NULL_PRUNING_TARGET, (netuid,))
                .await?
                == Some(requested)
            {
                OwnerCooldown::None
            } else {
                rule.cooldown
            }
        } else {
            rule.cooldown
        };
        self.window(&snapshot, netuid, cooldown).await
    }

    async fn ensure_owner<V: View + ?Sized>(&self, at: &V, netuid: NetUid) -> Result<(), TxError> {
        let stored = at.storage(&st::SUBNET_OWNER, (netuid,)).await?;
        if stored.get() != Some(&self.owner) {
            return Err(TxError::Chain(ChainError::new(
                "signer is not the subnet owner",
                Some("BadOrigin".into()),
            )));
        }
        Ok(())
    }

    async fn window<V: View + ?Sized>(
        &self,
        at: &V,
        netuid: NetUid,
        cooldown: OwnerCooldown,
    ) -> Result<(u64, Vec<String>), TxError> {
        let snapshot = at.pin().await?;
        let now = snapshot.block().number;
        let (tempo, epoch, pending, freeze) = snapshot
            .fetch((
                st::TEMPO.at((netuid,)),
                st::LAST_EPOCH_BLOCK.at((netuid,)),
                st::PENDING_EPOCH_AT.at((netuid,)),
                st::ADMIN_FREEZE_WINDOW.at(()),
            ))
            .await?;
        let mut earliest = now;
        let mut reasons = vec![];
        if tempo != 0 {
            if pending > now {
                earliest = pending;
                reasons.push(format!(
                    "admin freeze until pending epoch at block {pending}; recheck epoch state"
                ));
            } else if epoch.saturating_add(u64::from(tempo)).saturating_sub(now) < u64::from(freeze)
            {
                earliest = epoch
                    .saturating_add(u64::from(tempo))
                    .max(now.saturating_add(1));
                reasons
                    .push("admin freeze until the next actual epoch; recheck epoch state".into());
            }
        }
        let (last, limit) = match cooldown {
            OwnerCooldown::None => (0, 0),
            OwnerCooldown::OwnerHyperparameter(h) => {
                let key = owner_rate_key(netuid, h);
                let (last, periods) = snapshot
                    .fetch((
                        st::LAST_RATE_LIMITED_BLOCK.at((key,)),
                        st::OWNER_HYPERPARAM_RATE_LIMIT.at(()),
                    ))
                    .await?;
                (last, u64::from(tempo).saturating_mul(u64::from(periods)))
            }
            OwnerCooldown::Transaction(code, rule) => {
                let last = snapshot
                    .storage(&st::TRANSACTION_KEY_LAST_BLOCK, (self.owner, netuid, code))
                    .await?;
                let limit = match rule {
                    OwnerRateKind::TempoMultiplier(storage) => {
                        let periods = snapshot
                            .storage_dyn("SubtensorModule", storage, &[])
                            .await?;
                        u64::from(tempo).saturating_mul(
                            periods
                                .as_u128()
                                .and_then(|n| u64::try_from(n).ok())
                                .ok_or_else(|| invalid("owner rate multiplier is not u64"))?,
                        )
                    }
                    OwnerRateKind::FixedConstant(name) => {
                        if name != "MinTempo" {
                            return Err(invalid("unknown fixed owner cooldown"));
                        }
                        u64::from(
                            snapshot
                                .constant(&rtao_runtime::constants::subtensor_module::MIN_TEMPO)
                                .await?,
                        )
                    }
                    OwnerRateKind::RuntimeDefault(limit) => {
                        if snapshot
                            .constant(&rtao_runtime::constants::aura::SLOT_DURATION)
                            .await?
                            < rtao_catalog::owner_map::PRODUCTION_SLOT_DURATION_MS
                        {
                            limit.fast
                        } else {
                            limit.production
                        }
                    }
                };
                (last, limit)
            }
        };
        if last != 0 && now.saturating_sub(last) < limit {
            let opens = last.saturating_add(limit);
            earliest = earliest.max(opens);
            reasons.push(format!("owner setter cooldown until block {opens}"));
        }
        Ok((earliest, reasons))
    }
}

fn invalid(message: &str) -> TxError {
    IntentError::InvalidArgument(message.into()).into()
}

/// Encode the runtime's authenticated owner-hyperparameter cooldown key.
#[doc(hidden)]
pub fn owner_rate_key(netuid: NetUid, h: &str) -> DynValue {
    DynValue::new(Value::record(vec![(
        "OwnerHyperparamUpdate".into(),
        Value::Tuple(vec![
            Value::Uint(u128::from(netuid.0)),
            Value::Str(h.into()),
        ]),
    )]))
}
