// Derived from RaoFoundation/subtensor pallets/subtensor/src/subnets/weights.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Follow the included commit's actual epoch and retain reveal diagnostics.

use super::WeightPreflight;
use crate::Metrics;
use rtao_client::{BlockHeader, BlockRef, Client, Snapshot, ViewExt};
use rtao_core::value_ext::ValueExt;
use rtao_core::{
    codec::Value,
    timelock::epoch_schedule::{
        current_epoch_pre_run_coinbase, predict_first_reveal_block, simulate_run_coinbase,
    },
};
use rtao_intents::weights::PreparedWeightCommit;
use rtao_metagraph::{MetagraphOptions, SubnetsMetagraphExt};
use rtao_primitives::{AccountId, MechId, NetUid, NetUidStorageIndex, Uid};
use rtao_reads::Namespaces;
use rtao_runtime::storage::subtensor_module as st;
use std::collections::{BTreeMap, VecDeque};

/// Chain epoch changes retained beside reveal outcomes for attribution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EpochNotice {
    /// Global epoch capacity delayed this subnet's processing.
    Deferred(u64),
    /// Inconsistent input skipped an epoch boundary.
    Skipped(u64),
    /// An owner requested an early epoch after commitment.
    Triggered(u64),
}

/// First observed reason a committed row could not reveal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevealMissReason {
    /// The committing hotkey was deregistered.
    NotRegistered,
    /// The first reveal snapshot was below the minimum stake threshold.
    Stake,
    /// Owner forced a newer weight implementation before the reveal.
    VersionKey,
    /// The validator permit was absent when a nonself row revealed.
    Permit,
    /// A destination UID no longer existed.
    Uid,
    /// The row no longer met population or minimum-count checks.
    Length,
    /// Chain drand ingestion had not reached the encrypted round.
    DrandRound,
    /// An owner-triggered early epoch shortened the cryptographic window.
    EpochTriggered,
    /// The runtime emitted no success event despite all observed checks passing.
    Unknown,
}
impl RevealMissReason {
    /// Stable Prometheus label for the miss reason.
    pub const fn label(self) -> &'static str {
        match self {
            Self::NotRegistered => "not_registered",
            Self::Stake => "stake",
            Self::VersionKey => "version_key",
            Self::Permit => "permit",
            Self::Uid => "uid",
            Self::Length => "length",
            Self::DrandRound => "drand_round",
            Self::EpochTriggered => "epoch_triggered",
            Self::Unknown => "unknown",
        }
    }
}

/// A bounded, completed reveal outcome for operator inspection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevealOutcome {
    /// Mechanism whose row was committed.
    pub mechanism: MechId,
    /// Epoch that actually received the commit, rather than the prediction.
    pub commit_epoch: u64,
    /// Actual reveal epoch followed by the watcher.
    pub reveal_epoch: u64,
    /// Predicted first reveal block at preparation time.
    pub predicted_first_block: u64,
    /// Drand round embedded in the commitment.
    pub round: u64,
    /// Finalized block where success or epoch expiration was observed.
    pub observed_block: u64,
    /// No miss reason when a matching success event was observed.
    pub missed: Option<RevealMissReason>,
    /// Relevant epoch events observed between inclusion and completion.
    pub epoch_notices: Vec<EpochNotice>,
}

#[derive(Clone, Debug)]
struct Pending {
    netuid: NetUid,
    mechanism: MechId,
    hotkey: AccountId,
    uid: Uid,
    uids: Vec<Uid>,
    version_key: u64,
    commit_epoch: u64,
    reveal_epoch: u64,
    predicted_first_block: u64,
    round: u64,
    inclusion_block: u64,
    first_check: Option<RevealMissReason>,
    first_checked: bool,
    first_reveal_hash: Option<rtao_primitives::H256>,
    last_round: u64,
    notices: Vec<EpochNotice>,
    warned_permit: bool,
}

#[derive(Clone)]
struct Unlocated {
    netuid: NetUid,
    mechanism: MechId,
    hotkey: AccountId,
    row: (Vec<Uid>, Vec<u16>),
    version_key: u64,
    state: WeightPreflight,
    commit: PreparedWeightCommit,
    report: super::submit::WeightSubmission,
}

