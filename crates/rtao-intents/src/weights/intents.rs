// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/weights.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Weight intent behavior, with state checks performed before signing.

use std::time::SystemTime;

use rtao_catalog::ErrorCode;
use rtao_client::{Snapshot, View, ViewExt};
use rtao_primitives::{AccountId, MechId, NetUid, NetUidStorageIndex, Ratio, Uid};
use rtao_runtime::{calls::subtensor_module as calls, storage::subtensor_module as st};
use rtao_tx::{BuildCx, Built, ChainError, Intent, IntentError, PreflightCx};
use serde_json::json;

use super::{ConformState, TimelockCommitRequest, WeightsError};
use crate::{CommitWeights, RevealWeights, SetWeights, WeightsArg};

struct CheckedState {
    conform: ConformState,
    commit_reveal: bool,
}

struct CommitRow {
    netuid: NetUid,
    mechid: MechId,
    row: (Vec<Uid>, Vec<u16>),
    version_key: u64,
    commit_reveal_version: u16,
}

fn semantic(message: String, code: ErrorCode) -> IntentError {
    IntentError::Chain(ChainError::with_code(message, None, code))
}

fn math_error(error: WeightsError) -> IntentError {
    match error.code() {
        Some(code) => semantic(error.to_string(), code),
        None => IntentError::Validation(error.to_string()),
    }
}

fn pairs(
    uids: Option<&[Uid]>,
    weights: Option<&WeightsArg>,
) -> Result<(Vec<Uid>, Vec<f64>), IntentError> {
    weights
        .ok_or_else(|| {
            IntentError::Validation(
        "Provide weights as a {uid: weight} mapping, or uids and weights as parallel lists.".into()
    )
        })?
        .as_pairs(uids)
}

fn count(uids: Option<&[Uid]>, weights: Option<&WeightsArg>) -> usize {
    uids.map_or_else(
        || match weights {
            Some(WeightsArg::Map(values)) => values.len(),
            _ => 0,
        },
        <[Uid]>::len,
    )
}

// py: weights.py::_preflight. Registration precedes the Null permit, then rate.
async fn checked<V: View + ?Sized>(
    view: &V,
    hotkey: AccountId,
    netuid: NetUid,
    mechid: MechId,
) -> Result<(Snapshot, CheckedState), IntentError> {
    let snapshot = View::pin(view).await?;
    let index = NetUidStorageIndex::new(netuid, mechid)?;
    let (uid, enabled, rate, last, min, max, mode) = snapshot
        .fetch((
            st::UIDS.at((netuid, hotkey)),
            st::COMMIT_REVEAL_WEIGHTS_ENABLED.at((netuid,)),
            st::WEIGHTS_SET_RATE_LIMIT.at((netuid,)),
            st::LAST_UPDATE.at((index,)),
            st::MIN_ALLOWED_WEIGHTS.at((netuid,)),
            st::MAX_WEIGHTS_LIMIT.at((netuid,)),
            st::SUBNET_EPOCH_CONSENSUS.at((netuid,)),
        ))
        .await?;
    let uid = uid.ok_or_else(|| {
        semantic(
            format!(
                "Hotkey {} is not registered on netuid {netuid}.",
                hotkey.to_ss58(42)
            ),
            ErrorCode::NotRegistered,
        )
    })?;
    let mode = mode.to_json();
    if null_mode(&mode, true) {
        let permits = snapshot.storage(&st::VALIDATOR_PERMIT, (netuid,)).await?;
        if !permits.get(usize::from(uid)).copied().unwrap_or(false) {
            return Err(semantic(
                format!(
                    "Only the sole Null validator permit holder may set weights on netuid {netuid}."
                ),
                ErrorCode::NotAuthorized,
            ));
        }
    }
    if rate != 0
        && let Some(&last) = last.get(usize::from(uid)).filter(|last| **last > 0)
    {
        let since = i128::from(snapshot.block().number) - i128::from(last);
        if since < i128::from(rate) {
            let remaining = i128::from(rate) - since;
            let seconds = (remaining as f64 * snapshot.block_time().await?.as_secs_f64()) as i128;
            return Err(semantic(
                format!(
                    "Weights on netuid {netuid} were last set {since} blocks ago; the rate limit is {rate} blocks. Retry in ~{remaining} blocks (~{seconds}s)."
                ),
                ErrorCode::RateLimited,
            ));
        }
    }
    Ok((
        snapshot,
        CheckedState {
            conform: ConformState {
                uid: Uid(uid),
                min_allowed_weights: min,
                max_weight_limit: max,
            },
            commit_reveal: enabled,
        },
    ))
}

