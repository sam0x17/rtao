// Derived from RaoFoundation/subtensor pallets/subtensor/src/subnets/subnet.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Owner orchestration over reviewed intents and live chain windows.

mod routing;
mod trim;
pub use trim::{TrimNullAction, TrimNullConfig, TrimNullSession, TrimNullStep, TrimReport};
#[cfg(test)]
mod tests;

use crate::{HparamArg, HyperparamPlanner, WindowWait};
use futures::StreamExt;
use rtao_client::{BlockRef, Client, ClientError, Finality, ViewExt};
use rtao_intents::{
    RegisterSubnet, SetHyperparameter, SetMechanismCount, SetSubnetIdentity, StartCall, TrimSubnet,
    UpdateSymbol,
};
use rtao_primitives::{AccountId, MechId, NetUid};
use rtao_runtime::storage::subtensor_module as st;
use rtao_signer::{Coldkey, KeySource};
use rtao_tx::{
    ChainError, ExecuteOptions, Executor, ExtrinsicResult, Intent, IntentError, PlanOptions,
    PolicyError, SubnetRegistered, TxError,
};
use std::{future::Future, path::Path, sync::Arc, time::Duration};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// Bounded framework options shared by owner methods.
#[derive(Clone, Debug)]
pub struct OwnerConfig {
    /// Deadline covering public reads, signing, submission and block waits.
    pub operation_timeout: Option<Duration>,
    /// Maximum finalized Null batches before a resumable stop.
    pub trim_max_batches: u32,
}
impl Default for OwnerConfig {
    fn default() -> Self {
        Self {
            operation_timeout: None,
            trim_max_batches: 128,
        }
    }
}

/// Registration completion preference, preserving the executor's correlation checks.
#[derive(Clone, Copy, Debug)]
pub enum RegistrationWait {
    /// Return after the registration extrinsic, without following queued creation.
    Receipt,
    /// Follow immediate or queued NetworkAdded, optionally bounded by timeout.
    UntilRegistered {
        /// Completion timeout; None preserves the chain follower's unbounded default.
        timeout: Option<Duration>,
    },
}
impl Default for RegistrationWait {
    fn default() -> Self {
        Self::UntilRegistered { timeout: None }
    }
}

/// Human-readable UTF-8 subnet identity fields.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubnetIdentity {
    /// Display name.
    pub subnet_name: String,
    /// Code repository URL.
    pub github_repo: String,
    /// Operator contact.
    pub subnet_contact: String,
    /// Website URL.
    pub subnet_url: String,
    /// Discord handle or invite.
    pub discord: String,
    /// Subnet description.
    pub description: String,
    /// Logo URL.
    pub logo_url: String,
    /// Additional public information.
    pub additional: String,
}

/// Owner coldkey operations with public planning before private-key access.
pub struct OwnerOps<K: KeySource<Coldkey>> {
    pub(crate) executor: Executor,
    pub(crate) keys: Arc<K>,
    pub(crate) options: ExecuteOptions,
    pub(crate) config: OwnerConfig,
    pub(crate) cancel: CancellationToken,
}

impl<K: KeySource<Coldkey>> Clone for OwnerOps<K> {
    fn clone(&self) -> Self {
        Self {
            executor: self.executor.clone(),
            keys: self.keys.clone(),
            options: self.options.clone(),
            config: self.config.clone(),
            cancel: self.cancel.clone(),
        }
    }
}

impl<K: KeySource<Coldkey>> OwnerOps<K> {
    /// Use an existing policy-bearing executor and lazy coldkey source.
    pub fn new(executor: Executor, keys: Arc<K>) -> Self {
        Self {
            executor,
            keys,
            options: ExecuteOptions {
                reconcile: true,
                ..Default::default()
            },
            config: OwnerConfig::default(),
            cancel: CancellationToken::new(),
        }
    }
    /// Configure operation deadlines and batch limits.
    pub fn with_config(mut self, config: OwnerConfig) -> Result<Self, TxError> {
        if config.trim_max_batches == 0 || config.operation_timeout.is_some_and(|d| d.is_zero()) {
            return Err(invalid("owner timeouts and batch limits must be positive"));
        }
        self.config = config;
        Ok(self)
    }
    /// Select typed routing options; the original coldkey remains the signer.
    pub fn with_execute_options(mut self, options: ExecuteOptions) -> Result<Self, TxError> {
        if options.force_proxy_type.is_some() && options.proxy_for.is_none() {
            return Err(invalid("force_proxy_type requires proxy_for"));
        }
        self.options = options;
        Ok(self)
    }
    /// Cancel public preparation and waits, and prevent another owner step.
    ///
    /// An entered execution drains to its receipt or deadline so cancellation
    /// cannot hide a broadcast transaction's hash or unknown fate. Configure an
    /// operation timeout to bound that drain when finality remains silent.
    /// The current executor call, including its configured internal retries and
    /// queued registration follow, finishes before cancellation is observed.
    pub fn with_cancellation(mut self, cancel: CancellationToken) -> Self {
        self.cancel = cancel;
        self
    }
    /// Borrow the public connected chain client.
    pub fn client(&self) -> &Client {
        self.executor.client()
    }

