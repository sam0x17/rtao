//! Typed setup for isolated localnet tests.
//!
//! Each scenario uses one nextest slot and derives its subnet from its own
//! registration receipt. Root calls go through the localnet's Alice lock.

use std::{error::Error, fmt, time::Duration};

use rtao_client::{
    ClientError, ExtrinsicReport, SubmitFailure, SubmitOptions, View, ViewExt, Wait,
};
use rtao_core::{codec::Value, value_ext::ValueExt};
use rtao_primitives::{AccountId, MechId, NetUid, TaoBalance};
use rtao_runtime::{
    Call, ComposeError,
    calls::{admin_utils, balances, subtensor_module},
    storage::subtensor_module as storage,
};
use rtao_signer::{KeypairSigner, Signer};

use crate::localnet::{Localnet, LocalnetError, LocalnetProfile, LocalnetSlot};

/// Failure while configuring or submitting an isolated scenario.
#[derive(Debug)]
pub enum ScenarioError {
    /// The builder requested dependent setup without its prerequisite.
    Invalid(&'static str),
    /// Slot allocation or locked root submission failed.
    Localnet(LocalnetError),
    /// A metadata or chain read failed.
    Client(ClientError),
    /// A generated call did not match the current runtime.
    Compose(ComposeError),
    /// Submission was rejected or its fate could not be resolved.
    Submit(SubmitFailure),
    /// An included setup extrinsic failed dispatch.
    Dispatch(String),
    /// Registration did not produce one unambiguous subnet identity.
    Registration(String),
}

impl fmt::Display for ScenarioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::Localnet(error) => error.fmt(f),
            Self::Client(error) => error.fmt(f),
            Self::Compose(error) => error.fmt(f),
            Self::Submit(error) => error.fmt(f),
            Self::Dispatch(message) | Self::Registration(message) => f.write_str(message),
        }
    }
}
impl Error for ScenarioError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Localnet(error) => Some(error),
            Self::Client(error) => Some(error),
            Self::Compose(error) => Some(error),
            Self::Submit(error) => Some(error),
            _ => None,
        }
    }
}
impl From<LocalnetError> for ScenarioError {
    fn from(error: LocalnetError) -> Self {
        Self::Localnet(error)
    }
}
impl From<ClientError> for ScenarioError {
    fn from(error: ClientError) -> Self {
        Self::Client(error)
    }
}
impl From<ComposeError> for ScenarioError {
    fn from(error: ComposeError) -> Self {
        Self::Compose(error)
    }
}
impl From<SubmitFailure> for ScenarioError {
    fn from(error: SubmitFailure) -> Self {
        Self::Submit(error)
    }
}

/// One funded slot and the chain objects created for its test.
#[derive(Debug)]
pub struct BuiltScenario {
    /// Independent funded coldkey and hotkey selected for this process.
    pub slot: LocalnetSlot,
    /// Subnet emitted by this scenario's registration, if requested.
    pub netuid: Option<NetUid>,
    /// Registered validator signer, if requested.
    pub validator: Option<KeypairSigner>,
    /// Registered miner signer, if requested.
    pub miner: Option<KeypairSigner>,
    /// Included reports for all setup submissions, in order.
    pub reports: Vec<ExtrinsicReport>,
}

/// Fluent localnet setup using generated call builders and direct submissions.
pub struct Scenario<'a> {
    localnet: &'a Localnet,
    owner: bool,
    subnet: bool,
    start: bool,
    validator: bool,
    miner: bool,
    stake: Option<TaoBalance>,
    commit_reveal: Option<bool>,
    mechanisms: Option<MechId>,
    weights_version: Option<u64>,
    commit_reveal_version: Option<u16>,
    tempo: Option<u16>,
    slot: Option<LocalnetSlot>,
}