fn null_mode(mode: &serde_json::Value, allow_numeric: bool) -> bool {
    mode == &json!("Null")
        || mode == &json!({"Null": null})
        || allow_numeric && mode.as_u64() == Some(1)
}

async fn ensure_raw_null(snapshot: &Snapshot, netuid: NetUid) -> Result<(), IntentError> {
    // Preserve Python's separate explicit-raw gate, including its mode forms.
    let mode = snapshot
        .storage(&st::SUBNET_EPOCH_CONSENSUS, (netuid,))
        .await?
        .to_json();
    if !null_mode(&mode, false) {
        return Err(IntentError::Validation(
            "raw_u16 requires a subnet running Null consensus.".into(),
        ));
    }
    Ok(())
}

async fn timelocked<V: View + ?Sized>(
    cx: &BuildCx<'_, V>,
    snapshot: &Snapshot,
    commit: CommitRow,
) -> Result<Built, IntentError> {
    let CommitRow {
        netuid,
        mechid,
        row,
        version_key,
        commit_reveal_version,
    } = commit;
    let (tempo, reveal, last_epoch, pending, epoch_index, since) = snapshot
        .fetch((
            st::TEMPO.at((netuid,)),
            st::REVEAL_PERIOD_EPOCHS.at((netuid,)),
            st::LAST_EPOCH_BLOCK.at((netuid,)),
            st::PENDING_EPOCH_AT.at((netuid,)),
            st::SUBNET_EPOCH_INDEX.at((netuid,)),
            st::BLOCKS_SINCE_LAST_STEP.at((netuid,)),
        ))
        .await?;
    let now = cx
        .clock
        .now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|error| IntentError::Validation(error.to_string()))?
        .as_secs_f64();
    let commit = super::commit::encrypt_at(TimelockCommitRequest {
        now,
        hotkey: AccountId(cx.hotkey_public_key()?),
        row,
        version_key,
        state: rtao_core::timelock::epoch_schedule::EpochScheduleState {
            current_block: snapshot.block().number,
            tempo,
            last_epoch_block: last_epoch,
            pending_epoch_at: pending,
            subnet_epoch_index: epoch_index,
            blocks_since_last_step: since,
        },
        reveal_period_epochs: reveal,
        block_time: snapshot.block_time().await?.as_secs_f64(),
    })
    .await
    .map_err(|error| IntentError::Validation(error.to_string()))?;
    let call = calls::commit_timelocked_mechanism_weights(
        netuid,
        mechid,
        commit.ciphertext,
        commit.reveal_round,
        commit_reveal_version,
    )
    .compose(cx.runtime)?;
    let mut built = Built::new(call);
    built
        .extras
        .insert("reveal_round".into(), json!(commit.reveal_round));
    Ok(built)
}

async fn clipping_warning<V: View + ?Sized>(
    cx: &PreflightCx<'_, V>,
    netuid: NetUid,
    weights: &[f64],
) -> Result<Vec<String>, IntentError> {
    let max = cx.view.storage(&st::MAX_WEIGHTS_LIMIT, (netuid,)).await?;
    let limit = f64::from(max) / f64::from(u16::MAX);
    let total = super::math::python_sum(weights.iter().copied());
    let largest = weights.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    Ok(
        if weights.len() > 1 && total > 0.0 && limit < 1.0 && largest / total > limit {
            vec![format!(
                "largest weight exceeds the subnet's max_weight_limit ({limit:.4} of total); weights will be clipped to conform"
            )]
        } else {
            vec![]
        },
    )
}