    /// Create a subnet and optionally follow its correlated queued registration.
    pub async fn create_subnet(
        &self,
        hotkey: AccountId,
        wait: RegistrationWait,
    ) -> Result<SubnetRegistered, TxError> {
        let operation = self.operation()?;
        let mut options = self.options.clone();
        match wait {
            RegistrationWait::Receipt => options.wait_for_registration = false,
            RegistrationWait::UntilRegistered { timeout } => {
                options.wait_for_registration = true;
                options.wait_for_inclusion = true;
                options.registration_timeout = timeout;
            }
        }
        let intent = RegisterSubnet {
            hotkey_ss58: Some(hotkey.into()),
        };
        operation.read(self.review(&intent)).await?;
        let result = operation.execute(&intent, &options).await?;
        let result = successful(result)?;
        if matches!(wait, RegistrationWait::UntilRegistered { .. })
            && result.output.netuid.is_none()
        {
            return Err(invalid(
                "registration receipt did not prove correlated NetworkAdded",
            ));
        }
        Ok(result.output)
    }

    /// Wait for the current mutable StartCallDelay schedule, then activate the subnet.
    pub async fn start(&self, netuid: NetUid) -> Result<ExtrinsicResult, TxError> {
        let operation = self.operation()?;
        operation
            .read(async {
                self.ensure_owner(netuid).await?;
                let mut heads = self.client().blocks_strict(Finality::Best).await?;
                loop {
                    let snapshot = self.client().at(BlockRef::Best).await?;
                    let schedule =
                        rtao_reads::reads::owner::subnet_start_schedule(&snapshot, netuid)
                            .await
                            .map_err(|e| invalid(&e.to_string()))?;
                    if u128::from(snapshot.block().number) >= schedule.earliest_start_block {
                        break;
                    }
                    next_head(&mut heads).await?;
                    self.ensure_owner(netuid).await?;
                }
                Ok(())
            })
            .await?;
        self.execute_owner(&operation, &StartCall { netuid }, netuid)
            .await
    }

    /// Set a shared hyperparameter, replan exactly once for an included rate-limit race.
    pub async fn set_hparam(
        &self,
        netuid: NetUid,
        name: &str,
        value: HparamArg,
        wait: WindowWait,
    ) -> Result<ExtrinsicResult, TxError> {
        let operation = self.operation()?;
        let planner = HyperparamPlanner::new(self.effective_owner()?);
        for attempt in 0..=1 {
            let intent = operation
                .read(async {
                    let plan = planner
                        .plan_with_wait(
                            self.client(),
                            netuid,
                            name,
                            value.clone(),
                            wait,
                            &self.cancel,
                        )
                        .await?;
                    if !plan.reasons.is_empty() {
                        return Err(policy(plan.reasons));
                    }
                    let mut raw = plan.raw.to_json();
                    if let Some(bits) = raw.get("bits") {
                        raw = bits.clone();
                    }
                    let intent = SetHyperparameter {
                        netuid,
                        name: name.into(),
                        value: raw,
                    };
                    self.review(&intent).await?;
                    Ok(intent)
                })
                .await?;
            let mut options = self.options.clone();
            options.retries = 0;
            options.replan_owner_rate_limit = false;
            let result = operation.execute(&intent, &options).await?;
            if attempt == 0 && exact_rate_race(&result) {
                continue;
            }
            return Ok(result);
        }
        Err(invalid("owner hyperparameter retry exhausted"))
    }