impl<'a> Scenario<'a> {
    /// Begin setup on a bootstrapped localnet, allocating no accounts yet.
    pub fn new(localnet: &'a Localnet) -> Self {
        Self {
            localnet,
            owner: false,
            subnet: false,
            start: false,
            validator: false,
            miner: false,
            stake: None,
            commit_reveal: None,
            mechanisms: None,
            weights_version: None,
            commit_reveal_version: None,
            tempo: None,
            slot: None,
        }
    }
    /// Use the process's funded slot owner.
    pub fn owner(mut self) -> Self {
        self.owner = true;
        self
    }
    /// Register a fresh subnet with the slot hotkey.
    pub fn subnet(mut self) -> Self {
        self.subnet = true;
        self
    }
    /// Start the subnet after registration.
    pub fn start(mut self) -> Self {
        self.start = true;
        self
    }
    /// Register the slot hotkey as a validator when it is not already registered.
    pub fn validator(mut self) -> Self {
        self.validator = true;
        self
    }
    /// Register a separate miner derived below the slot's development URI.
    pub fn miner(mut self) -> Self {
        self.miner = true;
        self
    }
    /// Stake an exact native rao amount on the validator.
    pub fn stake(mut self, amount: TaoBalance) -> Self {
        self.stake = Some(amount);
        self
    }
    /// Set the subnet's commit-reveal flag through the owner.
    pub fn commit_reveal(mut self, enabled: bool) -> Self {
        self.commit_reveal = Some(enabled);
        self
    }
    /// Set the subnet's number of mechanisms through the owner.
    pub fn mechanisms(mut self, count: MechId) -> Self {
        self.mechanisms = Some(count);
        self
    }
    /// Set the subnet's accepted weights version through the owner.
    pub fn weights_version(mut self, version: u64) -> Self {
        self.weights_version = Some(version);
        self
    }
    /// Set the global CR version through locked sudo; use only the global lane.
    pub fn commit_reveal_version(mut self, version: u16) -> Self {
        self.commit_reveal_version = Some(version);
        self
    }
    /// Set an explicit subnet tempo through locked sudo.
    pub fn tempo(mut self, tempo: u16) -> Self {
        self.tempo = Some(tempo);
        self
    }
    /// Choose an explicitly allocated slot, useful for in-process fake tests.
    pub fn with_slot(mut self, slot: LocalnetSlot) -> Self {
        self.slot = Some(slot);
        self
    }

    fn validate(&self) -> Result<(), ScenarioError> {
        if !self.owner {
            return Err(ScenarioError::Invalid("Scenario requires owner()"));
        }
        if !self.subnet
            && (self.start
                || self.validator
                || self.miner
                || self.commit_reveal.is_some()
                || self.mechanisms.is_some()
                || self.weights_version.is_some()
                || self.tempo.is_some())
        {
            return Err(ScenarioError::Invalid("subnet setup requires subnet()"));
        }
        if self.stake.is_some() && !self.validator {
            return Err(ScenarioError::Invalid("stake requires validator()"));
        }
        if self
            .mechanisms
            .is_some_and(|count| count.0 == 0 || count.0 > MechId::LIMIT)
        {
            return Err(ScenarioError::Invalid("mechanism count must be in 1..=16"));
        }
        if self.tempo == Some(0) {
            return Err(ScenarioError::Invalid("tempo must be nonzero"));
        }
        Ok(())
    }