impl Intent for SetWeights {
    fn summary(&self) -> String {
        format!(
            "set weights on netuid {} for {} uids",
            self.netuid,
            count(self.uids.as_deref(), self.weights.as_ref())
        )
    }
    fn validate(&self) -> Result<(), IntentError> {
        pairs(self.uids.as_deref(), self.weights.as_ref()).map(|_| ())
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let (uids, weights) = pairs(self.uids.as_deref(), self.weights.as_ref())?;
        let (snapshot, state) =
            checked(cx.view, cx.hotkey_address(None)?, self.netuid, self.mechid).await?;
        if self.raw_u16 {
            ensure_raw_null(&snapshot, self.netuid).await?;
        }
        let row = super::conform(&uids, &weights, state.conform, self.netuid, self.raw_u16)
            .map_err(math_error)?;
        if state.commit_reveal {
            timelocked(
                cx,
                &snapshot,
                CommitRow {
                    netuid: self.netuid,
                    mechid: self.mechid,
                    row,
                    version_key: self.version_key,
                    commit_reveal_version:
                        crate::generated::defaults::commit_weights::commit_reveal_version(),
                },
            )
            .await
        } else {
            Ok(Built::new(
                calls::set_mechanism_weights(
                    self.netuid,
                    self.mechid,
                    row.0.into_iter().map(|uid| uid.0).collect(),
                    row.1,
                    self.version_key,
                )
                .compose(cx.runtime)?,
            ))
        }
    }
    async fn effects<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        let enabled = cx
            .view
            .storage(&st::COMMIT_REVEAL_WEIGHTS_ENABLED, (self.netuid,))
            .await?;
        let mode = if enabled {
            "timelocked commit (commit-reveal enabled; chain auto-reveals)"
        } else {
            "direct plaintext set"
        };
        Ok(vec![self.summary(), format!("submission path: {mode}")])
    }
    async fn warnings<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        if self.raw_u16 {
            return Ok(vec![]);
        }
        let (_, weights) = pairs(self.uids.as_deref(), self.weights.as_ref())?;
        clipping_warning(cx, self.netuid, &weights).await
    }
}

impl Intent for CommitWeights {
    fn summary(&self) -> String {
        format!(
            "commit weights on netuid {} for {} uids",
            self.netuid,
            count(self.uids.as_deref(), self.weights.as_ref())
        )
    }
    fn validate(&self) -> Result<(), IntentError> {
        pairs(self.uids.as_deref(), self.weights.as_ref()).map(|_| ())
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let (uids, weights) = pairs(self.uids.as_deref(), self.weights.as_ref())?;
        let (snapshot, state) =
            checked(cx.view, cx.hotkey_address(None)?, self.netuid, self.mechid).await?;
        if self.raw_u16 {
            ensure_raw_null(&snapshot, self.netuid).await?;
        }
        let row = super::conform(&uids, &weights, state.conform, self.netuid, self.raw_u16)
            .map_err(math_error)?;
        timelocked(
            cx,
            &snapshot,
            CommitRow {
                netuid: self.netuid,
                mechid: self.mechid,
                row,
                version_key: self.version_key,
                commit_reveal_version: self.commit_reveal_version,
            },
        )
        .await
    }
    async fn warnings<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        if self.raw_u16 {
            return Ok(vec![]);
        }
        let (_, weights) = pairs(self.uids.as_deref(), self.weights.as_ref())?;
        clipping_warning(cx, self.netuid, &weights).await
    }
}