    /// Publish a subnet's public identity through the policy-bearing executor.
    pub async fn set_identity(
        &self,
        netuid: NetUid,
        identity: SubnetIdentity,
    ) -> Result<ExtrinsicResult, TxError> {
        let operation = self.operation()?;
        self.execute_owner(
            &operation,
            &SetSubnetIdentity {
                netuid,
                subnet_name: identity.subnet_name,
                github_repo: identity.github_repo,
                subnet_contact: identity.subnet_contact,
                subnet_url: identity.subnet_url,
                discord: identity.discord,
                description: identity.description,
                logo_url: identity.logo_url,
                additional: identity.additional,
            },
            netuid,
        )
        .await
    }
    /// Change the token symbol using the existing validated owner intent.
    pub async fn update_symbol(
        &self,
        netuid: NetUid,
        symbol: &str,
    ) -> Result<ExtrinsicResult, TxError> {
        let operation = self.operation()?;
        self.execute_owner(
            &operation,
            &UpdateSymbol {
                netuid,
                symbol: symbol.into(),
            },
            netuid,
        )
        .await
    }
    /// Set a bounded mechanism count, respecting its freeze and owner cooldown.
    pub async fn set_mechanism_count(
        &self,
        netuid: NetUid,
        count: u8,
    ) -> Result<ExtrinsicResult, TxError> {
        let operation = self.operation()?;
        operation
            .read(async {
                self.ensure_owner(netuid).await?;
                let cap = self.client().storage(&st::MAX_MECHANISM_COUNT, ()).await?;
                if count == 0 || count > cap.0 || count > MechId::LIMIT {
                    return Err(invalid("mechanism count exceeds the live runtime cap"));
                }
                Ok(())
            })
            .await?;
        self.execute_owner(
            &operation,
            &SetMechanismCount {
                netuid,
                mechanism_count: MechId(count),
            },
            netuid,
        )
        .await
    }
    /// Trim subnet population with the source-backed owner preflight.
    pub async fn trim(&self, netuid: NetUid, max_n: u16) -> Result<ExtrinsicResult, TxError> {
        let operation = self.operation()?;
        operation
            .read(async {
                self.ensure_owner(netuid).await?;
                let (minimum, maximum) = self
                    .client()
                    .fetch((
                        st::MIN_ALLOWED_UIDS.at((netuid,)),
                        st::MAX_ALLOWED_UIDS.at((netuid,)),
                    ))
                    .await?;
                if !(minimum..=maximum).contains(&max_n) {
                    return Err(invalid("trim target exceeds the live runtime UID bounds"));
                }
                Ok(())
            })
            .await?;
        self.execute_owner(&operation, &TrimSubnet { netuid, max_n }, netuid)
            .await
    }
    /// Resume at most 64 UID removals per finalized Null batch.
    ///
    /// The recovery record is fsynced before signing. Unresolved signing,
    /// delivery or unverified state advancement retains it and blocks restart
    /// until inclusion or expiry is reconciled. A sibling `.lock` file remains
    /// for stable advisory locking. `TrimReport::already_yuma` reports a valid
    /// Yuma subnet without pruning, which can exceed the requested target.
    pub async fn trim_null(
        &self,
        netuid: NetUid,
        target: u16,
        recovery: &Path,
    ) -> Result<TrimReport, TxError> {
        let operation = self.operation()?;
        self.trim_null_inner(&operation, netuid, target, recovery)
            .await
    }

    pub(crate) async fn review<I: Intent<Role = Coldkey> + Clone>(
        &self,
        intent: &I,
    ) -> Result<(), TxError> {
        let plan = self
            .executor
            .plan(
                &self.routed(intent),
                self.keys.as_ref(),
                &PlanOptions {
                    policy: self.options.policy.clone(),
                    deadline: self.options.deadline,
                    proxy_for: self.options.proxy_for,
                    force_proxy_type: self.options.force_proxy_type,
                },
            )
            .await?;
        if !plan.ok() {
            return Err(policy(plan.violations));
        }
        Ok(())
    }
    pub(crate) async fn execute_owner<I: Intent<Role = Coldkey, Output = ()> + Clone>(
        &self,
        operation: &OwnerOperation<'_, K>,
        intent: &I,
        netuid: NetUid,
    ) -> Result<ExtrinsicResult, TxError> {
        operation
            .read(async {
                self.ensure_owner(netuid).await?;
                self.review(intent).await
            })
            .await?;
        operation.execute(intent, &self.options).await
    }
    pub(crate) async fn ensure_owner(&self, netuid: NetUid) -> Result<(), TxError> {
        let (exists, owner) = self
            .client()
            .fetch((
                st::NETWORKS_ADDED.at((netuid,)),
                st::SUBNET_OWNER.at((netuid,)),
            ))
            .await?;
        if !exists {
            return Err(TxError::Chain(ChainError::new(
                "subnet does not exist",
                Some("SubnetNotExists".into()),
            )));
        }
        if owner.get() != Some(&self.effective_owner()?) {
            return Err(TxError::Chain(ChainError::new(
                "effective origin is not subnet owner",
                Some("BadOrigin".into()),
            )));
        }
        Ok(())
    }
    pub(crate) fn effective_owner(&self) -> Result<AccountId, TxError> {
        match self.options.proxy_for {
            Some(real) => Ok(real),
            None => Ok(self.keys.address()?),
        }
    }
    fn routed<I: Intent + Clone>(&self, intent: &I) -> routing::Routed<I> {
        routing::Routed {
            intent: intent.clone(),
            proxy_for: self.options.proxy_for,
            force_proxy_type: self.options.force_proxy_type,
        }
    }
    fn operation(&self) -> Result<OwnerOperation<'_, K>, TxError> {
        let timeout = match (self.config.operation_timeout, self.options.deadline) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, right) => left.or(right),
        };
        let deadline = timeout
            .map(|d| {
                Instant::now()
                    .checked_add(d)
                    .ok_or_else(|| invalid("owner deadline overflow"))
            })
            .transpose()?;
        Ok(OwnerOperation {
            owner: self,
            deadline,
        })
    }
}