    /// Submit requested setup and reject every unsuccessful inclusion.
    pub async fn build(self) -> Result<BuiltScenario, ScenarioError> {
        self.validate()?;
        let slot = match &self.slot {
            Some(slot) => slot.clone(),
            None => self.localnet.slot()?,
        };
        let mut result = BuiltScenario {
            slot,
            netuid: None,
            validator: None,
            miner: None,
            reports: Vec::new(),
        };
        if let Some(version) = self.commit_reveal_version {
            result.reports.push(
                self.root(admin_utils::sudo_set_commit_reveal_version(version))
                    .await?,
            );
        }
        if !self.subnet {
            return Ok(result);
        }
        let registration = self
            .owner_call(
                subtensor_module::register_network(result.slot.hotkey.account()),
                &result.slot.owner,
            )
            .await?;
        let netuid = registration_netuid(&registration)?;
        result.netuid = Some(netuid);
        result.reports.push(registration);
        let recorded_owner = self
            .localnet
            .client()
            .storage(&storage::SUBNET_OWNER, (netuid,))
            .await?;
        if recorded_owner.get() != Some(&result.slot.owner.account()) {
            return Err(ScenarioError::Registration(
                "NetworkAdded subnet has a different owner".into(),
            ));
        }
        if self.start {
            result.reports.push(
                self.owner_call(subtensor_module::start_call(netuid), &result.slot.owner)
                    .await?,
            );
        }
        result.reports.push(
            self.root(admin_utils::sudo_set_weights_set_rate_limit(netuid, 0))
                .await?,
        );
        for call in [
            admin_utils::sudo_set_serving_rate_limit(netuid, 0),
            admin_utils::sudo_set_min_allowed_weights(netuid, 0),
        ] {
            result
                .reports
                .push(self.owner_call(call, &result.slot.owner).await?);
        }
        let tempo = match (self.tempo, self.localnet.profile()) {
            (Some(tempo), _) => Some(tempo),
            (None, LocalnetProfile::Slow) => Some(10),
            (None, LocalnetProfile::Fast) => {
                let enabled = match self.commit_reveal {
                    Some(enabled) => enabled,
                    None => {
                        self.localnet
                            .client()
                            .storage(&storage::COMMIT_REVEAL_WEIGHTS_ENABLED, (netuid,))
                            .await?
                    }
                };
                enabled.then_some(48)
            }
        };
        if let Some(tempo) = tempo {
            result.reports.push(
                self.root(admin_utils::sudo_set_tempo(netuid, tempo))
                    .await?,
            );
        }
        if let Some(enabled) = self.commit_reveal {
            result.reports.push(
                self.owner_call(
                    admin_utils::sudo_set_commit_reveal_weights_enabled(netuid, enabled),
                    &result.slot.owner,
                )
                .await?,
            );
        }
        if let Some(count) = self.mechanisms {
            result.reports.push(
                self.owner_call(
                    admin_utils::sudo_set_mechanism_count(netuid, count),
                    &result.slot.owner,
                )
                .await?,
            );
        }
        if let Some(version) = self.weights_version {
            result.reports.push(
                self.owner_call(
                    admin_utils::sudo_set_weights_version_key(netuid, version),
                    &result.slot.owner,
                )
                .await?,
            );
        }
        if self.validator {
            self.register_if_missing(netuid, result.slot.hotkey.account(), &mut result)
                .await?;
            result.validator = Some(result.slot.hotkey.clone());
        }
        if self.miner {
            let miner = result.slot.miner.clone();
            self.register_if_missing(netuid, miner.account(), &mut result)
                .await?;
            result.reports.push(
                self.owner_call(
                    balances::transfer_keep_alive(
                        Value::Bytes(miner.account().0.to_vec()).into(),
                        TaoBalance(1_000_000_000),
                    ),
                    &result.slot.owner,
                )
                .await?,
            );
            result.miner = Some(miner);
        }
        if let Some(amount) = self.stake {
            result.reports.push(
                self.owner_call(
                    subtensor_module::add_stake(result.slot.hotkey.account(), netuid, amount),
                    &result.slot.owner,
                )
                .await?,
            );
        }
        Ok(result)
    }

    async fn register_if_missing(
        &self,
        netuid: NetUid,
        hotkey: AccountId,
        result: &mut BuiltScenario,
    ) -> Result<(), ScenarioError> {
        if self
            .localnet
            .client()
            .storage(&storage::UIDS, (netuid, hotkey))
            .await?
            .is_none()
        {
            result.reports.push(
                self.owner_call(
                    subtensor_module::burned_register(netuid, hotkey),
                    &result.slot.owner,
                )
                .await?,
            );
        }
        Ok(())
    }