/// Retains outstanding reveals and the last 256 completed outcomes.
#[derive(Default)]
pub struct RevealWatcher {
    pending: Vec<Pending>,
    unlocated: Vec<Unlocated>,
    outcomes: VecDeque<RevealOutcome>,
    last_observed_block: Option<u64>,
}
impl RevealWatcher {
    /// Number of included commitments still awaiting an outcome.
    pub fn pending(&self) -> usize {
        self.pending.len() + self.unlocated.len()
    }
    /// Included or temporarily unlocated commitments for one mechanism.
    pub fn pending_for(&self, mechanism: MechId) -> usize {
        self.pending
            .iter()
            .filter(|pending| pending.mechanism == mechanism)
            .count()
            + self
                .unlocated
                .iter()
                .filter(|pending| pending.mechanism == mechanism)
                .count()
    }
    /// Completed outcomes, oldest retained first.
    pub fn outcomes(&self) -> &VecDeque<RevealOutcome> {
        &self.outcomes
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn record(
        &mut self,
        client: &Client,
        netuid: NetUid,
        mechanism: MechId,
        hotkey: AccountId,
        row: (Vec<Uid>, Vec<u16>),
        version_key: u64,
        state: WeightPreflight,
        commit: PreparedWeightCommit,
        report: &super::submit::WeightSubmission,
        metrics: &Metrics,
    ) -> anyhow::Result<()> {
        self.unlocated.push(Unlocated {
            netuid,
            mechanism,
            hotkey,
            row: row.clone(),
            version_key,
            state: state.clone(),
            commit: commit.clone(),
            report: report.clone(),
        });
        let result = self
            .try_record(
                client,
                netuid,
                mechanism,
                hotkey,
                row,
                version_key,
                state,
                commit,
                report,
                metrics,
            )
            .await;
        if result.is_ok() {
            self.unlocated.pop();
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    async fn try_record(
        &mut self,
        client: &Client,
        netuid: NetUid,
        mechanism: MechId,
        hotkey: AccountId,
        row: (Vec<Uid>, Vec<u16>),
        version_key: u64,
        state: WeightPreflight,
        commit: PreparedWeightCommit,
        report: &super::submit::WeightSubmission,
        metrics: &Metrics,
    ) -> anyhow::Result<()> {
        let hash = report.block_hash;
        let inclusion = report.block_number;
        let index = NetUidStorageIndex::new(netuid, mechanism)?;
        let assumed_inclusion = state
            .at
            .number
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("commit prediction height overflow"))?;
        let after = simulate_run_coinbase(&state.schedule, assumed_inclusion);
        let predicted_epoch = current_epoch_pre_run_coinbase(&after, assumed_inclusion);
        let next_epoch = predicted_epoch
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("commit epoch overflow"))?;
        // The commit's inclusion block may so far exist only on a separate
        // submission endpoint; read it where it exists.
        let snapshot = client.at_receipt(hash).await?;
        let (first, second) = snapshot
            .fetch((
                st::TIMELOCKED_WEIGHT_COMMITS.at((index, predicted_epoch)),
                st::TIMELOCKED_WEIGHT_COMMITS.at((index, next_epoch)),
            ))
            .await?;
        let matches = |entry: &(AccountId, u64, Vec<u8>, u64)| {
            entry.0 == hotkey
                && entry.1 == inclusion
                && entry.2 == commit.ciphertext
                && entry.3 == commit.reveal_round
        };
        let actual = if first.iter().any(matches) {
            predicted_epoch
        } else if second.iter().any(matches) {
            next_epoch
        } else {
            // Delayed inclusion can span more than one epoch. Read the actual
            // inclusion index and probe it rather than guessing or abandoning
            // an already accepted commitment.
            let current = snapshot.storage(&st::SUBNET_EPOCH_INDEX, (netuid,)).await?;
            let queue = snapshot
                .storage(&st::TIMELOCKED_WEIGHT_COMMITS, (index, current))
                .await?;
            anyhow::ensure!(
                queue.iter().any(matches),
                "included commit is absent from predicted and actual inclusion epoch queues"
            );
            current
        };
        if actual != predicted_epoch {
            metrics.increment("rtao_weights_commit_epoch_slip_total");
        }
        let reveal_epoch = actual
            .checked_add(state.reveal_period)
            .ok_or_else(|| anyhow::anyhow!("reveal epoch overflow"))?;
        self.pending.push(Pending {
            netuid,
            mechanism,
            hotkey,
            uid: state
                .uid
                .ok_or_else(|| anyhow::anyhow!("included commit lost its preflight UID"))?,
            uids: row.0,
            version_key,
            commit_epoch: actual,
            reveal_epoch,
            predicted_first_block: predict_first_reveal_block(
                &state.schedule,
                state.reveal_period,
            )?,
            round: commit.reveal_round,
            inclusion_block: inclusion,
            first_check: None,
            first_checked: false,
            first_reveal_hash: None,
            last_round: 0,
            notices: Vec::new(),
            warned_permit: false,
        });
        // Reconciliation may discover an inclusion older than the head that
        // triggered submission. Replay finalized history from that inclusion
        // so a reveal already emitted during reconciliation is still observed.
        let resume = inclusion.saturating_sub(1);
        self.last_observed_block = Some(
            self.last_observed_block
                .map_or(resume, |last| last.min(resume)),
        );
        Ok(())
    }

    /// Process gap-free finalized heads and attribute outcomes at actual epochs.
    pub async fn observe(
        &mut self,
        client: &Client,
        head: &BlockHeader,
        metrics: &Metrics,
    ) -> anyhow::Result<()> {
        let mut index = 0;
        while index < self.unlocated.len() {
            let unlocated = self.unlocated[index].clone();
            if let Err(error) = self
                .try_record(
                    client,
                    unlocated.netuid,
                    unlocated.mechanism,
                    unlocated.hotkey,
                    unlocated.row,
                    unlocated.version_key,
                    unlocated.state,
                    unlocated.commit,
                    &unlocated.report,
                    metrics,
                )
                .await
            {
                tracing::warn!(mechanism = unlocated.mechanism.0, %error, "retaining accepted commitment until its inclusion queue can be read");
                index += 1;
            } else {
                self.unlocated.remove(index);
            }
        }
        if let Some(previous) = self.last_observed_block
            && !self.pending.is_empty()
        {
            for number in previous.saturating_add(1)..head.number {
                let snapshot = client.at(BlockRef::Number(number)).await?;
                let hash = snapshot.block().hash;
                let raw = client
                    .request(
                        "chain_getHeader",
                        serde_json::json!([format!("0x{}", hex::encode(hash.0))]),
                    )
                    .await?;
                let recovered = BlockHeader::from_json_with_hash(raw, hash)?;
                self.process_head(client, &recovered, metrics).await?;
                metrics.increment("rtao_weights_reveal_history_replayed_total");
                if self.pending.is_empty() {
                    break;
                }
            }
        }
        self.process_head(client, head, metrics).await
    }

    async fn process_head(
        &mut self,
        client: &Client,
        head: &BlockHeader,
        metrics: &Metrics,
    ) -> anyhow::Result<()> {
        if self
            .last_observed_block
            .is_some_and(|last| head.number <= last)
        {
            return Ok(());
        }
        if self.pending.is_empty() {
            self.last_observed_block = Some(head.number);
            return Ok(());
        }
        let snapshot = client.at(BlockRef::Hash(head.hash)).await?;
        let events = snapshot.events().await?;
        // Reveal processing runs before coinbase. Using the parent schedule
        // captures its lookahead even when coinbase is deferred or skipped.
        let parent = match head.parent_hash {
            Some(hash) => client.at(BlockRef::Hash(hash)).await?,
            None => {
                client
                    .at(BlockRef::Number(head.number.saturating_sub(1)))
                    .await?
            }
        };
        let mut completed = Vec::new();
        let mut observed_epochs = BTreeMap::new();
        let mut stored_round = None;
        for pending in &mut self.pending {
            if head.number < pending.inclusion_block {
                continue;
            }
            for event in events.records() {
                if !event.matches("SubtensorModule", "EpochDeferred")
                    && !event.matches("SubtensorModule", "EpochSkipped")
                    && !event.matches("SubtensorModule", "EpochTriggered")
                {
                    continue;
                }
                if event_netuid(&event.attributes) != Some(pending.netuid.0) {
                    continue;
                }
                let notice = match event.event_id.as_str() {
                    "EpochDeferred" => EpochNotice::Deferred(head.number),
                    "EpochSkipped" => EpochNotice::Skipped(head.number),
                    _ => EpochNotice::Triggered(head.number),
                };
                if pending.notices.len() < 64 {
                    pending.notices.push(notice);
                }
            }
            let index = NetUidStorageIndex::new(pending.netuid, pending.mechanism)?;
            let epoch = if let Some(epoch) = observed_epochs.get(&pending.netuid) {
                *epoch
            } else {
                let epoch = reveal_processing_epoch(&parent, pending.netuid, head.number).await?;
                observed_epochs.insert(pending.netuid, epoch);
                epoch
            };
            if epoch == pending.reveal_epoch
                && events.records().iter().any(|event| {
                    event.matches("SubtensorModule", "TimelockedWeightsRevealed")
                        && reveal_matches(&event.attributes, index, pending.hotkey)
                })
            {
                completed.push((pending.mechanism, pending.inclusion_block, None));
                continue;
            }
            if epoch < pending.reveal_epoch {
                if !pending.warned_permit && epoch > pending.commit_epoch {
                    let permits = snapshot
                        .storage(&st::VALIDATOR_PERMIT, (pending.netuid,))
                        .await?;
                    if permits.get(usize::from(pending.uid.0)) != Some(&true) {
                        tracing::warn!(
                            mechanism = pending.mechanism.0,
                            reveal_epoch = pending.reveal_epoch,
                            "validator permit lost before reveal epoch"
                        );
                        metrics.increment("rtao_weights_permit_lost_before_reveal_total");
                        pending.warned_permit = true;
                    }
                }
                continue;
            }
            if !pending.first_checked {
                let hash = *pending.first_reveal_hash.get_or_insert(head.hash);
                let first_snapshot = client.at(BlockRef::Hash(hash)).await?;
                pending.first_check = first_check(&first_snapshot, pending).await?;
                pending.first_checked = true;
            }
            if epoch == pending.reveal_epoch {
                pending.last_round = if let Some(round) = stored_round {
                    round
                } else {
                    let round = snapshot
                        .storage(&rtao_runtime::storage::drand::LAST_STORED_ROUND, ())
                        .await?;
                    stored_round = Some(round);
                    round
                };
            }
            if epoch > pending.reveal_epoch {
                // The parent is the last block inside the reveal window. Its
                // drand state stays valid even if this block ingested the round
                // too late, or an earlier watcher RPC temporarily failed.
                pending.last_round = parent
                    .storage(&rtao_runtime::storage::drand::LAST_STORED_ROUND, ())
                    .await?;
                let reason = pending.first_check.unwrap_or_else(|| {
                    if pending.last_round < pending.round {
                        RevealMissReason::DrandRound
                    } else if pending
                        .notices
                        .iter()
                        .any(|event| matches!(event, EpochNotice::Triggered(_)))
                    {
                        RevealMissReason::EpochTriggered
                    } else {
                        RevealMissReason::Unknown
                    }
                });
                completed.push((pending.mechanism, pending.inclusion_block, Some(reason)));
            }
        }
        for (mechanism, inclusion, missed) in completed {
            if let Some(position) = self.pending.iter().position(|pending| {
                pending.mechanism == mechanism && pending.inclusion_block == inclusion
            }) {
                let pending = self.pending.remove(position);
                if let Some(reason) = missed {
                    metrics.increment(&format!(
                        "rtao_weights_reveal_missed_total{{mech=\"{}\",reason=\"{}\"}}",
                        mechanism.0,
                        reason.label()
                    ));
                    metrics.update_readiness(|ready| ready.last_weight_succeeded = false);
                    tracing::warn!(mechanism = mechanism.0, reason = reason.label(), round = pending.round, predicted_first_block = pending.predicted_first_block, last_stored_round = pending.last_round, epoch_notices = ?pending.notices, "weight reveal epoch ended without a success event");
                } else {
                    metrics.increment("rtao_weights_revealed_total");
                }
                if self.outcomes.len() == 256 {
                    self.outcomes.pop_front();
                }
                self.outcomes.push_back(RevealOutcome {
                    mechanism,
                    commit_epoch: pending.commit_epoch,
                    reveal_epoch: pending.reveal_epoch,
                    predicted_first_block: pending.predicted_first_block,
                    round: pending.round,
                    observed_block: head.number,
                    missed,
                    epoch_notices: pending.notices,
                });
            }
        }
        self.last_observed_block = Some(head.number);
        Ok(())
    }
}

async fn reveal_processing_epoch(
    parent: &Snapshot,
    netuid: NetUid,
    block: u64,
) -> anyhow::Result<u64> {
    let (tempo, last, pending, index, counter) = parent
        .fetch((
            st::TEMPO.at((netuid,)),
            st::LAST_EPOCH_BLOCK.at((netuid,)),
            st::PENDING_EPOCH_AT.at((netuid,)),
            st::SUBNET_EPOCH_INDEX.at((netuid,)),
            st::BLOCKS_SINCE_LAST_STEP.at((netuid,)),
        ))
        .await?;
    let state = rtao_core::timelock::epoch_schedule::EpochScheduleState {
        tempo,
        last_epoch_block: last,
        pending_epoch_at: pending,
        subnet_epoch_index: index,
        blocks_since_last_step: counter,
        current_block: parent.block().number,
    };
    Ok(current_epoch_pre_run_coinbase(&state, block))
}
async fn first_check(
    snapshot: &Snapshot,
    pending: &Pending,
) -> anyhow::Result<Option<RevealMissReason>> {
    let (uid, permits, threshold, version, population, min, owner, consensus) = snapshot
        .fetch((
            st::UIDS.at((pending.netuid, pending.hotkey)),
            st::VALIDATOR_PERMIT.at((pending.netuid,)),
            st::STAKE_THRESHOLD.at(()),
            st::WEIGHTS_VERSION_KEY.at((pending.netuid,)),
            st::SUBNETWORK_N.at((pending.netuid,)),
            st::MIN_ALLOWED_WEIGHTS.at((pending.netuid,)),
            st::SUBNET_OWNER_HOTKEY.at((pending.netuid,)),
            st::SUBNET_EPOCH_CONSENSUS.at((pending.netuid,)),
        ))
        .await?;
    let Some(uid) = uid else {
        return Ok(Some(RevealMissReason::NotRegistered));
    };
    if owner.raw() != &pending.hotkey {
        let graph = snapshot
            .subnets()
            .selective_metagraph_with(
                pending.netuid,
                pending.mechanism,
                &[
                    rtao_runtime::SelectiveMetagraphIndex::Hotkeys,
                    rtao_runtime::SelectiveMetagraphIndex::TotalStake,
                ],
                MetagraphOptions::default(),
            )
            .await?;
        if let Some(graph) = graph {
            if let Some(neuron) = graph.by_hotkey(&pending.hotkey) {
                if neuron.total_stake()?.0 < threshold {
                    return Ok(Some(RevealMissReason::Stake));
                }
            } else {
                return Ok(Some(RevealMissReason::NotRegistered));
            }
        } else {
            anyhow::bail!("reveal stake snapshot is unavailable");
        }
    }
    if pending.version_key < version {
        return Ok(Some(RevealMissReason::VersionKey));
    }
    let self_only = pending.uids == [Uid(uid)];
    let null_consensus = super::consensus_null(&consensus)?;
    let yuma_owner = !null_consensus && owner.raw() == &pending.hotkey;
    if (null_consensus || (!self_only && !yuma_owner))
        && permits.get(usize::from(uid)) != Some(&true)
    {
        return Ok(Some(RevealMissReason::Permit));
    }
    if pending.uids.iter().any(|uid| uid.0 >= population) {
        return Ok(Some(RevealMissReason::Uid));
    }
    if pending.uids.len() > usize::from(population)
        || (!self_only && pending.uids.len() < usize::from(min.min(population)))
    {
        return Ok(Some(RevealMissReason::Length));
    }
    Ok(None)
}
fn event_netuid(attributes: &Value) -> Option<u16> {
    let value = attributes.field("netuid").or_else(|| match attributes {
        Value::List(fields) | Value::Tuple(fields) => fields.first(),
        _ => None,
    })?;
    u16::try_from(value.as_u128()?).ok()
}
fn reveal_matches(attributes: &Value, index: NetUidStorageIndex, hotkey: AccountId) -> bool {
    let (netuid, who) = match attributes {
        Value::List(fields) | Value::Tuple(fields) if fields.len() == 2 => (&fields[0], &fields[1]),
        Value::Dict(_) => match (
            attributes
                .field("netuid_index")
                .or_else(|| attributes.field("netuid")),
            attributes.field("who"),
        ) {
            (Some(index), Some(who)) => (index, who),
            _ => return false,
        },
        _ => return false,
    };
    let index_matches = netuid.as_u128() == Some(u128::from(index.0));
    let key_matches = who.value_bytes().as_deref() == Some(hotkey.0.as_slice())
        || who.as_str().is_some_and(|text| text == hotkey.to_string());
    index_matches && key_matches
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;
    use crate::weights::tests::{HOTKEY, fixture, header, seed_at, split_fixture};
    use rtao_primitives::IntoCoreValue;

    async fn slipped() -> anyhow::Result<(rtao_testkit::FakeChain, Client, RevealWatcher, Metrics)>
    {
        let (chain, client) = fixture().await?;
        let (watcher, metrics) = record_slipped(&chain, &client, |_, hash| async move {
            Ok(crate::weights::submit::WeightSubmission {
                block_hash: hash,
                block_number: 110,
            })
        })
        .await?;
        Ok((chain, client, watcher, metrics))
    }

    /// Seed a commit included at block 110 on `chain` (one epoch after the
    /// predicted one), locate it with `locate` and record it.
    async fn record_slipped<F, Fut>(
        chain: &rtao_testkit::FakeChain,
        client: &Client,
        locate: F,
    ) -> anyhow::Result<(RevealWatcher, Metrics)>
    where
        F: FnOnce(Client, rtao_primitives::H256) -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<crate::weights::submit::WeightSubmission>>,
    {
        let best = client.at(BlockRef::Best).await?;
        let state = WeightPreflight::fetch(&best, NetUid(1), MechId(0), HOTKEY).await?;
        let commit = PreparedWeightCommit {
            ciphertext: vec![4, 5, 6],
            reveal_round: 500,
        };
        chain.mutate_state(|state| -> anyhow::Result<()> {
            state.advance(30);
            seed_at(
                state,
                110,
                "SubtensorModule",
                "TimelockedWeightCommits",
                (NetUidStorageIndex(1), 6u64),
                vec![(HOTKEY, 110u64, commit.ciphertext.clone(), 500u64)],
            )?;
            for block in [110, 119, 120] {
                seed_at(
                    state,
                    block,
                    "SubtensorModule",
                    "SubnetEpochIndex",
                    (NetUid(1),),
                    6u64,
                )?;
                seed_at(
                    state,
                    block,
                    "SubtensorModule",
                    "LastEpochBlock",
                    (NetUid(1),),
                    110u64,
                )?;
                seed_at(
                    state,
                    block,
                    "SubtensorModule",
                    "BlocksSinceLastStep",
                    (NetUid(1),),
                    block.saturating_sub(110),
                )?;
            }
            seed_at(
                state,
                129,
                "SubtensorModule",
                "SubnetEpochIndex",
                (NetUid(1),),
                7u64,
            )?;
            seed_at(
                state,
                129,
                "SubtensorModule",
                "LastEpochBlock",
                (NetUid(1),),
                120u64,
            )?;
            seed_at(
                state,
                129,
                "SubtensorModule",
                "BlocksSinceLastStep",
                (NetUid(1),),
                9u64,
            )?;
            seed_at(
                state,
                120,
                "SubtensorModule",
                "SubnetOwnerHotkey",
                (NetUid(1),),
                HOTKEY,
            )?;
            seed_at(state, 120, "Drand", "LastStoredRound", (), 499u64)?;
            seed_at(state, 129, "Drand", "LastStoredRound", (), 499u64)?;
            seed_at(state, 130, "Drand", "LastStoredRound", (), 501u64)?;
            Ok(())
        })?;
        let included = header(chain, 110)?;
        let report = locate(client.clone(), included.hash).await?;
        let metrics = Metrics::default();
        let mut watcher = RevealWatcher::default();
        watcher
            .record(
                client,
                NetUid(1),
                MechId(0),
                HOTKEY,
                (vec![Uid(1)], vec![65535]),
                0,
                state,
                commit,
                &report,
                &metrics,
            )
            .await?;
        Ok((watcher, metrics))
    }

    /// With a separate submission endpoint, the commit's inclusion block may
    /// exist only on that endpoint. Locating and recording it still succeeds.
    #[tokio::test]
    async fn split_endpoint_commit_is_located_and_recorded_while_reads_lag() -> anyhow::Result<()> {
        let (read, node, client) = split_fixture().await?;
        let (watcher, metrics) = record_slipped(&node, &client, |client, hash| async move {
            // The read endpoint has not imported the inclusion block.
            assert!(client.at(BlockRef::Hash(hash)).await.is_err());
            crate::weights::submit::inclusion(&client, hash).await
        })
        .await?;
        assert!(header(&read, 110).is_err(), "the read endpoint lags");
        assert_eq!(watcher.pending(), 1);
        assert!(watcher.unlocated.is_empty());
        assert_eq!(watcher.pending[0].inclusion_block, 110);
        assert_eq!(watcher.pending[0].commit_epoch, 6);
        assert_eq!(watcher.pending[0].reveal_epoch, 7);
        assert!(
            metrics
                .render()
                .contains("rtao_weights_commit_epoch_slip_total 1")
        );
        client.close().await;
        Ok(())
    }
    fn success_event() -> serde_json::Value {
        serde_json::json!([{"phase":"Initialization","event":{"SubtensorModule":{"TimelockedWeightsRevealed":[1,HOTKEY.to_string()]}},"topics":[]}])
    }

    #[tokio::test]
    async fn inclusion_slip_follows_actual_epoch_and_never_counts_a_miss() -> anyhow::Result<()> {
        let (chain, client, mut watcher, metrics) = slipped().await?;
        assert_eq!(watcher.pending(), 1);
        assert_eq!(watcher.pending[0].commit_epoch, 6);
        assert_eq!(watcher.pending[0].reveal_epoch, 7);
        assert!(
            metrics
                .render()
                .contains("rtao_weights_commit_epoch_slip_total 1")
        );
        chain.mutate_state(|state| {
            let hash = state.block_hash(110).expect("included hash");
            state
                .seed_events(hash, success_event())
                .expect("seed another epoch's reveal");
        });
        watcher
            .observe(&client, &header(&chain, 110)?, &metrics)
            .await?;
        assert_eq!(
            watcher.pending(),
            1,
            "earlier epoch's same-account event cannot complete this commit"
        );
        chain.mutate_state(|state| {
            let hash = state.block_hash(120).expect("actual reveal hash");
            state
                .seed_events(hash, success_event())
                .expect("seed actual reveal");
        });
        watcher
            .observe(&client, &header(&chain, 120)?, &metrics)
            .await?;
        assert_eq!(watcher.pending(), 0);
        let result = watcher.outcomes().front().expect("outcome");
        assert_eq!(result.commit_epoch, 6);
        assert_eq!(result.reveal_epoch, 7);
        assert_eq!(result.observed_block, 120);
        assert!(result.missed.is_none());
        watcher
            .observe(&client, &header(&chain, 130)?, &metrics)
            .await?;
        assert!(
            !metrics
                .render()
                .contains("rtao_weights_reveal_missed_total")
        );
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn round_arriving_after_actual_epoch_is_still_attributed_to_drand() -> anyhow::Result<()>
    {
        let (chain, client, mut watcher, metrics) = slipped().await?;
        chain.mutate_state(|state| {
            seed_at(
                state,
                120,
                "SubtensorModule",
                "ValidatorPermit",
                (NetUid(1),),
                vec![false, false, false],
            )
        })?;
        watcher
            .observe(&client, &header(&chain, 120)?, &metrics)
            .await?;
        assert_eq!(watcher.pending(), 1);
        watcher
            .observe(&client, &header(&chain, 130)?, &metrics)
            .await?;
        assert_eq!(
            watcher.outcomes().front().expect("miss").missed,
            Some(RevealMissReason::DrandRound)
        );
        assert!(
            metrics
                .render()
                .contains("rtao_weights_reveal_missed_total{mech=\"0\",reason=\"drand_round\"} 1")
        );
        client.close().await;

        let (chain, client, mut watcher, metrics) = slipped().await?;
        chain.mutate_state(|state| -> anyhow::Result<()> {
            seed_at(
                state,
                120,
                "SubtensorModule",
                "ValidatorPermit",
                (NetUid(1),),
                vec![false, false, false],
            )?;
            seed_at(
                state,
                120,
                "SubtensorModule",
                "SubnetEpochConsensus",
                (NetUid(1),),
                "Null",
            )
        })?;
        watcher
            .observe(&client, &header(&chain, 120)?, &metrics)
            .await?;
        watcher
            .observe(&client, &header(&chain, 130)?, &metrics)
            .await?;
        assert_eq!(
            watcher.outcomes().front().expect("Null miss").missed,
            Some(RevealMissReason::Permit)
        );
        client.close().await;

        let (chain, client, mut watcher, metrics) = slipped().await?;
        chain.mutate_state(|state| -> anyhow::Result<()> {
            seed_at(
                state,
                120,
                "SubtensorModule",
                "ValidatorPermit",
                (NetUid(1),),
                vec![false, false, false],
            )?;
            seed_at(
                state,
                120,
                "SubtensorModule",
                "SubnetOwnerHotkey",
                (NetUid(1),),
                AccountId([8; 32]),
            )?;
            seed_at(state, 120, "SubtensorModule", "StakeThreshold", (), 0u64)?;
            state.seed_runtime(
                "SubnetInfoRuntimeApi",
                "get_selective_mechagraph",
                rtao_core::codec::value::to_corpus_json(&Value::record(vec![
                    ("netuid".into(), Value::Int(1)),
                    (
                        "hotkeys".into(),
                        Value::List(vec![Value::Bytes(HOTKEY.0.to_vec())]),
                    ),
                    ("total_stake".into(), Value::List(vec![Value::Int(0)])),
                ])),
            )?;
            Ok(())
        })?;
        watcher
            .observe(&client, &header(&chain, 120)?, &metrics)
            .await?;
        watcher
            .observe(&client, &header(&chain, 130)?, &metrics)
            .await?;
        assert_eq!(
            watcher
                .outcomes()
                .front()
                .expect("Yuma nonowner miss")
                .missed,
            Some(RevealMissReason::Permit)
        );
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn first_reveal_snapshot_attributes_version_or_deregistration_before_round()
    -> anyhow::Result<()> {
        let (chain, client, mut watcher, metrics) = slipped().await?;
        chain.mutate_state(|state| {
            seed_at(
                state,
                120,
                "SubtensorModule",
                "WeightsVersionKey",
                (NetUid(1),),
                2u64,
            )
        })?;
        watcher
            .observe(&client, &header(&chain, 120)?, &metrics)
            .await?;
        watcher
            .observe(&client, &header(&chain, 130)?, &metrics)
            .await?;
        assert_eq!(
            watcher.outcomes().front().expect("version miss").missed,
            Some(RevealMissReason::VersionKey)
        );
        client.close().await;

        let (chain, client, mut watcher, metrics) = slipped().await?;
        chain.mutate_state(|state| -> anyhow::Result<()> {
            let info = state
                .runtime
                .storage_entry("SubtensorModule", "Uids")
                .ok_or_else(|| anyhow::anyhow!("fixture Uids missing"))?;
            let key = state
                .runtime
                .storage_key(
                    info,
                    &[NetUid(1).into_core_value(), HOTKEY.into_core_value()],
                )
                .map_err(rtao_core::CoreError::from)?;
            let hash = state
                .block_hash(120)
                .ok_or_else(|| anyhow::anyhow!("fixture block missing"))?;
            state.seed_at(hash, key, None);
            Ok(())
        })?;
        watcher
            .observe(&client, &header(&chain, 120)?, &metrics)
            .await?;
        watcher
            .observe(&client, &header(&chain, 130)?, &metrics)
            .await?;
        assert_eq!(
            watcher
                .outcomes()
                .front()
                .expect("registration miss")
                .missed,
            Some(RevealMissReason::NotRegistered)
        );
        client.close().await;
        Ok(())
    }

    #[test]
    fn named_and_positional_event_fields_match_only_the_expected_mechanism_and_hotkey() {
        let event = Value::Tuple(vec![Value::Int(4097), Value::Str(HOTKEY.to_string())]);
        assert!(reveal_matches(&event, NetUidStorageIndex(4097), HOTKEY));
        assert!(!reveal_matches(&event, NetUidStorageIndex(1), HOTKEY));
        assert!(!reveal_matches(
            &event,
            NetUidStorageIndex(4097),
            AccountId([8; 32])
        ));
        assert_eq!(
            event_netuid(&Value::record(vec![("netuid".into(), Value::Int(1))])),
            Some(1)
        );
    }

    #[tokio::test]
    async fn accepted_commit_survives_a_temporary_inclusion_queue_read_failure()
    -> anyhow::Result<()> {
        let (chain, client, _, _) = slipped().await?;
        let preparation = client.at(BlockRef::Number(100)).await?;
        let state = WeightPreflight::fetch(&preparation, NetUid(1), MechId(0), HOTKEY).await?;
        let included = header(&chain, 110)?;
        let report = crate::weights::submit::WeightSubmission {
            block_hash: included.hash,
            block_number: 110,
        };
        chain.mutate_state(|state| {
            state.queue_reply(
                "state_queryStorageAt",
                rtao_testkit::Reply::error(-32603, "temporary inclusion queue error", None),
            )
        });
        let mut watcher = RevealWatcher::default();
        let metrics = Metrics::default();
        assert!(
            watcher
                .record(
                    &client,
                    NetUid(1),
                    MechId(0),
                    HOTKEY,
                    (vec![Uid(1)], vec![65535]),
                    0,
                    state,
                    PreparedWeightCommit {
                        ciphertext: vec![4, 5, 6],
                        reveal_round: 500
                    },
                    &report,
                    &metrics
                )
                .await
                .is_err()
        );
        assert_eq!(watcher.pending(), 1);
        assert_eq!(watcher.unlocated.len(), 1);
        watcher.observe(&client, &included, &metrics).await?;
        assert_eq!(watcher.unlocated.len(), 0);
        assert_eq!(watcher.pending(), 1);
        assert_eq!(watcher.pending[0].reveal_epoch, 7);
        watcher.observe(&client, &included, &metrics).await?;
        assert_eq!(
            metrics
                .render()
                .matches("rtao_weights_commit_epoch_slip_total 1")
                .count(),
            1
        );
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn finalized_history_recovers_a_reveal_emitted_during_reconciliation()
    -> anyhow::Result<()> {
        let (chain, client, mut watcher, metrics) = slipped().await?;
        chain.mutate_state(|state| {
            let hash = state.block_hash(120).expect("actual reveal hash");
            state
                .seed_events(hash, success_event())
                .expect("historical reveal event");
        });
        watcher
            .observe(&client, &header(&chain, 130)?, &metrics)
            .await?;
        assert_eq!(watcher.pending(), 0);
        assert_eq!(
            watcher
                .outcomes()
                .front()
                .expect("recovered reveal")
                .observed_block,
            120
        );
        assert!(
            watcher
                .outcomes()
                .front()
                .expect("recovered reveal")
                .missed
                .is_none()
        );
        assert!(
            metrics
                .render()
                .contains("rtao_weights_reveal_history_replayed_total")
        );
        assert!(
            !metrics
                .render()
                .contains("rtao_weights_reveal_missed_total")
        );
        client.close().await;
        Ok(())
    }
}
