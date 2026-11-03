//! Weight-specific projection over the generic operation journal.
use super::reveal::RevealPreparation;
use super::{OwnerRowPolicy, RevealWatcher, Slot, WeightPreflight, WeightScheduler, WeightsInput};
use rtao_client::journal::{
    JournalBinding, JournalOwner, OperationIntent, OperationJournal, RecoveryStatus, SemanticGuard,
};
use rtao_intents::weights::PreparedWeightCommit;
use rtao_primitives::{AccountId, MechId, NetUid, Uid};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

pub(super) struct Durable {
    pub journal: Arc<OperationJournal>,
    pub scope: String,
    pub genesis: rtao_primitives::H256,
    pub account: AccountId,
    pub netuid: NetUid,
    pub _owner: JournalOwner,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    schema: u32,
    slots: BTreeMap<MechId, Slot>,
    reveals: RevealWatcher,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Work {
    pub netuid: NetUid,
    pub mechanism: MechId,
    pub input: WeightsInput,
    pub version_key: u64,
    /// Owner-row policy the work was queued under (D-093). It is part of the
    /// operation's immutable effects. Omitted for the default
    /// [`OwnerRowPolicy::Hold`], so default work keeps its exact bytes and
    /// effects written before the field existed load as `Hold`.
    #[serde(default, skip_serializing_if = "OwnerRowPolicy::is_hold")]
    pub owner_rows: OwnerRowPolicy,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Material {
    pub row: (Vec<Uid>, Vec<u16>),
    #[serde(with = "optional_commit")]
    pub commit: Option<PreparedWeightCommit>,
    pub preparation: RevealPreparation,
    /// Contract of the application call encoder that composed this attempt's
    /// call (D-088). Omitted for compiled and name-resolved calls, so their
    /// material keeps its bytes. It is attempt evidence, never part of the
    /// operation's identity, so installing or removing an encoder never
    /// changes an operation ID's effects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_encoder: Option<String>,
}
/// Contract of the application encoder that composed the latest signed
/// attempt of `record` that one composed, if any (D-088).
pub(super) fn signed_encoder_contract(
    record: &rtao_client::journal::OperationRecord,
) -> anyhow::Result<Option<String>> {
    for attempt in record.attempts.iter().rev() {
        match attempt.material.get("call_encoder") {
            None => {}
            Some(serde_json::Value::String(contract)) => return Ok(Some(contract.clone())),
            Some(_) => anyhow::bail!(
                "retained weight attempt {} has a malformed call encoder contract",
                attempt.id
            ),
        }
    }
    Ok(None)
}
pub(super) mod commit_codec {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        c: &PreparedWeightCommit,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        (&c.ciphertext, c.reveal_round).serialize(s)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> Result<PreparedWeightCommit, D::Error> {
        let (ciphertext, reveal_round) = Deserialize::deserialize(d)?;
        Ok(PreparedWeightCommit {
            ciphertext,
            reveal_round,
        })
    }
}
mod optional_commit {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        c: &Option<PreparedWeightCommit>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        c.as_ref()
            .map(|c| (&c.ciphertext, c.reveal_round))
            .serialize(s)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> Result<Option<PreparedWeightCommit>, D::Error> {
        let c: Option<(Vec<u8>, u64)> = Deserialize::deserialize(d)?;
        Ok(c.map(|(ciphertext, reveal_round)| PreparedWeightCommit {
            ciphertext,
            reveal_round,
        }))
    }
}
impl WeightScheduler {
    /// Restore durable scheduling and acquire exclusive ownership of this validator.
    ///
    /// The native backend additionally locks against other processes. Identity and
    /// unknown schemas fail closed. Startup reconciles before submitting anything.
    pub fn with_journal(
        mut self,
        journal: Arc<OperationJournal>,
        genesis: rtao_primitives::H256,
        netuid: NetUid,
        hotkey: AccountId,
    ) -> anyhow::Result<Self> {
        let scope = format!("weights:{}:{netuid}:{hotkey}", hex::encode(genesis.0));
        let owner = journal.claim(scope.clone())?;
        if let Some(saved) = journal.snapshot()?.checkpoints.get(&scope) {
            let checkpoint: Checkpoint = serde_json::from_value(saved.clone())?;
            anyhow::ensure!(
                checkpoint.schema == 1,
                "unsupported weight checkpoint; retain journal"
            );
            self.slots = checkpoint.slots;
            self.reveals = checkpoint.reveals;
        }
        self.durable = Some(Durable {
            journal,
            scope,
            genesis,
            account: hotkey,
            netuid,
            _owner: owner,
        });
        Ok(self)
    }
    /// Add application-correlated work without tying its lifetime to an epoch.
    ///
    /// Reusing the same ID and effect is idempotent. A second outstanding operation
    /// for a mechanism is refused; inspect/reconcile the earlier one first.
    /// The work is prepared under [`OwnerRowPolicy::Hold`]; see
    /// [`Self::enqueue_with_owner_rows`].
    pub fn enqueue(
        &self,
        id: impl Into<String>,
        correlation_id: Option<String>,
        mechanism: MechId,
        input: WeightsInput,
    ) -> anyhow::Result<()> {
        self.enqueue_with_owner_rows(id, correlation_id, mechanism, input, OwnerRowPolicy::Hold)
    }
    /// [`Self::enqueue`] with the [`OwnerRowPolicy`] the work is prepared
    /// under (D-093).
    ///
    /// The policy is part of the operation's immutable effects, journaled as
    /// `owner_rows` and omitted for [`OwnerRowPolicy::Hold`]. Restart,
    /// recovery and every re-preparation of the operation decide from it.
    /// Reusing the same ID, input and policy is idempotent; the same ID with
    /// another policy is refused like changed input.
    ///
    /// Work queued here is journaled before the guard runs, so a row the
    /// policy holds stays pending and blocks the mechanism, and no scheduler
    /// API retires it; check [`OwnerRowPolicy::holds`] first when queuing
    /// directly. (A callback proposal the guard holds is not journaled.) An
    /// earlier release cannot read effects that carry the policy and then
    /// refuses every mechanism of the scope, so let `Submit` work finish
    /// before rolling back.
    pub fn enqueue_with_owner_rows(
        &self,
        id: impl Into<String>,
        correlation_id: Option<String>,
        mechanism: MechId,
        input: WeightsInput,
        owner_rows: OwnerRowPolicy,
    ) -> anyhow::Result<()> {
        MechId::new(mechanism.0)?;
        let d = self
            .durable
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("enqueue requires a durable journal"))?;
        let id = id.into();
        let work = Work {
            netuid: d.netuid,
            mechanism,
            input,
            version_key: self.policy.version_key.value(),
            owner_rows,
        };
        let effects = serde_json::to_value(&work)?;
        // JSON encodes nonfinite scores as null. Refuse lossy input before
        // inserting an immutable operation that could not be restored later.
        let _: Work = serde_json::from_value(effects.clone())?;
        if let Some((binding, _)) = self.pending_work(mechanism)? {
            anyhow::ensure!(
                binding.id() == id,
                "reconcile the existing mechanism operation before queuing another"
            );
        }
        d.journal.begin(OperationIntent {
            id,
            correlation_id,
            genesis: d.genesis,
            account: d.account,
            scope: d.scope.clone(),
            effects,
            obligation: false,
            semantic_policy: self.semantic_policy.clone(),
            extensions: Default::default(),
        })?;
        Ok(())
    }
    /// Install an application semantic contract and its current adapter.
    /// Previously queued work retains its contract token across software restarts.
    pub fn with_semantic_guard(
        mut self,
        contract: impl Into<String>,
        guard: Arc<dyn SemanticGuard>,
    ) -> Self {
        self.semantic_policy = Some(contract.into());
        self.semantic_guard = Some(guard);
        self
    }
    /// Install a signed-inner protocol adapter for journaled shield dispatch
    /// and retained shield recovery, replacing the default. Without one, a
    /// scheduler using [`Self::with_shielded_submission`], or holding retained
    /// shielded work under the native profile, binds
    /// [`rtao_client::journal::NativeShieldProtocol`]. Work retained under
    /// another profile reports the adapter as required until it is
    /// reinstalled after restart. It never establishes application economics.
    pub fn with_shield_protocol(
        mut self,
        guard: Arc<dyn rtao_client::journal::ShieldProtocolGuard>,
    ) -> Self {
        self.shield_protocol = Some(guard);
        self
    }
    pub(super) fn pending_work(
        &self,
        mechanism: MechId,
    ) -> anyhow::Result<Option<(JournalBinding, Work)>> {
        let Some(d) = &self.durable else {
            return Ok(None);
        };
        let snapshot = d.journal.snapshot()?;
        let mut result = None;
        for record in snapshot.operations.values().filter(|r| {
            r.intent.scope == d.scope
                && r.extensions.get("scheduler_handed_off") != Some(&serde_json::Value::Bool(true))
        }) {
            let work: Work = serde_json::from_value(record.intent.effects.clone())?;
            anyhow::ensure!(
                record.intent.genesis == d.genesis
                    && record.intent.account == d.account
                    && work.netuid == d.netuid,
                "durable work identity mismatch"
            );
            if work.mechanism != mechanism {
                continue;
            }
            anyhow::ensure!(
                result.is_none(),
                "multiple pending mechanism operations; retain journal for reconciliation"
            );
            let mut binding = d.journal.bind(&record.intent.id)?;
            if let Some(guard) = self.guard_for(&record.intent) {
                binding = binding.with_guard(guard);
            }
            // The native default binds only work it could have created: a
            // record retained under another adapter's profile stays unbound,
            // so recovery reports that adapter as required.
            let native = match &record.mode {
                rtao_client::journal::ExecutionMode::Shielded(policy) => {
                    policy.protocol == rtao_client::journal::NativeShieldProtocol::PROFILE
                }
                _ => self.shield_period.is_some(),
            };
            if let Some(guard) = &self.shield_protocol {
                binding = binding.with_shield_protocol(guard.clone());
            } else if native {
                binding = binding
                    .with_shield_protocol(Arc::new(rtao_client::journal::NativeShieldProtocol));
            }
            result = Some((binding, work));
        }
        Ok(result)
    }
    /// The semantic guard a binding of this operation runs: the installed
    /// one, and only when the operation was queued under the scheduler's
    /// semantic contract.
    fn guard_for(&self, intent: &OperationIntent) -> Option<Arc<dyn SemanticGuard>> {
        self.semantic_guard
            .clone()
            .filter(|_| intent.semantic_policy == self.semantic_policy)
    }
    /// Whether this mechanism's next attempt dispatches through a journal
    /// binding that runs the installed semantic guard (D-088). Only such an
    /// attempt may carry an application encoder's declared calls: a
    /// non-durable scheduler never runs the guard, and neither does a
    /// binding of work queued under another semantic contract or none.
    pub(super) fn semantic_guard_runs(&self, mechanism: MechId) -> anyhow::Result<bool> {
        Ok(match self.pending_work(mechanism)? {
            Some((binding, _)) => self.guard_for(&binding.record()?.intent).is_some(),
            None => false,
        })
    }
    pub(super) fn persist(&self, handed_off: Option<&str>) -> anyhow::Result<()> {
        let Some(d) = &self.durable else {
            return Ok(());
        };
        let value = serde_json::json!({"schema":1,"slots":self.slots,"reveals":self.reveals});
        d.journal.update(|image| {
            image.checkpoints.insert(d.scope.clone(), value);
            if let Some(id) = handed_off {
                image
                    .operations
                    .get_mut(id)
                    .ok_or(rtao_client::journal::JournalError::Identity)?
                    .extensions
                    .insert("scheduler_handed_off".into(), serde_json::Value::Bool(true));
            }
            Ok(())
        })?;
        Ok(())
    }
    pub(super) async fn record_inclusion(
        &mut self,
        client: &rtao_client::Client,
        mech: MechId,
        inclusion: &super::submit::WeightSubmission,
    ) -> anyhow::Result<()> {
        let Some(d) = &self.durable else {
            return Ok(());
        };
        let snapshot = client.at_receipt(inclusion.block_hash).await?;
        anyhow::ensure!(
            snapshot.block().number == inclusion.block_number,
            "inclusion height mismatch"
        );
        let epoch =
            super::reads::storage(&snapshot, &super::st::SUBNET_EPOCH_INDEX, (d.netuid,)).await?;
        let slot = self.slots.entry(mech).or_default();
        // Preparation can precede inclusion by several epochs. Retain the
        // observed inclusion, so restart cannot reopen that cadence slot.
        if slot
            .last_attempt_block
            .is_none_or(|block| block <= inclusion.block_number)
        {
            slot.last_attempt_block = Some(inclusion.block_number);
            slot.last_attempt_epoch = Some(epoch);
        }
        Ok(())
    }
    /// Recover without obtaining a private signer or authorizing any submission.
    /// Observations continue while new submissions are paused. Each call scans
    /// at most 128 transaction blocks per active operation and reads at most
    /// 256 reveal blocks.
    pub async fn recover_finalized(
        &mut self,
        client: &rtao_client::Client,
        finalized: &rtao_client::BlockHeader,
        metrics: &crate::Metrics,
    ) -> anyhow::Result<()> {
        let tip = client.at(rtao_client::BlockRef::Finalized).await?;
        anyhow::ensure!(
            finalized.number <= tip.block().number,
            "recovery head is not finalized"
        );
        let expected = client
            .at(rtao_client::BlockRef::Number(finalized.number))
            .await?;
        anyhow::ensure!(
            expected.block().hash == finalized.hash,
            "recovery head differs from finalized history"
        );
        let recovery = self.recover_work(client, metrics).await;
        // The verified head snapshot spares the watcher resolving it again.
        self.reveals
            .observe_at(client, finalized, Some(expected), metrics)
            .await?;
        self.persist(None)?;
        recovery
    }
    pub(super) async fn recover_work(
        &mut self,
        client: &rtao_client::Client,
        metrics: &crate::Metrics,
    ) -> anyhow::Result<()> {
        let Some(d) = &self.durable else {
            return Ok(());
        };
        anyhow::ensure!(
            client.genesis() == d.genesis,
            "scheduler context differs from durable identity"
        );
        let mechanisms = d
            .journal
            .snapshot()?
            .operations
            .values()
            .filter(|r| {
                r.intent.scope == d.scope
                    && r.extensions.get("scheduler_handed_off")
                        != Some(&serde_json::Value::Bool(true))
            })
            .map(|r| serde_json::from_value::<Work>(r.intent.effects.clone()).map(|w| w.mechanism))
            .collect::<Result<Vec<_>, _>>()?;
        for mech in mechanisms {
            let Some((binding, work)) = self.pending_work(mech)? else {
                continue;
            };
            let status = binding.recover(client, 128).await?;
            match status {
                RecoveryStatus::Rebuildable => {
                    self.slots.entry(mech).or_default().uncertain = false;
                }
                RecoveryStatus::Unresolved(reason) => {
                    self.slots.entry(mech).or_default().uncertain = true;
                    self.attempts.insert(
                        mech,
                        super::WeightAttempt {
                            mechanism: mech,
                            best_block: 0,
                            outcome: super::WeightAttemptOutcome::Unresolved(format!(
                                "recovery pending: {reason:?}"
                            )),
                        },
                    );
                }
                RecoveryStatus::Finalized(report) => {
                    // A paired journal's vector tail is the carrier, whose
                    // receipt and material never establish the weight effect.
                    // Resolve the unique physical effect before mutating the
                    // scheduler or handing any commitment to RevealWatcher.
                    let record = binding.record()?;
                    let attempt = record
                        .finalized_effect()?
                        .filter(|attempt| attempt.hash == report.hash)
                        .ok_or_else(|| {
                            anyhow::anyhow!("finalized weight effect identity differs")
                        })?;
                    let material: Material = serde_json::from_value(attempt.material.clone())?;
                    let inclusion = super::submit::inclusion(
                        client,
                        report
                            .block_hash
                            .ok_or_else(|| anyhow::anyhow!("missing finalized block"))?,
                    )
                    .await?;
                    self.record_inclusion(client, mech, &inclusion).await?;
                    if report.success
                        && let Some(commit) = material.commit
                    {
                        self.reveals
                            .record_prepared(
                                client,
                                work.netuid,
                                mech,
                                record.intent.account,
                                material.row,
                                work.version_key,
                                material.preparation,
                                commit,
                                &inclusion,
                                metrics,
                            )
                            .await?;
                    }
                    self.slots.entry(mech).or_default().uncertain = false;
                    self.persist(Some(binding.id()))?;
                }
            }
        }
        Ok(())
    }
    pub(super) fn prepare_binding(
        &self,
        mech: MechId,
        row: (Vec<Uid>, Vec<u16>),
        commit: Option<PreparedWeightCommit>,
        preflight: &WeightPreflight,
        call_encoder: Option<&str>,
    ) -> anyhow::Result<Option<JournalBinding>> {
        let Some((binding, _)) = self.pending_work(mech)? else {
            anyhow::ensure!(
                self.durable.is_none(),
                "durable weight dispatch requires a pending logical operation"
            );
            return Ok(None);
        };
        let binding = binding.material(serde_json::to_value(Material {
            row,
            commit,
            preparation: RevealPreparation::from(preflight),
            call_encoder: call_encoder.map(str::to_owned),
        })?)?;
        Ok(Some(binding))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{VersionKey, due};
    use super::*;
    use crate::{ChainClock, ClockGuard, WeightCadence, WeightPolicy};
    use rtao_client::BlockRef;
    use rtao_client::journal::{JournalControl, OperationJournal};
    #[tokio::test]
    async fn cadence_uses_actual_inclusion_epoch_across_restart_while_reads_lag()
    -> anyhow::Result<()> {
        let (_, node, client) = super::super::tests::split_fixture().await?;
        let root = std::path::PathBuf::from(
            std::env::var_os("RTAO_TEST_TMPDIR").ok_or_else(|| anyhow::anyhow!("task scratch"))?,
        )
        .join(format!("inclusion-cadence-{}", std::process::id()));
        std::fs::create_dir_all(&root)?;
        let make = || {
            WeightScheduler::new(
                WeightPolicy::default(),
                Arc::new(ChainClock::new(1_000_000)),
                ClockGuard::default(),
            )
        };
        let journal = OperationJournal::open(root.join("journal.json"))?;
        let hotkey = super::super::tests::HOTKEY;
        let mut scheduler =
            make()?.with_journal(journal.clone(), client.genesis(), NetUid(1), hotkey)?;
        scheduler.slots.insert(
            MechId(0),
            Slot {
                last_attempt_block: Some(100),
                last_attempt_epoch: Some(5),
                uncertain: true,
                ..Default::default()
            },
        );
        node.mutate_state(|state| -> anyhow::Result<()> {
            state.advance(5);
            super::super::tests::seed_at(
                state,
                105,
                "SubtensorModule",
                "SubnetEpochIndex",
                (NetUid(1),),
                7u64,
            )
        })?;
        let hash = super::super::tests::header(&node, 105)?.hash;
        scheduler
            .record_inclusion(
                &client,
                MechId(0),
                &super::super::submit::WeightSubmission {
                    block_hash: hash,
                    block_number: 105,
                },
            )
            .await?;
        scheduler.persist(None)?;
        drop(scheduler);
        drop(journal);
        let journal = OperationJournal::open(root.join("journal.json"))?;
        let mut restored =
            make()?.with_journal(journal.clone(), client.genesis(), NetUid(1), hotkey)?;
        let slot = &restored.slots[&MechId(0)];
        assert_eq!(slot.last_attempt_epoch, Some(7));
        assert_eq!(slot.last_attempt_block, Some(105));
        assert_eq!(client.at(BlockRef::Best).await?.block().number, 100);
        restored.slots.entry(MechId(0)).or_default().uncertain = false;
        let cadence = WeightCadence::OncePerEpoch { offset_blocks: 2 };
        restored.update_policy(WeightPolicy {
            version_key: VersionKey::Fixed(0),
            cadence: Some(cadence),
        });
        let snapshot = client.at(BlockRef::Best).await?;
        let mut view = WeightPreflight::fetch(&snapshot, NetUid(1), MechId(0), hotkey).await?;
        view.rate_limit = 0;
        assert_eq!(
            restored.next_action(MechId(0), hotkey, &view)?,
            super::super::WeightOpportunity::AwaitBlock {
                not_before_block: 105
            }
        );
        let slot = &restored.slots[&MechId(0)];
        assert!(!due(cadence, slot, &view, Uid(0)));
        // A counter rollback remains blocked even after the block view catches up.
        view.at.number = 106;
        view.schedule.subnet_epoch_index = 5;
        assert_eq!(
            restored.next_action(MechId(0), hotkey, &view)?,
            super::super::WeightOpportunity::AwaitEpoch {
                after_epoch: 7,
                offset_blocks: 2
            }
        );
        view.schedule.subnet_epoch_index = 7;
        assert!(!due(cadence, slot, &view, Uid(0)));
        view.schedule.subnet_epoch_index = 8;
        view.schedule.last_epoch_block = 104;
        assert_eq!(
            restored.next_action(MechId(0), hotkey, &view)?,
            super::super::WeightOpportunity::Ready
        );
        // New chain limits and offsets still govern this newly observed epoch.
        view.rate_limit = 2;
        assert_eq!(
            restored.next_action(MechId(0), hotkey, &view)?,
            super::super::WeightOpportunity::RateLimited {
                retry_after_block: 107
            }
        );
        view.at.number = 107;
        view.schedule.last_epoch_block = 107;
        assert_eq!(
            restored.next_action(MechId(0), hotkey, &view)?,
            super::super::WeightOpportunity::AwaitBlock {
                not_before_block: 109
            }
        );
        view.at.number = 109;
        assert!(due(cadence, slot, &view, Uid(0)));
        drop(restored);
        drop(journal);
        client.close().await;
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    /// Default work keeps the exact effects of earlier releases; only an
    /// opted-in policy adds the `owner_rows` key (D-093).
    #[test]
    fn owner_rows_encoding_keeps_default_effects_bytes() -> anyhow::Result<()> {
        let input = WeightsInput::ByUid([(Uid(2), 65535.0)].into_iter().collect());
        let work = |owner_rows| Work {
            netuid: NetUid(1),
            mechanism: MechId(0),
            input: input.clone(),
            version_key: 0,
            owner_rows,
        };
        let legacy = serde_json::json!({"netuid":1,"mechanism":0,"input":input,"version_key":0});
        assert_eq!(serde_json::to_value(work(OwnerRowPolicy::Hold))?, legacy);
        assert_eq!(
            serde_json::to_vec(&work(OwnerRowPolicy::Hold))?,
            serde_json::to_vec(&legacy)?
        );
        let decoded: Work = serde_json::from_value(legacy.clone())?;
        assert_eq!(decoded.owner_rows, OwnerRowPolicy::Hold);
        let opted = serde_json::to_value(work(OwnerRowPolicy::Submit))?;
        assert_eq!(opted["owner_rows"], "submit");
        let decoded: Work = serde_json::from_value(opted.clone())?;
        assert_eq!(decoded.owner_rows, OwnerRowPolicy::Submit);
        assert_eq!(decoded.input, input);
        let mut unknown = opted.clone();
        unknown["owner_rows"] = "submit_all".into();
        assert!(serde_json::from_value::<Work>(unknown).is_err());
        let mut extra = legacy;
        extra["future_field"] = true.into();
        assert!(serde_json::from_value::<Work>(extra).is_err());
        Ok(())
    }

    /// The policy is part of the operation's identity: an ID queued under one
    /// policy refuses another, and the journaled value survives reopening.
    #[test]
    fn owner_rows_is_immutable_operation_identity() -> anyhow::Result<()> {
        let root = std::path::PathBuf::from(
            std::env::var_os("RTAO_TEST_TMPDIR").ok_or_else(|| anyhow::anyhow!("task scratch"))?,
        )
        .join(format!("owner-rows-identity-{}", std::process::id()));
        std::fs::create_dir_all(&root)?;
        let path = root.join("journal.json");
        let make = || {
            WeightScheduler::new(
                WeightPolicy::default(),
                Arc::new(ChainClock::new(1_000_000)),
                ClockGuard::default(),
            )
        };
        let genesis = rtao_primitives::H256([1; 32]);
        let account = AccountId([2; 32]);
        let input = WeightsInput::ByUid([(Uid(2), 65535.0)].into_iter().collect());
        let journal = OperationJournal::open(&path)?;
        let scheduler = make()?.with_journal(journal.clone(), genesis, NetUid(1), account)?;
        let scope = scheduler
            .durable
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("scope"))?
            .scope
            .clone();
        // An operation retained by an earlier release, without the key.
        journal.begin(OperationIntent {
            id: "legacy".into(),
            correlation_id: None,
            genesis,
            account,
            scope: scope.clone(),
            effects: serde_json::json!({"netuid":1,"mechanism":0,"input":input,"version_key":0}),
            obligation: false,
            semantic_policy: None,
            extensions: Default::default(),
        })?;
        let (_, work) = scheduler
            .pending_work(MechId(0))?
            .ok_or_else(|| anyhow::anyhow!("legacy work"))?;
        assert_eq!(work.owner_rows, OwnerRowPolicy::Hold);
        // An idempotent rejoin may publish a new revision, never new records.
        let operations = || -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::to_value(&journal.snapshot()?.operations)?)
        };
        let before = operations()?;
        scheduler.enqueue("legacy", None, MechId(0), input.clone())?;
        scheduler.enqueue_with_owner_rows(
            "legacy",
            None,
            MechId(0),
            input.clone(),
            OwnerRowPolicy::Hold,
        )?;
        assert_eq!(operations()?, before, "idempotent rejoin");
        let bytes = std::fs::read(&path)?;
        let refused = scheduler.enqueue_with_owner_rows(
            "legacy",
            None,
            MechId(0),
            input.clone(),
            OwnerRowPolicy::Submit,
        );
        assert!(
            matches!(
                refused
                    .as_ref()
                    .err()
                    .and_then(|e| e.downcast_ref::<rtao_client::journal::JournalError>()),
                Some(rtao_client::journal::JournalError::Identity)
            ),
            "{refused:?}"
        );
        assert_eq!(std::fs::read(&path)?, bytes, "a refusal writes nothing");
        // Opted-in work on another mechanism: idempotent only under Submit.
        let queue = |id: &str, policy| {
            scheduler.enqueue_with_owner_rows(id, None, MechId(1), input.clone(), policy)
        };
        queue("burn", OwnerRowPolicy::Submit)?;
        let queued = operations()?;
        queue("burn", OwnerRowPolicy::Submit)?;
        assert_eq!(operations()?, queued);
        let bytes = std::fs::read(&path)?;
        assert!(queue("burn", OwnerRowPolicy::Hold).is_err());
        assert!(
            scheduler
                .enqueue("burn", None, MechId(1), input.clone())
                .is_err()
        );
        assert_eq!(std::fs::read(&path)?, bytes);
        let record = journal.bind("burn")?.record()?;
        assert_eq!(record.intent.effects["owner_rows"], "submit");
        // Plain enqueue is the Hold form of the same intent.
        let snapshot = journal.snapshot()?;
        let legacy = &snapshot.operations["legacy"].intent;
        assert!(legacy.effects.get("owner_rows").is_none());
        drop(scheduler);
        drop(journal);
        let journal = OperationJournal::open(&path)?;
        let restored = make()?.with_journal(journal.clone(), genesis, NetUid(1), account)?;
        let (_, burn) = restored
            .pending_work(MechId(1))?
            .ok_or_else(|| anyhow::anyhow!("lost work"))?;
        assert_eq!(burn.owner_rows, OwnerRowPolicy::Submit);
        let (_, legacy) = restored
            .pending_work(MechId(0))?
            .ok_or_else(|| anyhow::anyhow!("lost legacy work"))?;
        assert_eq!(legacy.owner_rows, OwnerRowPolicy::Hold);
        drop(restored);
        drop(journal);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn competing_scheduler_and_unsupported_checkpoint_refuse_without_reset() -> anyhow::Result<()> {
        let root = std::path::PathBuf::from(
            std::env::var_os("RTAO_TEST_TMPDIR").ok_or_else(|| anyhow::anyhow!("task scratch"))?,
        )
        .join(format!("checkpoint-refusal-{}", std::process::id()));
        std::fs::create_dir_all(&root)?;
        let journal = OperationJournal::open(root.join("journal.json"))?;
        let make = || {
            WeightScheduler::new(
                WeightPolicy::default(),
                Arc::new(ChainClock::new(1_000_000)),
                ClockGuard::default(),
            )
        };
        let genesis = rtao_primitives::H256([1; 32]);
        let account = AccountId([2; 32]);
        let scheduler = make()?.with_journal(journal.clone(), genesis, NetUid(1), account)?;
        assert!(
            make()?
                .with_journal(journal.clone(), genesis, NetUid(1), account)
                .is_err()
        );
        scheduler.enqueue(
            "retained-work",
            None,
            MechId(0),
            WeightsInput::Parallel {
                uids: vec![Uid(1)],
                weights: vec![1.0],
            },
        )?;
        let scope = scheduler
            .durable
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("scope"))?
            .scope
            .clone();
        drop(scheduler);
        for saved in [
            serde_json::json!({"schema":99,"slots":{},"reveals":RevealWatcher::default()}),
            serde_json::json!({"schema":1,"slots":{},"reveals":RevealWatcher::default(),"future_evidence":true}),
        ] {
            journal.update(|image| {
                image.checkpoints.insert(scope.clone(), saved);
                Ok(())
            })?;
            let before = std::fs::read(root.join("journal.json"))?;
            assert!(
                make()?
                    .with_journal(journal.clone(), genesis, NetUid(1), account)
                    .is_err()
            );
            assert_eq!(std::fs::read(root.join("journal.json"))?, before);
            assert!(journal.bind("retained-work").is_ok());
        }
        drop(journal);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[tokio::test]
    async fn pending_input_and_uncertainty_survive_pause_missed_epochs_and_parameter_changes()
    -> anyhow::Result<()> {
        let (chain, client) = super::super::tests::fixture().await?;
        let hotkey = super::super::tests::HOTKEY;
        let root = std::path::PathBuf::from(
            std::env::var_os("RTAO_TEST_TMPDIR").ok_or_else(|| anyhow::anyhow!("task scratch"))?,
        )
        .join(format!("durable-work-{}", std::process::id()));
        std::fs::create_dir_all(&root)?;
        let journal = OperationJournal::open(root.join("journal.json"))?;
        let make = || {
            WeightScheduler::new(
                WeightPolicy {
                    version_key: VersionKey::Fixed(42),
                    cadence: Some(WeightCadence::OncePerEpoch { offset_blocks: 2 }),
                },
                Arc::new(ChainClock::new(1_000_000)),
                ClockGuard::default(),
            )
        };
        let mut scheduler =
            make()?.with_journal(journal.clone(), client.genesis(), NetUid(1), hotkey)?;
        let input = WeightsInput::Parallel {
            uids: vec![Uid(1)],
            weights: vec![1.0],
        };
        assert!(
            scheduler
                .enqueue(
                    "nonfinite",
                    None,
                    MechId(0),
                    WeightsInput::Parallel {
                        uids: vec![Uid(1)],
                        weights: vec![f64::NAN],
                    }
                )
                .is_err()
        );
        assert!(journal.snapshot()?.operations.is_empty());
        scheduler.enqueue(
            "application-work",
            Some("result-revision".into()),
            MechId(0),
            input.clone(),
        )?;
        scheduler.slots.insert(
            MechId(0),
            Slot {
                last_attempt_epoch: Some(5),
                last_attempt_block: Some(100),
                uncertain: true,
                ..Default::default()
            },
        );
        scheduler.persist(None)?;
        journal.control(JournalControl {
            submissions_paused: true,
            allow_obligations: false,
            extensions: Default::default(),
        })?;
        scheduler.resolve_uncertainty(MechId(0));
        assert!(scheduler.slots[&MechId(0)].uncertain);
        drop(scheduler);
        drop(journal);
        let journal = OperationJournal::open(root.join("journal.json"))?;
        let mut scheduler =
            make()?.with_journal(journal.clone(), client.genesis(), NetUid(1), hotkey)?;
        let (binding, work) = scheduler
            .pending_work(MechId(0))?
            .ok_or_else(|| anyhow::anyhow!("lost work"))?;
        assert_eq!(work.input, input);
        assert_eq!(work.version_key, 42);
        assert_eq!(
            binding.record()?.intent.correlation_id.as_deref(),
            Some("result-revision")
        );
        assert!(binding.can_dispatch().is_err());
        assert!(
            scheduler
                .enqueue(
                    "application-work",
                    Some("result-revision".into()),
                    MechId(0),
                    input.clone()
                )
                .is_ok()
        );
        assert!(
            scheduler
                .enqueue("different-op", None, MechId(0), input)
                .is_err()
        );
        chain.mutate_state(|state| -> anyhow::Result<()> {
            state.advance(80);
            state.seed(
                &rtao_runtime::storage::subtensor_module::TEMPO,
                (NetUid(1),),
                20u16,
            )?;
            state.seed(
                &rtao_runtime::storage::subtensor_module::LAST_EPOCH_BLOCK,
                (NetUid(1),),
                160u64,
            )?;
            state.seed(
                &rtao_runtime::storage::subtensor_module::SUBNET_EPOCH_INDEX,
                (NetUid(1),),
                9u64,
            )?;
            state.seed(
                &rtao_runtime::storage::subtensor_module::WEIGHTS_SET_RATE_LIMIT,
                (NetUid(1),),
                90u64,
            )?;
            Ok(())
        })?;
        let snapshot = client.at(BlockRef::Best).await?;
        let mut preflight = WeightPreflight::fetch(&snapshot, NetUid(1), MechId(0), hotkey).await?;
        assert_eq!(
            scheduler.next_action(MechId(0), hotkey, &preflight)?,
            super::super::WeightOpportunity::Paused
        );
        let mut slot = Slot {
            last_attempt_epoch: Some(5),
            last_attempt_block: Some(100),
            uncertain: false,
            ..Default::default()
        };
        assert!(!due(
            WeightCadence::RateLimited { margin: 0 },
            &slot,
            &preflight,
            Uid(0)
        ));
        preflight.rate_limit = 10;
        assert!(due(
            WeightCadence::OncePerEpoch { offset_blocks: 2 },
            &slot,
            &preflight,
            Uid(0)
        ));
        slot.last_attempt_epoch = Some(9);
        assert!(!due(
            WeightCadence::OncePerEpoch { offset_blocks: 2 },
            &slot,
            &preflight,
            Uid(0)
        ));
        journal.control(JournalControl::default())?;
        assert!(binding.can_dispatch().is_ok());
        assert_eq!(
            scheduler.next_action(MechId(0), hotkey, &preflight)?,
            super::super::WeightOpportunity::Reconcile
        );
        scheduler
            .recover_work(&client, &crate::Metrics::default())
            .await?;
        assert_eq!(
            scheduler.next_action(MechId(0), hotkey, &preflight)?,
            super::super::WeightOpportunity::Ready
        );
        scheduler
            .slots
            .entry(MechId(0))
            .or_default()
            .last_attempt_epoch = Some(preflight.schedule.subnet_epoch_index);
        assert!(matches!(
            scheduler.next_action(MechId(0), hotkey, &preflight)?,
            super::super::WeightOpportunity::AwaitEpoch { after_epoch: 9, .. }
        ));
        scheduler.update_policy(WeightPolicy::default());
        assert_eq!(
            scheduler
                .pending_work(MechId(0))?
                .ok_or_else(|| anyhow::anyhow!("lost work"))?
                .1
                .version_key,
            42
        );
        drop(binding);
        drop(scheduler);
        drop(journal);
        client.close().await;
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}

#[cfg(test)]
mod handoff_tests {
    use super::*;
    use crate::{ChainClock, ClockGuard, Metrics, WeightPolicy};
    use rtao_client::{BlockRef, SubmitOptions};
    use rtao_runtime::calls::subtensor_module as calls;
    use rtao_signer::{KeypairSigner, Signer};
    #[tokio::test]
    async fn crash_after_finalized_commit_before_scheduler_handoff_recovers_exact_material_once()
    -> anyhow::Result<()> {
        let (chain, client) = super::super::tests::fixture().await?;
        let signer = KeypairSigner::from_seed(&[7; 32], rtao_primitives::CryptoType::Ed25519)?;
        let hotkey = signer.account();
        let root = std::path::PathBuf::from(
            std::env::var_os("RTAO_TEST_TMPDIR").ok_or_else(|| anyhow::anyhow!("task scratch"))?,
        )
        .join(format!("handoff-{}", std::process::id()));
        std::fs::create_dir_all(&root)?;
        let journal = OperationJournal::open(root.join("journal.json"))?;
        let make = || {
            WeightScheduler::new(
                WeightPolicy::default(),
                Arc::new(ChainClock::new(1_000_000)),
                ClockGuard::default(),
            )
        };
        let scheduler =
            make()?.with_journal(journal.clone(), client.genesis(), NetUid(1), hotkey)?;
        scheduler.enqueue(
            "logical-commit",
            Some("caller-revision".into()),
            MechId(0),
            WeightsInput::Parallel {
                uids: vec![Uid(1)],
                weights: vec![1.0],
            },
        )?;
        let best = client.at(BlockRef::Best).await?;
        let preparation =
            WeightPreflight::fetch(&best, NetUid(1), MechId(0), super::super::tests::HOTKEY)
                .await?;
        let commit = PreparedWeightCommit {
            ciphertext: vec![4, 5, 6],
            reveal_round: 500,
        };
        let binding = scheduler
            .prepare_binding(
                MechId(0),
                (vec![Uid(1)], vec![65535]),
                Some(commit.clone()),
                &preparation,
                None,
            )?
            .ok_or_else(|| anyhow::anyhow!("binding"))?;
        let retained = commit.clone();
        chain.mutate_state(|state| {
            state.set_handler("author_submitAndWatchExtrinsic", move |state, request| {
                let reply = state.dispatch(request);
                let number = state.block;
                let result = super::super::tests::seed_at(
                    state,
                    number,
                    "SubtensorModule",
                    "TimelockedWeightCommits",
                    (rtao_primitives::NetUidStorageIndex(1), 5u64),
                    vec![(
                        hotkey,
                        number,
                        retained.ciphertext.clone(),
                        retained.reveal_round,
                    )],
                );
                assert!(result.is_ok());
                reply
            })
        });
        let call = calls::commit_timelocked_mechanism_weights(
            NetUid(1),
            MechId(0),
            commit.ciphertext,
            commit.reveal_round,
            4,
        )
        .compose(&client.refresh_runtime().await?)?;
        let report = client
            .submit(
                call,
                &signer,
                &SubmitOptions {
                    journal: Some(binding.clone()),
                    ..Default::default()
                },
            )
            .await?;
        assert!(report.success);
        assert_eq!(scheduler.reveals.pending(), 0);
        // The receipt was persisted, but no reveal watcher/checkpoint handoff ran.
        drop(binding);
        drop(scheduler);
        drop(journal);
        let journal = OperationJournal::open(root.join("journal.json"))?;
        journal.control(rtao_client::journal::JournalControl {
            submissions_paused: true,
            allow_obligations: false,
            extensions: Default::default(),
        })?;
        let mut restored =
            make()?.with_journal(journal.clone(), client.genesis(), NetUid(1), hotkey)?;
        let number = report
            .block_number
            .ok_or_else(|| anyhow::anyhow!("inclusion height"))?;
        // The fixture's watch can report inclusion ahead of its deliberately
        // lagged finalized read view. Recovery must wait for that view to catch up.
        chain.mutate_state(|state| state.set_finalized(number))?;
        let head = super::super::tests::header(&chain, number)?;
        restored
            .recover_finalized(&client, &head, &Metrics::default())
            .await?;
        assert_eq!(restored.reveals.pending(), 1);
        assert!(restored.pending_work(MechId(0))?.is_none());
        restored
            .recover_finalized(&client, &head, &Metrics::default())
            .await?;
        assert_eq!(restored.reveals.pending(), 1);
        assert_eq!(chain.mutate_state(|state| state.submissions.len()), 1);
        assert_eq!(
            journal
                .bind("logical-commit")?
                .record()?
                .intent
                .correlation_id
                .as_deref(),
            Some("caller-revision")
        );
        drop(restored);
        drop(journal);
        client.close().await;
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "durable_paired_tests.rs"]
mod paired_tests;

#[cfg(test)]
#[path = "durable_public_tests.rs"]
mod public_tests;

#[cfg(test)]
#[path = "durable_readiness_tests.rs"]
mod readiness_tests;

#[cfg(test)]
#[path = "durable_composer_tests.rs"]
mod composer_tests;

#[cfg(test)]
#[path = "durable_owner_row_tests.rs"]
mod owner_row_tests;

#[cfg(test)]
#[path = "durable_clock_tests.rs"]
mod clock_tests;

#[cfg(test)]
#[path = "durable_slot_tests.rs"]
mod slot_tests;