    async fn root(&self, call: Call) -> Result<ExtrinsicReport, ScenarioError> {
        let runtime = self.localnet.client().runtime().await?;
        successful(self.localnet.sudo(call.compose(&runtime)?).await?)
    }

    async fn owner_call(
        &self,
        call: Call,
        signer: &dyn Signer,
    ) -> Result<ExtrinsicReport, ScenarioError> {
        let runtime = self.localnet.client().runtime().await?;
        successful(
            self.localnet
                .client()
                .submit(
                    call.compose(&runtime)?,
                    signer,
                    &SubmitOptions {
                        wait: Wait::Inclusion,
                        deadline: Some(Duration::from_secs(180)),
                        ..Default::default()
                    },
                )
                .await?,
        )
    }
}

fn successful(report: ExtrinsicReport) -> Result<ExtrinsicReport, ScenarioError> {
    if report.success {
        Ok(report)
    } else {
        Err(ScenarioError::Dispatch(report.message))
    }
}

fn registration_netuid(report: &ExtrinsicReport) -> Result<NetUid, ScenarioError> {
    let mut added = report
        .events
        .iter()
        .filter(|event| event.matches("SubtensorModule", "NetworkAdded"));
    let event = added.next().ok_or_else(|| {
        ScenarioError::Registration(
            "register_network produced no NetworkAdded; queued setup needs a registration follower"
                .into(),
        )
    })?;
    if added.next().is_some() {
        return Err(ScenarioError::Registration(
            "registration produced multiple NetworkAdded events".into(),
        ));
    }
    let first = match &event.attributes {
        Value::List(values) | Value::Tuple(values) => values.first(),
        value => value.field("netuid"),
    };
    let netuid = first
        .and_then(ValueExt::as_u128)
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| {
            ScenarioError::Registration("NetworkAdded has no u16 subnet identity".into())
        })?;
    NetUid::new(netuid).map_err(|_| {
        ScenarioError::Registration("NetworkAdded subnet identity is out of range".into())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FakeChain, FakeState, SubmitScript};
    use rtao_client::{Client, ClientConfig, Network};
    use serde_json::{Value as Json, json};
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };
    use subxt_rpcs::RpcClient;

    type TestResult = Result<(), Box<dyn Error>>;
    static ROOT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestRoot(PathBuf);
    impl TestRoot {
        fn new() -> Result<Self, Box<dyn Error>> {
            let base = std::env::var_os("RTAO_TEMP_ROOT")
                .map(PathBuf::from)
                .ok_or("RTAO_TEMP_ROOT is required for scenario fixtures")?;
            let path = base.join(format!(
                "scenario-fake-{}-{}",
                std::process::id(),
                ROOT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path)?;
            Ok(Self(path))
        }
    }
    impl Drop for TestRoot {
        fn drop(&mut self) {
            // The fake has no process, launchd unit, keyfiles or disk cache.
            // Its only on-disk dependency is an unlocked Alice advisory file.
            // Refuse cleanup if a symlink or an unexpected entry appeared.
            let Ok(root) = fs::symlink_metadata(&self.0) else {
                return;
            };
            if !root.is_dir() || root.file_type().is_symlink() {
                return;
            }
            let Ok(entries) = fs::read_dir(&self.0) else {
                return;
            };
            for entry in entries.flatten() {
                if entry.file_name() != "e2e" {
                    return;
                }
                let Ok(meta) = fs::symlink_metadata(entry.path()) else {
                    return;
                };
                if !meta.is_dir() || meta.file_type().is_symlink() {
                    return;
                }
                let Ok(locks) = fs::read_dir(entry.path()) else {
                    return;
                };
                for lock in locks.flatten() {
                    let Ok(meta) = fs::symlink_metadata(lock.path()) else {
                        return;
                    };
                    if !meta.is_file()
                        || meta.file_type().is_symlink()
                        || !lock.file_name().to_string_lossy().ends_with(".alice.lock")
                    {
                        return;
                    }
                }
            }
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    async fn fixture(state: FakeState) -> Result<(Localnet, FakeChain, TestRoot), Box<dyn Error>> {
        let fake = FakeChain::from_state(state);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(fake.clone()), config).await?;
        let root = TestRoot::new()?;
        Ok((
            Localnet::from_client(client, LocalnetProfile::Fast, root.0.clone())?,
            fake,
            root,
        ))
    }

    fn success() -> Json {
        json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System",
            "event_id":"ExtrinsicSuccess","attributes":{"dispatch_info":{
            "weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]})
    }
    fn added(netuid: u16) -> Json {
        json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"SubtensorModule",
            "event_id":"NetworkAdded","attributes":[netuid,0],"topics":[]})
    }
    fn registered_state(slot: &LocalnetSlot, netuid: u16) -> Result<FakeState, Box<dyn Error>> {
        let mut state = FakeState::try_default()?;
        state.seed_typed(
            "SubtensorModule",
            "SubnetOwner",
            (NetUid(netuid),),
            slot.owner.account(),
        )?;
        state.seed_typed(
            "SubtensorModule",
            "Uids",
            (NetUid(netuid), slot.hotkey.account()),
            0_u16,
        )?;
        // The Python fake's permissive Uids default is zero. A real optional
        // map miss must be explicit here so miner registration is exercised.
        let uids = state
            .runtime
            .storage_entry("SubtensorModule", "Uids")
            .ok_or("Uids storage missing")?;
        let missing_miner = state.runtime.storage_key(
            uids,
            &[
                Value::Int(i128::from(netuid)),
                Value::Bytes(slot.miner.account().0.to_vec()),
            ],
        )?;
        state.seed_encoded(missing_miner, None);
        state.queue_submit(SubmitScript::InBlockThenFinalized {
            events: json!([added(netuid), success()]),
        });
        Ok(state)
    }

    #[tokio::test]
    async fn generated_setup_is_signed_by_slot_owner_and_locked_alice() -> TestResult {
        let slot = LocalnetSlot::new(12)?;
        let state = registered_state(&slot, 7)?;
        let (ln, fake, _root) = fixture(state).await?;
        let setup = Scenario::new(&ln)
            .with_slot(slot.clone())
            .owner()
            .subnet()
            .start()
            .validator()
            .miner()
            .stake(TaoBalance(1_000_000_000_000))
            .commit_reveal(false)
            .mechanisms(MechId(2))
            .weights_version(3)
            .build()
            .await?;
        assert_eq!(setup.netuid, Some(NetUid(7)));
        assert_eq!(
            setup.validator.as_ref().map(Signer::account),
            Some(slot.hotkey.account())
        );
        assert_eq!(
            setup.miner.as_ref().map(Signer::account),
            Some(slot.miner.account())
        );
        assert_eq!(setup.reports.len(), 11);
        let runtime = ln.client().runtime().await?;
        let alice = KeypairSigner::from_uri("//Alice", rtao_primitives::CryptoType::Sr25519)?;
        let submissions = fake
            .state()
            .lock()
            .map_err(|_| "poisoned fake state")?
            .submissions
            .clone();
        assert_eq!(submissions.len(), setup.reports.len());
        for (index, bytes) in submissions.iter().enumerate() {
            let decoded = runtime.core().decode_extrinsic(bytes, true)?;
            let address = decoded
                .field("address")
                .ok_or("signed setup lacks address")?;
            let address = address.variant("Id").unwrap_or(address);
            let expected = if index == 2 {
                alice.account()
            } else {
                slot.owner.account()
            };
            if let Some(address) = address.as_str() {
                assert_eq!(address.parse::<AccountId>()?, expected);
            } else {
                assert_eq!(
                    address.value_bytes().ok_or("unrecognized setup address")?,
                    expected.0
                );
            }
            let call = decoded.field("call").ok_or("signed setup lacks call")?;
            let expected_call = [
                "register_network",
                "start_call",
                "sudo",
                "sudo_set_serving_rate_limit",
                "sudo_set_min_allowed_weights",
                "sudo_set_commit_reveal_weights_enabled",
                "sudo_set_mechanism_count",
                "sudo_set_weights_version_key",
                "burned_register",
                "transfer_keep_alive",
                "add_stake",
            ][index];
            assert_eq!(
                call.field("call_function").and_then(Value::as_str),
                Some(expected_call)
            );
        }
        assert_eq!(
            fake.requests()
                .iter()
                .filter(|request| request.method == "account_nextIndex"
                    && request.params[0].as_str() == Some(&alice.account().to_string()))
                .count(),
            1
        );
        ln.client().close().await;
        Ok(())
    }

    #[tokio::test]
    async fn invalid_setup_never_reads_or_submits() -> TestResult {
        let (ln, fake, _root) = fixture(FakeState::try_default()?).await?;
        let before = fake.requests().len();
        let result = Scenario::new(&ln)
            .owner()
            .validator()
            .stake(TaoBalance(1))
            .build()
            .await;
        assert!(matches!(
            result,
            Err(ScenarioError::Invalid("subnet setup requires subnet()"))
        ));
        assert_eq!(fake.requests().len(), before);
        assert!(
            fake.state()
                .lock()
                .map_err(|_| "poisoned fake state")?
                .submissions
                .is_empty()
        );
        ln.client().close().await;
        Ok(())
    }

    #[tokio::test]
    async fn unrelated_subnet_cannot_replace_registration_receipt() -> TestResult {
        let slot = LocalnetSlot::new(1)?;
        let mut state = FakeState::try_default()?;
        state.seed_typed(
            "SubtensorModule",
            "SubnetOwner",
            (NetUid(999),),
            slot.owner.account(),
        )?;
        let (ln, fake, _root) = fixture(state).await?;
        let result = Scenario::new(&ln)
            .with_slot(slot)
            .owner()
            .subnet()
            .build()
            .await;
        assert!(
            matches!(result, Err(ScenarioError::Registration(message)) if message.contains("no NetworkAdded"))
        );
        assert_eq!(
            fake.state()
                .lock()
                .map_err(|_| "poisoned fake state")?
                .submissions
                .len(),
            1
        );
        ln.client().close().await;
        Ok(())
    }

    #[tokio::test]
    async fn ambiguous_or_foreign_registration_stops_before_subnet_setup() -> TestResult {
        for ambiguous in [false, true] {
            let slot = LocalnetSlot::new(2)?;
            let mut state = registered_state(&slot, 7)?;
            if ambiguous {
                let mut replacement = FakeState::try_default()?;
                replacement.queue_submit(SubmitScript::InBlockThenFinalized {
                    events: json!([added(7), added(8), success()]),
                });
                state = replacement;
            } else {
                state.seed_typed(
                    "SubtensorModule",
                    "SubnetOwner",
                    (NetUid(7),),
                    AccountId([8; 32]),
                )?;
            }
            let (ln, fake, _root) = fixture(state).await?;
            let result = Scenario::new(&ln)
                .with_slot(slot)
                .owner()
                .subnet()
                .start()
                .build()
                .await;
            assert!(matches!(result, Err(ScenarioError::Registration(_))));
            assert_eq!(
                fake.state()
                    .lock()
                    .map_err(|_| "poisoned fake state")?
                    .submissions
                    .len(),
                1
            );
            ln.client().close().await;
        }
        Ok(())
    }
}
