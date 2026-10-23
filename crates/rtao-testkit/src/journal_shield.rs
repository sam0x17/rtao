//! Real local sockets and files, synthetic runtime execution/body/events.
//! No native decryption, finality verification, live funding or deployed code
//! qualification is implied. The fixed synthetic genesis gates public dispatch.
use crate::{FakeNode, FakeState, Reply};
use rtao_client::{
    Client, ClientConfig, Network,
    journal::{
        Incompatibility, JournalBinding, OperationIntent, OperationJournal, OperationRecord,
        PairPhase, ShieldProtocolGuard,
    },
};
use rtao_primitives::{AccountId, CryptoType, H256};
use rtao_signer::{
    BoxFuture, Coldkey, DynKeySource, Hotkey, KeySource, KeyView, KeypairSigner, MultiSignature,
    Signer, SignerError, SignerRole, SigningRequest,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

/// Fallible synthetic fixture operations; diagnostics never include signer data.
pub type LabResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Synthetic handling of a published encrypted carrier.
#[derive(Clone, Copy)]
pub enum Inner {
    /// Include the retained exact inner and report this dispatch status.
    Include(bool),
    /// Leave the inner unresolved, without advancing finality through expiry.
    Pending,
    /// Finalize the whole validity window without the inner.
    Absent,
    /// Include the exact inner but lose its carrier admission acknowledgment.
    LostAcknowledgment,
}
/// One-shot signer synchronization point; contains no signing payload.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// Before interactive preparation.
    Prepare,
    /// Before the signed inner is produced.
    Inner,
    /// Before the carrier signature is produced.
    Carrier,
}
struct Gate {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
/// A controlled clock-free signer pause for cancellation/upgrade tests.
pub struct SigningPause(Arc<Gate>);
impl SigningPause {
    /// Wait until the signer actually reaches this stage.
    pub async fn entered(&self) {
        self.0.entered.notified().await;
    }
    /// Permit that stage to continue once.
    pub fn release(&self) {
        self.0.release.notify_one();
    }
}
/// Fixed disposable test signer. Counters contain no keys or signing bytes.
pub struct LabSigner {
    key: KeypairSigner,
    prepares: AtomicU64,
    signatures: AtomicU64,
    gates: Mutex<BTreeMap<Stage, Arc<Gate>>>,
}
impl LabSigner {
    /// Use the public fixture seed, never an operator wallet or real funds.
    pub fn new(crypto: CryptoType) -> LabResult<Self> {
        Ok(Self {
            key: KeypairSigner::from_seed(&[7; 32], crypto)?,
            prepares: AtomicU64::new(0),
            signatures: AtomicU64::new(0),
            gates: Mutex::new(BTreeMap::new()),
        })
    }
    /// Number of interactive-preparation entries.
    pub fn preparations(&self) -> u64 {
        self.prepares.load(Ordering::SeqCst)
    }
    /// Number of cryptographic signatures actually produced.
    pub fn signatures(&self) -> u64 {
        self.signatures.load(Ordering::SeqCst)
    }
    /// Pause the next matching signer stage, without changing any SDK timeout.
    pub fn pause(&self, stage: Stage) -> SigningPause {
        let gate = Arc::new(Gate {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        lock(&self.gates).insert(stage, gate.clone());
        SigningPause(gate)
    }
    async fn checkpoint(&self, stage: Stage) {
        let gate = lock(&self.gates).remove(&stage);
        if let Some(gate) = gate {
            gate.entered.notify_one();
            gate.release.notified().await;
        }
    }
}
impl Signer for LabSigner {
    fn account(&self) -> AccountId {
        self.key.account()
    }
    fn crypto_type(&self) -> CryptoType {
        self.key.crypto_type()
    }
    fn prepare_shielded(&self) -> BoxFuture<'_, Result<(), SignerError>> {
        Box::pin(async move {
            self.prepares.fetch_add(1, Ordering::SeqCst);
            self.checkpoint(Stage::Prepare).await;
            Ok(())
        })
    }
    fn sign<'a>(
        &'a self,
        request: &'a SigningRequest<'a>,
    ) -> BoxFuture<'a, Result<MultiSignature, SignerError>> {
        Box::pin(async move {
            match request.purpose {
                rtao_signer::SignPurpose::ShieldInner => self.checkpoint(Stage::Inner).await,
                rtao_signer::SignPurpose::ShieldCarrier => self.checkpoint(Stage::Carrier).await,
                _ => {}
            }
            let signature = self.key.sign(request).await?;
            self.signatures.fetch_add(1, Ordering::SeqCst);
            Ok(signature)
        })
    }
}
/// Public identity plus counted resolution of the fixed synthetic signer.
pub struct LabKeys {
    signer: Arc<LabSigner>,
    unlocks: AtomicU64,
}
impl LabKeys {
    /// Share the synthetic hardware/key source across restarted SDK callers.
    pub fn new(signer: Arc<LabSigner>) -> Self {
        Self {
            signer,
            unlocks: AtomicU64::new(0),
        }
    }
    /// Number of private-signer resolutions.
    pub fn unlocks(&self) -> u64 {
        self.unlocks.load(Ordering::SeqCst)
    }
}
impl KeyView for LabKeys {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.signer.account())
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok(self.signer.account().0)
    }
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.signer.account())
    }
}
macro_rules! key_source {
    ($role:ty) => {
        impl KeySource<$role> for LabKeys {
            fn address(&self) -> Result<AccountId, SignerError> {
                Ok(self.signer.account())
            }
            async fn signer(&self) -> Result<Arc<dyn Signer>, SignerError> {
                self.unlocks.fetch_add(1, Ordering::SeqCst);
                Ok(self.signer.clone())
            }
        }
    };
}
key_source!(Hotkey);
key_source!(Coldkey);
impl DynKeySource for LabKeys {
    fn signer_for<'a>(
        &'a self,
        _role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move {
            self.unlocks.fetch_add(1, Ordering::SeqCst);
            Ok(self.signer.clone() as Arc<dyn Signer>)
        })
    }
}
/// Explicit test-only qualification over synthetic code identities.
pub struct SyntheticProtocol {
    codes: Mutex<BTreeSet<H256>>,
}
impl Default for SyntheticProtocol {
    fn default() -> Self {
        Self {
            codes: Mutex::new(BTreeSet::from([H256([42; 32])])),
        }
    }
}
impl SyntheticProtocol {
    /// Accept an additional synthetic code identity after a controlled upgrade.
    pub fn allow(&self, code: H256) {
        lock(&self.codes).insert(code);
    }
}
impl ShieldProtocolGuard for SyntheticProtocol {
    fn profile(&self) -> &str {
        "rtao-testkit-signed-inner-v1"
    }
    fn check(&self, genesis: H256, code: H256) -> Result<(), Incompatibility> {
        if genesis == Client::SYNTHETIC_JOURNAL_GENESIS && lock(&self.codes).contains(&code) {
            return Ok(());
        }
        Err(Incompatibility {
            code: "synthetic-code-unqualified".into(),
            descriptor: None,
            detail: "install the independently qualified synthetic adapter".into(),
            extensions: Default::default(),
        })
    }
}
type InclusionHook = Box<dyn FnMut(&mut FakeState, &OperationRecord, u64) + Send>;
struct Control {
    journal: Weak<OperationJournal>,
    scripts: VecDeque<Inner>,
    bodies: BTreeMap<String, (Vec<u8>, bool)>,
    hook: Option<InclusionHook>,
    author_hook: Option<InclusionHook>,
    code: H256,
    historical_codes: BTreeMap<String, H256>,
    runtime_floor: u64,
    history_unavailable: bool,
    external_journal: Option<PathBuf>,
    author_entered: Arc<tokio::sync::Notify>,
}
/// Native Session/journal harness for public Client/Executor/scheduler tests.
/// Bodies/events and dispatch/queue effects are synthetic, not native Wasm.
pub struct JournalLab {
    /// Actual local WebSocket peer; no external endpoint is contacted.
    pub node: FakeNode,
    /// Client using explicit native author custody.
    pub client: Client,
    /// Fixed test key, never printed or persisted in the operation journal.
    pub signer: Arc<LabSigner>,
    /// Installed synthetic runtime-code qualification.
    pub protocol: Arc<SyntheticProtocol>,
    journal: Option<Arc<OperationJournal>>,
    path: PathBuf,
    config: ClientConfig,
    control: Arc<Mutex<Control>>,
}
impl JournalLab {
    /// Create a fresh native journal and fixed synthetic-genesis local peer.
    pub async fn new(
        state: FakeState,
        path: impl AsRef<Path>,
        crypto: CryptoType,
    ) -> LabResult<Self> {
        Self::construct(state, path, crypto, false).await
    }
    /// Also start the actual client head tracker for scheduler/metagraph tests.
    /// The controlled peer remains the sole source and runtime execution is synthetic.
    pub async fn with_head_tracking(
        state: FakeState,
        path: impl AsRef<Path>,
        crypto: CryptoType,
    ) -> LabResult<Self> {
        Self::construct(state, path, crypto, true).await
    }
    async fn construct(
        mut state: FakeState,
        path: impl AsRef<Path>,
        crypto: CryptoType,
        track_heads: bool,
    ) -> LabResult<Self> {
        let path = path.as_ref().to_owned();
        if path.exists() {
            return Err("existing journal retained; choose a fresh disposable path".into());
        }
        state.genesis_hash = format!("0x{}", hex::encode(Client::SYNTHETIC_JOURNAL_GENESIS.0));
        state.seed_constant("Aura", "SlotDuration", json!(1))?;
        state.seed_typed("MevShield", "NextKey", (), vec![0u8; 1184])?;
        state.set_result(
            "state_getStorageHash",
            json!(format!("0x{}", hex::encode(H256([42; 32]).0))),
        );
        let node = FakeNode::with_state(Arc::new(Mutex::new(state))).await?;
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.track_heads = track_heads;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let session =
            rtao_rpc::Session::connect(rtao_rpc::SessionConfig::new(url::Url::parse(node.url())?))
                .await?;
        let client = Client::from_session(session, config.clone()).await?;
        client.enable_synthetic_journal_dispatch().await?;
        let journal = OperationJournal::open(&path)?;
        let control = Arc::new(Mutex::new(Control {
            journal: Arc::downgrade(&journal),
            scripts: VecDeque::new(),
            bodies: BTreeMap::new(),
            hook: None,
            author_hook: None,
            code: H256([42; 32]),
            historical_codes: BTreeMap::new(),
            runtime_floor: 0,
            history_unavailable: false,
            external_journal: None,
            author_entered: Arc::new(tokio::sync::Notify::new()),
        }));
        let observing = control.clone();
        node.mutate_state(|state| {
            state.set_handler("state_getStorageHash", move |_state, request| {
                let control = lock(&observing);
                let code = request
                    .params
                    .get(1)
                    .and_then(Value::as_str)
                    .and_then(|hash| control.historical_codes.get(hash))
                    .unwrap_or(&control.code);
                Reply::result(json!(format!("0x{}", hex::encode(code.0))))
            })
        });
        let observing = control.clone();
        node.mutate_state(|state| {
            state.set_handler("author_submitAndWatchExtrinsic", move |state, request| {
                let mut control = lock(&observing);
                let image = if let Some(journal) = control.journal.upgrade() {
                    match journal.snapshot() {
                        Ok(image) => image,
                        Err(_) => return Reply::silent(),
                    }
                } else if let Some(path) = &control.external_journal {
                    // The child owns the actual file lock. Inspect one bounded
                    // atomic publication; never open a second writing backend.
                    use std::io::Read;
                    let bytes = (|| -> std::io::Result<Vec<u8>> {
                        let mut bytes = Vec::new();
                        std::fs::File::open(path)?
                            .take(rtao_client::journal::MAX_JOURNAL_BYTES + 1)
                            .read_to_end(&mut bytes)?;
                        Ok(bytes)
                    })();
                    let Some(image) = bytes
                        .ok()
                        .filter(|b| b.len() as u64 <= rtao_client::journal::MAX_JOURNAL_BYTES)
                        .and_then(|b| {
                            serde_json::from_slice::<rtao_client::journal::JournalImage>(&b).ok()
                        })
                        .filter(|i| i.validate().is_ok())
                    else {
                        return Reply::silent();
                    };
                    image
                } else {
                    return Reply::silent();
                };
                let selected = image.operations.values().find_map(|record| {
                    record.shield_pairs.iter().find_map(|pair| {
                        let carrier = record.attempt(pair.carrier?).ok()?;
                        (request.params[0] == format!("0x{}", hex::encode(&carrier.bytes)))
                            .then_some((record, pair))
                    })
                });
                let Some((record, pair)) = selected else {
                    return Reply::silent();
                };
                assert!(
                    matches!(pair.phase, PairPhase::DispatchAuthorized { .. }),
                    "author request preceded durable authorization"
                );
                let inner = match record.attempt(pair.inner) {
                    Ok(inner) => inner,
                    Err(_) => return Reply::silent(),
                };
                let script = control.scripts.pop_front().unwrap_or(Inner::Include(true));
                let reply = state.dispatch(request);
                let number = state.block;
                if let Some(hook) = &mut control.author_hook {
                    hook(state, record, number);
                }
                match script {
                    Inner::Include(_) | Inner::LostAcknowledgment => {
                        let success = if let Inner::Include(success) = script {
                            success
                        } else {
                            true
                        };
                        if let Some(hash) = state.block_hash(number) {
                            control.bodies.insert(hash, (inner.bytes.clone(), success));
                        }
                        state.set_nonce(
                            record.intent.account.to_string(),
                            inner.nonce.saturating_add(1),
                        );
                        if let Some(hook) = &mut control.hook {
                            hook(state, record, number);
                        }
                        let _ = state.set_finalized(number);
                    }
                    Inner::Absent => {
                        if let Some(expiry) = inner.expiry {
                            state.advance(expiry.saturating_add(1).saturating_sub(state.block));
                        }
                    }
                    Inner::Pending => {
                        let _ = state.set_finalized(number);
                    }
                }
                if matches!(script, Inner::LostAcknowledgment) {
                    control.history_unavailable = true;
                    control.author_entered.notify_one();
                    Reply::silent()
                } else {
                    reply
                }
            })
        });
        let observing = control.clone();
        node.mutate_state(|state|state.set_handler("chain_getBlock",move |state,request| {
            if lock(&observing).history_unavailable {return Reply::error(-32001,"synthetic finalized body unavailable",None);}
            let mut reply=state.dispatch(request);
            let inclusion=request.params[0].as_str().and_then(|hash|lock(&observing).bodies.get(hash).cloned());
            if let (Some((inner,success)),Some(Ok(raw)))=(inclusion,&mut reply.outcome)
                && let Some(list)=raw.pointer_mut("/block/extrinsics").and_then(Value::as_array_mut) {
                let index=list.len();list.push(json!(format!("0x{}",hex::encode(inner))));
                let events=(0..=index).map(|i|if i==index && !success {
                    json!({"phase":"ApplyExtrinsic","extrinsic_idx":i,"module_id":"System","event_id":"ExtrinsicFailed","attributes":{"dispatch_error":{"BadOrigin":null},"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]})
                }else{json!({"phase":"ApplyExtrinsic","extrinsic_idx":i,"module_id":"System","event_id":"ExtrinsicSuccess","attributes":{"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]})}).collect::<Vec<_>>();
                if let Some(hash)=request.params[0].as_str() {let _=state.seed_events(hash,json!(events));}
            }
            reply
        }));
        Ok(Self {
            node,
            client,
            signer: Arc::new(LabSigner::new(crypto)?),
            protocol: Arc::new(SyntheticProtocol::default()),
            journal: Some(journal),
            path,
            config,
            control,
        })
    }
    /// Borrow the current actual native journal.
    pub fn journal(&self) -> LabResult<&Arc<OperationJournal>> {
        self.journal
            .as_ref()
            .ok_or_else(|| "journal restart incomplete".into())
    }
    /// Bind a retained logical operation and reinstall its trusted test adapter.
    pub fn bind(&self, id: &str) -> LabResult<JournalBinding> {
        Ok(self
            .journal()?
            .bind(id)?
            .with_shield_protocol(self.protocol.clone()))
    }
    /// Persist new public synthetic intent/material before any author request.
    pub fn begin(&self, id: &str, effects: Value, material: Value) -> LabResult<JournalBinding> {
        Ok(self
            .journal()?
            .begin(OperationIntent {
                id: id.into(),
                correlation_id: Some(format!("application:{id}")),
                genesis: self.client.genesis(),
                account: self.signer.account(),
                scope: "synthetic-public-lifecycle".into(),
                effects,
                obligation: false,
                semantic_policy: None,
                extensions: Default::default(),
            })?
            .with_shield_protocol(self.protocol.clone())
            .material(material)?)
    }
    /// Script the next finite author requests; exhausted scripts include an inner.
    pub fn script(&self, scripts: impl IntoIterator<Item = Inner>) {
        lock(&self.control).scripts.extend(scripts);
    }
    /// Observe the physical carrier inclusion before synthetic inner handling.
    /// Used to install a causal signer pause for a following reseal.
    pub fn after_author(
        &self,
        hook: impl FnMut(&mut FakeState, &OperationRecord, u64) + Send + 'static,
    ) {
        lock(&self.control).author_hook = Some(Box::new(hook));
    }
    /// Install synthetic native queue effects after actual carrier author I/O.
    pub fn after_inclusion(
        &self,
        hook: impl FnMut(&mut FakeState, &OperationRecord, u64) + Send + 'static,
    ) {
        lock(&self.control).hook = Some(Box::new(hook));
    }
    /// Wait for a scripted lost acknowledgment after actual author I/O. The
    /// caller can then drop the real socket without a preparation-time timer.
    pub async fn lost_acknowledgment(&self) {
        let entered = lock(&self.control).author_entered.clone();
        entered.notified().await;
    }
    /// Actual author RPC count, not a transport/consensus authority statement.
    pub fn authors(&self) -> usize {
        self.node
            .requests()
            .iter()
            .filter(|r| r.method.starts_with("author_submit"))
            .count()
    }
    /// Publish a controlled shape-compatible runtime upgrade at a new block.
    /// Historical code/version identities remain pinned; approval of the new
    /// code is a separate explicit `protocol.allow` decision.
    pub fn upgrade(
        &self,
        spec_version: u32,
        transaction_version: u32,
        code: H256,
    ) -> LabResult<()> {
        self.node.mutate_state(|state| -> LabResult<()> {
            let runtime = rtao_core::runtime::Runtime::parse(
                &state.runtime.metadata_bytes,
                spec_version,
                transaction_version,
                state.runtime.ss58_format,
            )?;
            let mut control = lock(&self.control);
            for height in control.runtime_floor..=state.block {
                if let Some(hash) = state.block_hash(height) {
                    state.set_runtime_at(hash.clone(), state.runtime.clone());
                    let previous = control.code;
                    control.historical_codes.entry(hash).or_insert(previous);
                }
            }
            state.advance(1);
            control.runtime_floor = state.block;
            state.runtime = Arc::new(runtime);
            control.code = code;
            Ok(())
        })
    }
    /// Release this process's native journal lock while keeping only the
    /// synthetic peer alive for an executable child-process crash/recovery.
    /// Author I/O inspects the child's bounded atomic disk publication.
    pub async fn release_for_process(&mut self) -> LabResult<()> {
        self.client.close().await;
        let original = self.journal.take().ok_or("journal already released")?;
        if Arc::strong_count(&original) != 1 {
            self.journal = Some(original);
            return Err("drop caller journal/operation owners before releasing".into());
        }
        drop(original);
        lock(&self.control).external_journal = Some(self.path.clone());
        Ok(())
    }
    /// Close actual author transport, drop every owned journal handle and reopen
    /// the original file and a new native Session. Retained external owners refuse.
    pub async fn restart(&mut self) -> LabResult<()> {
        if self.journal.is_some() {
            self.release_for_process().await?;
        } else if lock(&self.control).external_journal.is_none() {
            return Err("journal restart incomplete".into());
        }
        let journal = OperationJournal::open(&self.path)?;
        {
            let mut control = lock(&self.control);
            control.journal = Arc::downgrade(&journal);
            control.history_unavailable = false;
            control.external_journal = None;
        }
        self.journal = Some(journal);
        let session = rtao_rpc::Session::connect(rtao_rpc::SessionConfig::new(url::Url::parse(
            self.node.url(),
        )?))
        .await?;
        self.client = Client::from_session(session, self.config.clone()).await?;
        self.client.enable_synthetic_journal_dispatch().await?;
        Ok(())
    }
    /// End owned sockets and journal handles, preserving files for inspection.
    pub async fn close(&mut self) {
        self.client.close().await;
        self.node.shutdown().await;
        self.journal.take();
    }
}