impl Intent for RevealWeights {
    fn summary(&self) -> String {
        format!(
            "reveal weights on netuid {} for {} uids",
            self.netuid,
            self.uids.len()
        )
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let weights = self
            .weights
            .iter()
            .map(|ratio: &Ratio| ratio.get())
            .collect::<Vec<_>>();
        let row = super::normalize(&self.uids, &weights).map_err(math_error)?;
        if row.0.is_empty() {
            return Err(IntentError::Validation(
                "All weights are zero; nothing to reveal.".into(),
            ));
        }
        Ok(Built::new(
            calls::reveal_weights(
                self.netuid,
                row.0.into_iter().map(|uid| uid.0).collect(),
                row.1,
                self.salt.clone(),
                self.version_key,
            )
            .compose(cx.runtime)?,
        ))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use rtao_client::{BlockRef, Client, ClientConfig, Network, ReadKind};
    use rtao_core::codec::Value;
    use rtao_testkit::rpc::RpcClient;
    use rtao_testkit::{FakeChain, FakeState, RecordingView};
    use rtao_tx::{Clock, OriginView};
    use std::time::Duration;

    const NETUID: NetUid = NetUid(3);
    const MECHID: MechId = MechId(1);
    const HOTKEY: AccountId = AccountId([7; 32]);

    struct FixedClock;
    impl Clock for FixedClock {
        fn now(&self) -> SystemTime {
            SystemTime::UNIX_EPOCH + Duration::from_secs_f64(1_752_076_800.25)
        }
    }

    async fn fixture(null: bool, enabled: bool) -> (Snapshot, FakeChain) {
        let mut state = FakeState::default();
        state
            .seed_typed("SubtensorModule", "Uids", (NETUID, HOTKEY), 2u16)
            .unwrap();
        state
            .seed_typed(
                "SubtensorModule",
                "CommitRevealWeightsEnabled",
                (NETUID,),
                enabled,
            )
            .unwrap();
        state
            .seed_typed("SubtensorModule", "WeightsSetRateLimit", (NETUID,), 3u64)
            .unwrap();
        state
            .seed_typed(
                "SubtensorModule",
                "LastUpdate",
                (NetUidStorageIndex::new(NETUID, MECHID).unwrap(),),
                vec![0u64, 0, 97],
            )
            .unwrap();
        // Reading the bare netuid would incorrectly reject this mechanism.
        state
            .seed_typed(
                "SubtensorModule",
                "LastUpdate",
                (NetUidStorageIndex(NETUID.0),),
                vec![0u64, 0, 100],
            )
            .unwrap();
        state
            .seed_typed("SubtensorModule", "MinAllowedWeights", (NETUID,), 2u16)
            .unwrap();
        state
            .seed_typed("SubtensorModule", "MaxWeightsLimit", (NETUID,), u16::MAX)
            .unwrap();
        state
            .seed_python(
                "SubtensorModule",
                "SubnetEpochConsensus",
                &[json!(NETUID.0)],
                if null {
                    json!({"Null":null})
                } else {
                    json!({"Yuma":null})
                },
            )
            .unwrap();
        state
            .seed_typed(
                "SubtensorModule",
                "ValidatorPermit",
                (NETUID,),
                vec![false, false, true],
            )
            .unwrap();
        state
            .seed_typed(
                "SubtensorModule",
                "MechanismCountCurrent",
                (NETUID,),
                MechId(2),
            )
            .unwrap();
        state
            .seed_typed("SubtensorModule", "Tempo", (NETUID,), 20u16)
            .unwrap();
        state
            .seed_typed("SubtensorModule", "RevealPeriodEpochs", (NETUID,), 1u64)
            .unwrap();
        state
            .seed_typed("SubtensorModule", "LastEpochBlock", (NETUID,), 90u64)
            .unwrap();
        state
            .seed_typed("SubtensorModule", "PendingEpochAt", (NETUID,), 0u64)
            .unwrap();
        state
            .seed_typed("SubtensorModule", "SubnetEpochIndex", (NETUID,), 5u64)
            .unwrap();
        state
            .seed_typed("SubtensorModule", "BlocksSinceLastStep", (NETUID,), 15u64)
            .unwrap();
        let fake = FakeChain::from_state(state);
        let mut config =
            ClientConfig::new(Network::resolve_with("ws://127.0.0.1:9944", |_| None).unwrap())
                .unwrap();
        config.runtime_cache_dir = None;
        config.config_store = None;
        config.track_heads = false;
        let client = Client::from_rpc(RpcClient::new(fake.clone()), config)
            .await
            .unwrap();
        let snapshot = client.at(BlockRef::Best).await.unwrap();
        fake.mutate_state(|state| state.requests.clear());
        (snapshot, fake)
    }

    fn input(values: &[f64]) -> SetWeights {
        SetWeights {
            netuid: NETUID,
            mechid: MECHID,
            version_key: 123,
            raw_u16: false,
            uids: Some((0..values.len()).map(|index| Uid(index as u16)).collect()),
            weights: Some(WeightsArg::List(
                values
                    .iter()
                    .map(|value| Ratio::new(*value).unwrap())
                    .collect(),
            )),
        }
    }

    async fn build<I: Intent>(intent: &I, view: &Snapshot) -> Result<Built, IntentError> {
        let runtime = View::runtime(view).await?;
        intent
            .build(&BuildCx {
                view,
                origin: OriginView::Account(HOTKEY),
                runtime: &runtime,
                clock: &FixedClock,
                spec: I::SPEC,
            })
            .await
    }

    #[tokio::test]
    async fn plaintext_uses_mechanism_index_and_coalesced_pinned_preflight() {
        let (snapshot, fake) = fixture(false, false).await;
        let intent: SetWeights = input(&[1.0, 2.0, 3.0]);
        let built = build(&intent, &snapshot).await.unwrap();
        let expected = calls::set_mechanism_weights(
            NETUID,
            MECHID,
            vec![0, 1, 2],
            vec![21845, 43690, 65535],
            123,
        )
        .compose(&View::runtime(&snapshot).await.unwrap())
        .unwrap();
        assert_eq!(built.call.as_bytes(), expected.as_bytes());
        assert!(built.extras.is_empty());
        let requests = fake.requests();
        let queries = requests
            .iter()
            .filter(|request| request.method == "state_queryStorageAt")
            .collect::<Vec<_>>();
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].params[0].as_array().unwrap().len(), 7);
        assert_eq!(
            queries[0].params[1],
            json!(format!("0x{}", hex::encode(snapshot.block().hash.0)))
        );
        assert!(
            !requests
                .iter()
                .any(|request| request.method.starts_with("author_"))
        );
    }

    #[tokio::test]
    async fn null_permit_blocks_self_weights_before_conforming_or_crypto() {
        let (snapshot, fake) = fixture(true, false).await;
        fake.mutate_state(|state| {
            state
                .seed_typed(
                    "SubtensorModule",
                    "ValidatorPermit",
                    (NETUID,),
                    vec![false, false, false],
                )
                .unwrap()
        });
        let mut intent = input(&[1.0]);
        intent.uids = Some(vec![Uid(2)]);
        let error = build(&intent, &snapshot).await.unwrap_err();
        assert_eq!(error.code(), ErrorCode::NotAuthorized);
        assert_eq!(
            error.to_string(),
            "Only the sole Null validator permit holder may set weights on netuid 3."
        );
        assert!(
            !fake
                .requests()
                .iter()
                .any(|request| request.method.starts_with("author_"))
        );
    }

    #[tokio::test]
    async fn raw_null_ratios_are_retained_and_yuma_is_refused() {
        let (snapshot, _) = fixture(true, false).await;
        let mut intent = input(&[1.0, 2.0, 3.0]);
        intent.raw_u16 = true;
        let built = build(&intent, &snapshot).await.unwrap();
        let expected =
            calls::set_mechanism_weights(NETUID, MECHID, vec![0, 1, 2], vec![1, 2, 3], 123)
                .compose(&View::runtime(&snapshot).await.unwrap())
                .unwrap();
        assert_eq!(built.call.as_bytes(), expected.as_bytes());
        let (yuma, _) = fixture(false, false).await;
        assert_eq!(
            build(&intent, &yuma).await.unwrap_err().to_string(),
            "raw_u16 requires a subnet running Null consensus."
        );
    }

    #[tokio::test]
    async fn registration_and_rate_errors_keep_codes_order_and_wait_hint() {
        let (snapshot, fake) = fixture(true, false).await;
        fake.mutate_state(|state| {
            let info = state
                .runtime
                .storage_entry("SubtensorModule", "Uids")
                .unwrap();
            let key = state
                .runtime
                .storage_key(
                    info,
                    &[
                        Value::Int(i128::from(NETUID.0)),
                        Value::Bytes(HOTKEY.0.to_vec()),
                    ],
                )
                .unwrap();
            state.seed_encoded(key, None);
            state
                .seed_typed(
                    "SubtensorModule",
                    "ValidatorPermit",
                    (NETUID,),
                    vec![false, false, false],
                )
                .unwrap();
        });
        let intent = input(&[1.0, 2.0]);
        assert_eq!(
            build(&intent, &snapshot).await.unwrap_err().code(),
            ErrorCode::NotRegistered
        );
        let (snapshot, fake) = fixture(false, false).await;
        fake.mutate_state(|state| {
            state
                .seed_typed(
                    "SubtensorModule",
                    "LastUpdate",
                    (NetUidStorageIndex::new(NETUID, MECHID).unwrap(),),
                    vec![0u64, 0, 101],
                )
                .unwrap()
        });
        let error = build(&intent, &snapshot).await.unwrap_err();
        assert_eq!(error.code(), ErrorCode::RateLimited);
        assert_eq!(
            error.to_string(),
            "Weights on netuid 3 were last set -1 blocks ago; the rate limit is 3 blocks. Retry in ~4 blocks (~48s)."
        );
    }

    #[tokio::test]
    async fn adaptive_and_forced_commits_share_corrected_round_and_extras() {
        let (snapshot, _) = fixture(false, true).await;
        let set = input(&[1.0, 2.0, 3.0]);
        let adaptive = build(&set, &snapshot).await.unwrap();
        let expected_round = rtao_core::timelock::timelock_commit::reveal_round_at(
            1_752_076_800.25,
            &rtao_core::timelock::epoch_schedule::EpochScheduleState {
                current_block: snapshot.block().number,
                tempo: 20,
                last_epoch_block: 90,
                pending_epoch_at: 0,
                subnet_epoch_index: 5,
                blocks_since_last_step: 15,
            },
            1,
            snapshot.block_time().await.unwrap().as_secs_f64(),
        )
        .unwrap();
        assert_eq!(
            adaptive.extras.get("reveal_round"),
            Some(&json!(expected_round))
        );
        assert_eq!(&adaptive.call.as_bytes()[..2], &[7, 118]);
        let (disabled, _) = fixture(false, false).await;
        let force = CommitWeights {
            netuid: set.netuid,
            uids: set.uids,
            weights: set.weights,
            mechid: set.mechid,
            version_key: set.version_key,
            raw_u16: false,
            commit_reveal_version: 4,
        };
        let forced = build(&force, &disabled).await.unwrap();
        assert_eq!(
            forced.extras.get("reveal_round"),
            Some(&json!(expected_round))
        );
        assert_eq!(&forced.call.as_bytes()[..2], &[7, 118]);
        assert_ne!(
            adaptive.call.as_bytes(),
            forced.call.as_bytes(),
            "encryption is randomized"
        );
    }

    #[tokio::test]
    async fn null_commit_preserves_python_preflight_and_schedule_reads() {
        let (snapshot, fake) = fixture(true, true).await;
        let view = RecordingView::new(snapshot);
        let snapshot = View::pin(&view).await.unwrap();
        let mut intent = input(&[1.0, 2.0, 3.0]);
        intent.raw_u16 = true;
        let built = build(&intent, &snapshot).await.unwrap();
        assert_eq!(&built.call.as_bytes()[..2], &[7, 118]);
        let actual = view
            .touches()
            .into_iter()
            .filter(|touch| touch.kind == ReadKind::Storage)
            .map(|touch| {
                assert_eq!(touch.at, Some(snapshot.block().hash));
                assert_eq!(touch.pallet_or_api, "SubtensorModule");
                touch.item
            })
            .collect::<std::collections::BTreeSet<_>>();
        let expected = [
            "BlocksSinceLastStep",
            "CommitRevealWeightsEnabled",
            "LastEpochBlock",
            "LastUpdate",
            "MaxWeightsLimit",
            "MinAllowedWeights",
            "PendingEpochAt",
            "RevealPeriodEpochs",
            "SubnetEpochConsensus",
            "SubnetEpochIndex",
            "Tempo",
            "Uids",
            "ValidatorPermit",
            "WeightsSetRateLimit",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(actual, expected);
        assert!(
            fake.requests()
                .iter()
                .all(|request| !request.method.starts_with("author_"))
        );
    }

    #[tokio::test]
    async fn legacy_reveal_quantizes_without_reading_registration_or_limits() {
        let (snapshot, fake) = fixture(false, false).await;
        let reveal = RevealWeights {
            netuid: NETUID,
            uids: vec![Uid(0), Uid(1), Uid(2)],
            weights: vec![
                Ratio::new(1.0).unwrap(),
                Ratio::new(2.0).unwrap(),
                Ratio::new(3.0).unwrap(),
            ],
            salt: vec![0, 123, u16::MAX],
            version_key: 123,
        };
        let built = build(&reveal, &snapshot).await.unwrap();
        let expected = calls::reveal_weights(
            NETUID,
            vec![0, 1, 2],
            vec![21845, 43690, 65535],
            reveal.salt.clone(),
            123,
        )
        .compose(&View::runtime(&snapshot).await.unwrap())
        .unwrap();
        assert_eq!(built.call.as_bytes(), expected.as_bytes());
        assert!(fake.requests().is_empty());
        let mut zero = reveal;
        zero.weights = vec![Ratio::new(0.0).unwrap(); 3];
        assert_eq!(
            build(&zero, &snapshot).await.unwrap_err().to_string(),
            "All weights are zero; nothing to reveal."
        );
    }

    include!("null_weights_tests.rs");
}