/// One absolute budget across preparation, retries and batch execution.
pub(crate) struct OwnerOperation<'a, K: KeySource<Coldkey>> {
    owner: &'a OwnerOps<K>,
    deadline: Option<Instant>,
}
impl<K: KeySource<Coldkey>> OwnerOperation<'_, K> {
    fn check(&self) -> Result<(), TxError> {
        if self.owner.cancel.is_cancelled() {
            return Err(invalid("owner operation cancelled"));
        }
        if self.owner.client().is_closed() {
            return Err(TxError::ConnectionNotReady);
        }
        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(invalid("owner operation deadline expired"));
        }
        Ok(())
    }

    pub(crate) async fn read<T>(
        &self,
        future: impl Future<Output = Result<T, TxError>>,
    ) -> Result<T, TxError> {
        self.check()?;
        tokio::select! {
            biased;
            _ = self.owner.cancel.cancelled() => Err(invalid("owner operation cancelled")),
            _ = self.owner.client().closed() => Err(TxError::ConnectionNotReady),
            _ = async { match self.deadline { Some(d) => tokio::time::sleep_until(d).await, None => futures::future::pending().await } } => Err(invalid("owner operation deadline expired")),
            result = future => result,
        }
    }

    pub(crate) async fn execute<I: Intent<Role = Coldkey> + Clone>(
        &self,
        intent: &I,
        options: &ExecuteOptions,
    ) -> Result<ExtrinsicResult<I::Output>, TxError> {
        self.check()?;
        let mut options = options.clone();
        options.deadline = self
            .deadline
            .map(|d| d.saturating_duration_since(Instant::now()));
        // Client submission owns the attempted hash and converts an expired
        // postbroadcast wait into unknown fate. Never drop that future here.
        self.owner
            .executor
            .execute(
                &self.owner.routed(intent),
                self.owner.keys.as_ref(),
                &options,
            )
            .await
    }
}

pub(crate) fn invalid(message: &str) -> TxError {
    IntentError::InvalidArgument(message.into()).into()
}
pub(crate) fn policy(violations: Vec<String>) -> TxError {
    TxError::Policy(PolicyError { violations })
}
pub(crate) fn successful<T>(result: ExtrinsicResult<T>) -> Result<ExtrinsicResult<T>, TxError> {
    if let Some(hash) = result.fate_unknown {
        return Err(TxError::Rpc(rtao_rpc::RpcError::FateUnknown {
            hash: rtao_rpc::H256(hash.0),
        }));
    }
    if result.success && result.fate_unknown.is_none() {
        Ok(result)
    } else {
        Err(TxError::Chain(
            result
                .error
                .unwrap_or_else(|| ChainError::new(result.message, None)),
        ))
    }
}
fn exact_rate_race(result: &ExtrinsicResult) -> bool {
    !result.success
        && result.fate_unknown.is_none()
        && matches!(
            result.inclusion,
            rtao_client::Inclusion::InBlock | rtao_client::Inclusion::Finalized
        )
        && result.block_hash.is_some()
        && result.extrinsic_id.is_some()
        && result
            .error
            .as_ref()
            .is_some_and(|error| error.name.as_deref() == Some("TxRateLimitExceeded"))
}
pub(crate) async fn next_head(heads: &mut rtao_client::head::BlockStream) -> Result<(), TxError> {
    match heads.next().await {
        Some(Ok(_)) | Some(Err(rtao_client::HeadError::Reorg(_))) => Ok(()),
        Some(Err(e)) => Err(ClientError::Protocol(e.to_string()).into()),
        None => Err(ClientError::Protocol("owner head stream ended".into()).into()),
    }
}
