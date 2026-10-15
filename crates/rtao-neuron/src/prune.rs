//! Predict the next capacity-replacement victim from one pinned chain state.
//!
//! This models non-root registration, including owner protection, the free-UID
//! floor, and the fallback that can replace a neuron still in its immunity
//! period. It does not use the deprecated metagraph `pruning_score` column.
// Behavioral source: RaoFoundation/subtensor @ 850f2755bb05a3851f948489f0ab27b0645cebce,
// pallets/subtensor/src/subnets/registration.rs:469-591, subnets/uids.rs:70-89,
// utils/misc.rs:569-573. Source facts are modeled independently for monitoring.

use std::collections::BTreeSet;

use bytes::Bytes;
use rtao_client::{Client, ClientError, Fetch, ReadSpec, Snapshot, View, ViewExt};
use rtao_primitives::{AccountId, AlphaBalance, NetUid, Uid};
use rtao_runtime::{RuntimeHandle, storage::subtensor_module as st};

/// Exact non-root registration parameters at a single chain block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PruneParameters {
    /// Subnet whose ordinary registration replacement is being predicted.
    pub netuid: NetUid,
    /// Block used to compute registration age.
    pub block: u64,
    /// `SubnetworkN`, including any UID whose Keys entry is missing.
    pub population: u16,
    /// `MaxAllowedUids`; registration appends below this threshold.
    pub max_uids: u16,
    /// Time-based immunity period in blocks.
    pub immunity_period: u16,
    /// Minimum neurons outside the owner's immortal set and time immunity that must remain.
    pub min_non_immune_uids: u16,
    /// Bound on the owner's immortal set, with primary-owner priority.
    pub immune_owner_uids_limit: u16,
    /// Authoritative `SubnetOwner` coldkey, including its ValueQuery default.
    pub owner_coldkey: AccountId,
    /// Present `SubnetOwnerHotkey`, distinguished from a missing storage entry.
    pub owner_hotkey: Option<AccountId>,
    /// The primary owner's UID from `Uids`, if the association is present.
    pub owner_uid: Option<Uid>,
}

/// One populated Keys entry and its authoritative pruning inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PruneNeuron {
    /// Position within this subnet's bounded UID population.
    pub uid: Uid,
    /// Registered public hotkey, including an explicitly stored zero account.
    pub hotkey: AccountId,
    /// Present `Owner` association; missing entries remain absent.
    pub coldkey: Option<AccountId>,
    /// `BlockAtRegistration`, with its ValueQuery zero default.
    pub registered_at: u64,
    /// Exact base-subnet `Emission[uid]`, defaulting to zero for a short vector.
    pub emission: AlphaBalance,
}

/// A hotkey's immunity and risk of replacement by the next registration.
///
/// `prune_risk` is a deterministic zero-or-one indicator for the observed
/// state. It is not a probability or an estimate of future emissions. A
/// positive immunity countdown does not guarantee protection: the pallet can
/// fall back to immune neurons when the non-immune floor prevents pruning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PruneRisk {
    /// Observed subnet.
    pub netuid: NetUid,
    /// Pinned state block used for all inputs.
    pub block: u64,
    /// Hotkey being monitored.
    pub hotkey: AccountId,
    /// Populated UID belonging to this hotkey, or `None` when deregistered.
    pub uid: Option<Uid>,
    /// A populated Keys entry currently belongs to this hotkey.
    pub registered: bool,
    /// Time-based immunity remaining, independent of owner protection.
    pub immunity_blocks_remaining: u64,
    /// Owner protection excludes this hotkey or refuses its replacement.
    pub owner_protected: bool,
    /// Population has reached the capacity at which registration replaces a UID.
    pub at_capacity: bool,
    /// UID selected by `get_neuron_to_prune`, even below capacity.
    ///
    /// Selection alone does not imply replacement. In particular, the pallet
    /// can select a primary owner when its configured immortal limit is zero,
    /// but `replace_neuron` refuses to replace that hotkey.
    pub candidate_uid: Option<Uid>,
    /// Selected candidate remains in its time-based immunity period.
    pub candidate_is_immune: bool,
    /// The next ordinary registration would replace this monitored hotkey.
    pub prune_risk: bool,
}

