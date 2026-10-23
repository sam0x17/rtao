//! Weight-specific projection over the generic operation journal.
use super::reveal::RevealPreparation;
use super::{RevealWatcher, Slot, WeightPreflight, WeightScheduler, WeightsInput};
use rtao_client::ViewExt;
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
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Material {
    pub row: (Vec<Uid>, Vec<u16>),
    #[serde(with = "optional_commit")]
    pub commit: Option<PreparedWeightCommit>,
    pub preparation: RevealPreparation,
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
    pub fn enqueue(
        &self,
        id: impl Into<String>,
        correlation_id: Option<String>,
        mechanism: MechId,
        input: WeightsInput,
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
            if record.intent.semantic_policy == self.semantic_policy
                && let Some(guard) = &self.semantic_guard
            {
                binding = binding.with_guard(guard.clone());
            }
            result = Some((binding, work));
        }
        Ok(result)
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
        let epoch = snapshot
            .storage(&super::st::SUBNET_EPOCH_INDEX, (d.netuid,))
            .await?;
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
    /// at most 128 transaction blocks per active operation and 256 reveal blocks.
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
        self.reveals.observe(client, finalized, metrics).await?;
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
                    let inclusion = super::submit::inclusion(
                        client,
                        report
                            .block_hash
                            .ok_or_else(|| anyhow::anyhow!("missing finalized block"))?,
                    )
                    .await?;
                    self.record_inclusion(client, mech, &inclusion).await?;
                    if report.success {
                        let record = binding.record()?;
                        let attempt = record
                            .attempts
                            .last()
                            .ok_or_else(|| anyhow::anyhow!("finalized operation has no attempt"))?;
                        let material: Material = serde_json::from_value(attempt.material.clone())?;
                        if let Some(commit) = material.commit {
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