/// Pruning state could not be read or does not describe a supported subnet.
#[derive(Debug, thiserror::Error)]
pub enum PruneError {
    /// Root registration selects by stake and has no immune fallback.
    #[error("root admission uses stake pruning; this monitor requires a non-root subnet")]
    RootSubnet,
    /// Invalid native subnet identifier.
    #[error(transparent)]
    Identifier(#[from] rtao_primitives::UnitError),
    /// Repeated or out-of-population UIDs make the supplied projection invalid.
    #[error("invalid pruning projection: {0}")]
    InvalidState(String),
    /// Pinned storage access or runtime-shape verification failed.
    #[error(transparent)]
    Client(#[from] ClientError),
}

impl PruneRisk {
    /// Compute the exact non-root pallet selection from public native inputs.
    ///
    /// Only populated Keys entries belong in `neurons`; sparse UIDs are skipped
    /// by the pallet. Rows may be supplied in any order. Their exact integer
    /// emissions, registration blocks, and UIDs determine all tie breaks.
    pub fn evaluate(
        parameters: &PruneParameters,
        neurons: &[PruneNeuron],
        hotkey: AccountId,
    ) -> Result<Self, PruneError> {
        supported_subnet(parameters.netuid)?;
        let mut uids = BTreeSet::new();
        for neuron in neurons {
            if neuron.uid.0 >= parameters.population || !uids.insert(neuron.uid) {
                return Err(PruneError::InvalidState(format!(
                    "UID {} is repeated or outside population {}",
                    neuron.uid, parameters.population
                )));
            }
        }

        // The source sorts ascending, despite a comment describing recent
        // registration as priority. Follow the executable order: oldest, UID.
        let mut owner_rows = neurons
            .iter()
            .filter(|neuron| neuron.coldkey == Some(parameters.owner_coldkey))
            .collect::<Vec<_>>();
        owner_rows.sort_by_key(|neuron| (neuron.registered_at, neuron.uid));
        owner_rows.truncate(usize::from(parameters.immune_owner_uids_limit));
        let mut immortal = owner_rows
            .into_iter()
            .map(|neuron| (neuron.uid, neuron.hotkey))
            .collect::<Vec<_>>();
        if let (Some(owner_hotkey), Some(owner_uid)) =
            (parameters.owner_hotkey, parameters.owner_uid)
            && !immortal.contains(&(owner_uid, owner_hotkey))
        {
            immortal.insert(0, (owner_uid, owner_hotkey));
            immortal.truncate(usize::from(parameters.immune_owner_uids_limit));
        }
        let immortal = immortal
            .into_iter()
            .map(|(_, account)| account)
            .collect::<BTreeSet<_>>();

        let eligible = neurons
            .iter()
            .filter(|neuron| !immortal.contains(&neuron.hotkey))
            .collect::<Vec<_>>();
        let free_count = eligible
            .iter()
            .filter(|neuron| immunity_remaining(parameters, neuron) == 0)
            .count();
        let candidate = if free_count > usize::from(parameters.min_non_immune_uids) {
            eligible
                .iter()
                .copied()
                .filter(|neuron| immunity_remaining(parameters, neuron) == 0)
                .min_by_key(|neuron| (neuron.emission.0, neuron.registered_at, neuron.uid))
        } else {
            None
        }
        .or_else(|| {
            eligible
                .iter()
                .copied()
                .filter(|neuron| immunity_remaining(parameters, neuron) > 0)
                .min_by_key(|neuron| (neuron.emission.0, neuron.registered_at, neuron.uid))
        });
        let watched = neurons
            .iter()
            .filter(|neuron| neuron.hotkey == hotkey)
            .min_by_key(|neuron| neuron.uid);
        let owner_protected = immortal.contains(&hotkey) || parameters.owner_hotkey == Some(hotkey);
        let at_capacity = parameters.population >= parameters.max_uids;
        Ok(Self {
            netuid: parameters.netuid,
            block: parameters.block,
            hotkey,
            uid: watched.map(|neuron| neuron.uid),
            registered: watched.is_some(),
            immunity_blocks_remaining: watched
                .map_or(0, |neuron| immunity_remaining(parameters, neuron)),
            owner_protected,
            at_capacity,
            candidate_uid: candidate.map(|neuron| neuron.uid),
            candidate_is_immune: candidate
                .is_some_and(|neuron| immunity_remaining(parameters, neuron) > 0),
            prune_risk: at_capacity
                && !owner_protected
                && candidate.is_some_and(|neuron| neuron.hotkey == hotkey),
        })
    }

    /// Pin the client's configured head once and read an exact observation.
    pub async fn from_client(
        client: &Client,
        netuid: NetUid,
        hotkey: AccountId,
    ) -> Result<Self, PruneError> {
        supported_subnet(netuid)?;
        Self::observe(&client.pin().await?, netuid, hotkey).await
    }

    /// Read all pruning inputs at this snapshot's immutable state hash.
    ///
    /// The current base-subnet emission vector and authoritative Keys/Owner
    /// storage are used directly. A selective graph can omit these inputs, and
    /// mechanism graphs contain different emission columns, so substituting a
    /// metagraph would change the pallet's selection. Storage queries coalesce
    /// into batches of at most 256 keys and retain actual absence for `try_get`
    /// entries instead of manufacturing zero-account registrations.
    pub async fn observe(
        snapshot: &Snapshot,
        netuid: NetUid,
        hotkey: AccountId,
    ) -> Result<Self, PruneError> {
        supported_subnet(netuid)?;
        let (
            population,
            max_uids,
            immunity_period,
            min_non_immune_uids,
            immune_owner_uids_limit,
            owner_coldkey,
            owner_hotkey,
            emissions,
        ) = snapshot
            .fetch((
                st::SUBNETWORK_N.at((netuid,)),
                st::MAX_ALLOWED_UIDS.at((netuid,)),
                st::IMMUNITY_PERIOD.at((netuid,)),
                st::MIN_NON_IMMUNE_UIDS.at((netuid,)),
                st::IMMUNE_OWNER_UIDS_LIMIT.at((netuid,)),
                st::SUBNET_OWNER.at((netuid,)),
                Present(st::SUBNET_OWNER_HOTKEY.at((netuid,))),
                st::EMISSION.at((netuid,)),
            ))
            .await?;
        let owner_hotkey = owner_hotkey.map(|account| *account.raw());
        let row_queries = (0..population)
            .map(|uid| {
                (
                    Present(st::KEYS.at((netuid, uid))),
                    st::BLOCK_AT_REGISTRATION.at((netuid, uid)),
                )
            })
            .collect();
        let rows = snapshot.fetch(Many(row_queries)).await?;
        let present = rows
            .into_iter()
            .enumerate()
            .filter_map(|(uid, (hotkey, block))| {
                hotkey.map(|account| (uid as u16, *account.raw(), block))
            })
            .collect::<Vec<_>>();
        let owners = snapshot
            .fetch(Many(
                present
                    .iter()
                    .map(|(_, account, _)| Present(st::OWNER.at((*account,))))
                    .collect(),
            ))
            .await?;
        let owner_uid = match owner_hotkey {
            Some(account) => snapshot
                .storage(&st::UIDS, (netuid, account))
                .await?
                .map(Uid),
            None => None,
        };
        let neurons = present
            .into_iter()
            .zip(owners)
            .map(|((uid, hotkey, registered_at), coldkey)| PruneNeuron {
                uid: Uid(uid),
                hotkey,
                coldkey: coldkey.map(|account| *account.raw()),
                registered_at,
                emission: emissions.get(usize::from(uid)).copied().unwrap_or_default(),
            })
            .collect::<Vec<_>>();
        Self::evaluate(
            &PruneParameters {
                netuid,
                block: snapshot.block().number,
                population,
                max_uids,
                immunity_period,
                min_non_immune_uids,
                immune_owner_uids_limit,
                owner_coldkey: *owner_coldkey.raw(),
                owner_hotkey,
                owner_uid,
            },
            &neurons,
            hotkey,
        )
    }

    /// Publish immunity and next-registration risk on the framework recorder.
    pub fn publish_metrics(&self, metrics: &crate::Metrics) {
        metrics.gauge(
            "rtao_immunity_blocks_remaining",
            self.immunity_blocks_remaining as f64,
        );
        metrics.gauge("rtao_prune_risk", f64::from(u8::from(self.prune_risk)));
    }
}

fn supported_subnet(netuid: NetUid) -> Result<(), PruneError> {
    NetUid::new(netuid.get())?;
    if netuid == NetUid::ROOT {
        return Err(PruneError::RootSubnet);
    }
    Ok(())
}
fn immunity_remaining(parameters: &PruneParameters, neuron: &PruneNeuron) -> u64 {
    u64::from(parameters.immunity_period)
        .saturating_sub(parameters.block.saturating_sub(neuron.registered_at))
}

// The typed query still verifies its actual Default storage surface. Only
// decoding of an absent raw value changes, to match pallet try_get semantics.
struct Present<F>(F);
impl<F: Fetch> Fetch for Present<F> {
    type Output = Option<F::Output>;
    fn queries(&self) -> Vec<ReadSpec> {
        self.0.queries()
    }
    fn decode(
        self,
        runtime: &RuntimeHandle,
        values: &[Option<Bytes>],
    ) -> Result<Self::Output, ClientError> {
        match values {
            [None] => Ok(None),
            [Some(_)] => self.0.decode(runtime, values).map(Some),
            _ => Err(ClientError::Protocol(
                "presence query must contain exactly one storage value".into(),
            )),
        }
    }
}
struct Many<F>(Vec<F>);
impl<F: Fetch> Fetch for Many<F> {
    type Output = Vec<F::Output>;
    fn queries(&self) -> Vec<ReadSpec> {
        self.0.iter().flat_map(F::queries).collect()
    }
    fn decode(
        self,
        runtime: &RuntimeHandle,
        values: &[Option<Bytes>],
    ) -> Result<Self::Output, ClientError> {
        let mut offset = 0usize;
        let mut decoded = Vec::with_capacity(self.0.len());
        for query in self.0 {
            let end = offset
                .checked_add(query.queries().len())
                .ok_or_else(|| ClientError::Protocol("pruning query length overflow".into()))?;
            let raw = values.get(offset..end).ok_or_else(|| {
                ClientError::Protocol("pruning query omitted a storage value".into())
            })?;
            decoded.push(query.decode(runtime, raw)?);
            offset = end;
        }
        if offset != values.len() {
            return Err(ClientError::Protocol(
                "pruning query returned extra storage values".into(),
            ));
        }
        Ok(decoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtao_client::{BlockRef, ClientConfig, Network};
    use rtao_testkit::rpc::RpcClient;
    use rtao_testkit::{FakeChain, FakeState};

    fn parameters(population: u16) -> PruneParameters {
        PruneParameters {
            netuid: NetUid(1),
            block: 100,
            population,
            max_uids: population,
            immunity_period: 10,
            min_non_immune_uids: 0,
            immune_owner_uids_limit: 0,
            owner_coldkey: AccountId([9; 32]),
            owner_hotkey: None,
            owner_uid: None,
        }
    }
    fn row(uid: u16, emission: u64, registered_at: u64) -> PruneNeuron {
        PruneNeuron {
            uid: Uid(uid),
            hotkey: AccountId([uid as u8; 32]),
            coldkey: None,
            registered_at,
            emission: AlphaBalance(emission),
        }
    }

    #[test]
    fn expiry_boundary_and_immune_fallback_match_pallet() -> Result<(), PruneError> {
        let p = parameters(2);
        let rows = [row(0, 100, 90), row(1, 0, 91)];
        let risk = PruneRisk::evaluate(&p, &rows, rows[0].hotkey)?;
        assert_eq!(risk.immunity_blocks_remaining, 0);
        assert_eq!(risk.candidate_uid, Some(Uid(0)));
        assert!(!risk.candidate_is_immune);
        assert!(risk.prune_risk);
        let rows = [row(0, 100, 91), row(1, 0, 92)];
        let risk = PruneRisk::evaluate(&p, &rows, rows[1].hotkey)?;
        assert_eq!(risk.immunity_blocks_remaining, 2);
        assert!(risk.candidate_is_immune);
        assert!(
            risk.prune_risk,
            "immunity does not prohibit the fallback replacement"
        );
        Ok(())
    }

    #[test]
    fn free_uid_floor_is_strict_and_can_prevent_every_candidate() -> Result<(), PruneError> {
        let mut p = parameters(3);
        let rows = [row(0, 100, 0), row(1, 0, 99), row(2, 1, 99)];
        p.min_non_immune_uids = 1;
        let risk = PruneRisk::evaluate(&p, &rows, rows[1].hotkey)?;
        assert_eq!(
            risk.candidate_uid,
            Some(Uid(1)),
            "one free UID equals the floor"
        );
        assert!(risk.prune_risk);
        p.min_non_immune_uids = 0;
        assert_eq!(
            PruneRisk::evaluate(&p, &rows, rows[0].hotkey)?.candidate_uid,
            Some(Uid(0))
        );
        p.min_non_immune_uids = 3;
        let old = [row(0, 0, 0), row(1, 0, 0), row(2, 0, 0)];
        let risk = PruneRisk::evaluate(&p, &old, old[0].hotkey)?;
        assert_eq!(risk.candidate_uid, None);
        assert!(!risk.prune_risk);
        Ok(())
    }

    #[test]
    fn integer_emission_registration_and_uid_ties_ignore_row_order() -> Result<(), PruneError> {
        let p = parameters(4);
        let rows = [
            row(3, 9_007_199_254_740_993, 0),
            row(2, 9_007_199_254_740_992, 2),
            row(1, 9_007_199_254_740_992, 1),
            row(0, 9_007_199_254_740_992, 1),
        ];
        let risk = PruneRisk::evaluate(&p, &rows, rows[3].hotkey)?;
        assert_eq!(risk.candidate_uid, Some(Uid(0)));
        assert!(risk.prune_risk);
        Ok(())
    }

    #[test]
    fn owner_protection_uses_oldest_first_and_primary_replaces_last_slot() -> Result<(), PruneError>
    {
        let mut p = parameters(5);
        p.immune_owner_uids_limit = 2;
        let mut rows = [
            row(0, 0, 20),
            row(1, 1, 10),
            row(2, 2, 10),
            row(3, 3, 30),
            row(4, 50, 0),
        ];
        for row in &mut rows[..4] {
            row.coldkey = Some(p.owner_coldkey);
        }
        // Before inserting the primary, registration ties protect UIDs1 and2.
        assert!(PruneRisk::evaluate(&p, &rows, rows[1].hotkey)?.owner_protected);
        assert!(PruneRisk::evaluate(&p, &rows, rows[2].hotkey)?.owner_protected);
        assert!(!PruneRisk::evaluate(&p, &rows, rows[0].hotkey)?.owner_protected);
        p.owner_hotkey = Some(rows[3].hotkey);
        p.owner_uid = Some(rows[3].uid);
        assert!(PruneRisk::evaluate(&p, &rows, rows[3].hotkey)?.owner_protected);
        assert!(PruneRisk::evaluate(&p, &rows, rows[1].hotkey)?.owner_protected);
        assert!(!PruneRisk::evaluate(&p, &rows, rows[2].hotkey)?.owner_protected);
        assert_eq!(
            PruneRisk::evaluate(&p, &rows, rows[0].hotkey)?.candidate_uid,
            Some(Uid(0))
        );
        Ok(())
    }

    #[test]
    fn zero_owner_limit_still_cannot_replace_primary_owner() -> Result<(), PruneError> {
        let mut p = parameters(2);
        let rows = [row(0, 0, 0), row(1, 100, 0)];
        p.owner_hotkey = Some(rows[0].hotkey);
        p.owner_uid = Some(rows[0].uid);
        let risk = PruneRisk::evaluate(&p, &rows, rows[0].hotkey)?;
        assert_eq!(risk.candidate_uid, Some(Uid(0)));
        assert!(risk.owner_protected);
        assert!(!risk.prune_risk);
        assert!(!PruneRisk::evaluate(&p, &rows, rows[1].hotkey)?.prune_risk);
        Ok(())
    }

    #[test]
    fn capacity_unregistered_empty_and_sparse_rows_report_no_false_replacement()
    -> Result<(), PruneError> {
        let mut p = parameters(3);
        let rows = [row(1, 0, 0)];
        p.max_uids = 4;
        let risk = PruneRisk::evaluate(&p, &rows, rows[0].hotkey)?;
        assert!(risk.registered);
        assert_eq!(risk.uid, Some(Uid(1)));
        assert_eq!(risk.candidate_uid, Some(Uid(1)));
        assert!(!risk.prune_risk);
        assert!(!risk.at_capacity);
        let absent = PruneRisk::evaluate(&p, &rows, AccountId([255; 32]))?;
        assert!(!absent.registered);
        assert_eq!(
            (
                absent.uid,
                absent.immunity_blocks_remaining,
                absent.prune_risk
            ),
            (None, 0, false)
        );
        let empty = PruneRisk::evaluate(&parameters(0), &[], AccountId::ZERO)?;
        assert_eq!(empty.candidate_uid, None);
        assert!(!empty.prune_risk);
        // Sparse capacity still uses the count of actual, nonimmune Keys entries.
        p.min_non_immune_uids = 1;
        p.max_uids = 3;
        assert_eq!(
            PruneRisk::evaluate(&p, &rows, rows[0].hotkey)?.candidate_uid,
            None
        );
        Ok(())
    }

    #[test]
    fn future_registration_zero_period_and_invalid_projection_are_explicit()
    -> Result<(), PruneError> {
        let mut p = parameters(1);
        let rows = [row(0, 0, u64::MAX)];
        let risk = PruneRisk::evaluate(&p, &rows, rows[0].hotkey)?;
        assert_eq!(risk.immunity_blocks_remaining, 10);
        assert!(risk.candidate_is_immune);
        p.immunity_period = 0;
        assert_eq!(
            PruneRisk::evaluate(&p, &rows, rows[0].hotkey)?.immunity_blocks_remaining,
            0
        );
        p.netuid = NetUid::ROOT;
        assert!(matches!(
            PruneRisk::evaluate(&p, &rows, rows[0].hotkey),
            Err(PruneError::RootSubnet)
        ));
        p.netuid = NetUid(1);
        assert!(matches!(
            PruneRisk::evaluate(&p, &[row(1, 0, 0)], AccountId::ZERO),
            Err(PruneError::InvalidState(_))
        ));
        assert!(matches!(
            PruneRisk::evaluate(&p, &[row(0, 0, 0), row(0, 0, 0)], AccountId::ZERO),
            Err(PruneError::InvalidState(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn observation_is_pinned_and_preserves_zero_accounts_and_missing_keys()
    -> anyhow::Result<()> {
        let netuid = NetUid(1);
        let mut state = FakeState::try_default()?;
        for (item, value) in [
            ("SubnetworkN", 4u16),
            ("MaxAllowedUids", 4),
            ("ImmunityPeriod", 0),
            ("MinNonImmuneUids", 0),
            ("ImmuneOwnerUidsLimit", 1),
        ] {
            state.seed_typed("SubtensorModule", item, (netuid,), value)?;
        }
        // SubnetOwner absent -> its authoritative coldkey default is zero.
        // SubnetOwnerHotkey absent must not manufacture zero-account protection.
        state.seed_typed("SubtensorModule", "Keys", (netuid, 0u16), AccountId::ZERO)?;
        state.seed_typed(
            "SubtensorModule",
            "Owner",
            (AccountId::ZERO,),
            AccountId::ZERO,
        )?;
        state.seed_typed(
            "SubtensorModule",
            "BlockAtRegistration",
            (netuid, 0u16),
            5u64,
        )?;
        // UID1 is sparse, and these rows have no Owner entry matching the owner.
        state.seed_typed(
            "SubtensorModule",
            "Keys",
            (netuid, 2u16),
            AccountId([2; 32]),
        )?;
        state.seed_typed(
            "SubtensorModule",
            "Keys",
            (netuid, 3u16),
            AccountId([3; 32]),
        )?;
        state.seed_typed(
            "SubtensorModule",
            "Owner",
            (AccountId([3; 32]),),
            AccountId([8; 32]),
        )?;
        state.seed_typed(
            "SubtensorModule",
            "Emission",
            (netuid,),
            vec![AlphaBalance(0), AlphaBalance(100)],
        )?;
        let chain = FakeChain::from_state(state);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
        let snapshot = client.at(BlockRef::Best).await?;
        let expected_hash =
            serde_json::Value::String(format!("0x{}", hex::encode(snapshot.block().hash.0)));
        chain.advance(1)?;
        let key = st::KEYS.at((netuid, 0u16)).queries();
        chain.mutate_state(|state| -> anyhow::Result<()> {
            state.seed_typed("SubtensorModule", "SubnetworkN", (netuid,), 8u16)?;
            let entry = state
                .runtime
                .storage_entry(key[0].surface.pallet, key[0].surface.name)
                .ok_or_else(|| anyhow::anyhow!("Keys entry"))?;
            let raw = state.runtime.storage_key(entry, &key[0].keys)?;
            state.seed_encoded(raw, None);
            Ok(())
        })?;
        let start = chain.requests().len();
        let risk = PruneRisk::observe(&snapshot, netuid, AccountId([2; 32])).await?;
        assert_eq!(risk.block, snapshot.block().number);
        assert_eq!(risk.uid, Some(Uid(2)));
        assert!(risk.registered && risk.at_capacity && risk.prune_risk);
        assert_eq!(risk.candidate_uid, Some(Uid(2)));
        assert_eq!(risk.immunity_blocks_remaining, 0);
        let owner = PruneRisk::observe(&snapshot, netuid, AccountId::ZERO).await?;
        assert_eq!(owner.uid, Some(Uid(0)));
        assert!(owner.registered && owner.owner_protected && !owner.prune_risk);
        for request in &chain.requests()[start..] {
            if request.method == "state_queryStorageAt" {
                assert_eq!(request.params[1], expected_hash);
            }
        }
        let current = PruneRisk::from_client(&client, netuid, AccountId::ZERO).await?;
        assert!(!current.registered && !current.prune_risk);
        client.close().await;
        Ok(())
    }
}
