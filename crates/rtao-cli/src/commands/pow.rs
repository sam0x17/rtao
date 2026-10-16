// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/context.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Additional source at the same commit/license: sdk/python/bittensor/pow_registration.py.
//! Public and late work refresh preserve the registration target and signer.

use crate::{context::SubmissionExecution, error::CliError, output::Output};
use rtao::{
    client::{BlockRef, Client, Inclusion, ViewExt},
    signer::{BoxFuture, DynKeySource, KeyView, Signer, SignerError, SignerRole},
    tx::{DynIntent, ExecuteOptions, ExtrinsicResult, OriginView, PlanOptions},
};
use rtao_intents::{
    AnyIntent, PowRegister,
    pow::{CancellationToken, MAX_WORK_AGE_BLOCKS, PowConfig, PowSolver, effective_difficulty},
};
use rtao_runtime::storage::subtensor_module as st;
use std::{sync::Arc, time::Duration};

fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}

/// Public preparation and late execution for unshielded direct requests.
pub(super) struct Execution {
    output: Output,
}
impl Execution {
    pub(super) fn new(intent: &AnyIntent, output: &Output) -> Option<Self> {
        matches!(intent, AnyIntent::PowRegister(_)).then(|| Self {
            output: output.clone(),
        })
    }
}
impl SubmissionExecution for Execution {
    fn prepare<'a>(
        &'a self,
        client: &'a Client,
        intent: &'a dyn DynIntent,
        public: &'a dyn KeyView,
        options: &'a PlanOptions,
    ) -> BoxFuture<'a, Result<Option<Box<dyn DynIntent>>, CliError>> {
        Box::pin(async move {
            let intent = AnyIntent::from_tool(intent.spec().name, intent.to_json_dyn())
                .map_err(|error| CliError::input(error.to_string()))?;
            let origin = options
                .proxy_for
                .map_or(OriginView::Keys(public), OriginView::Account);
            Ok(Some(
                refresh_with_origin(client, intent, &origin)
                    .await?
                    .into_dyn(),
            ))
        })
    }
    fn execute<'a>(
        &'a self,
        client: &'a Client,
        intent: &'a dyn DynIntent,
        keys: &'a dyn DynKeySource,
        options: &'a ExecuteOptions,
    ) -> BoxFuture<'a, Result<ExtrinsicResult, CliError>> {
        Box::pin(async move {
            let intent = AnyIntent::from_tool(intent.spec().name, intent.to_json_dyn())
                .map_err(|error| CliError::input(error.to_string()))?;
            // Unlock after the reviewed plan and confirmation. Cache that signer
            // so executor acquisition cannot introduce another password delay.
            let role = intent.as_dyn().signer_role_dyn().map_err(failure)?;
            let signer = keys.signer_for(role).await.map_err(failure)?;
            let unlocked = UnlockedKeys { keys, signer, role };
            // The signing delegate pays and signs; work still authenticates the
            // reviewed dispatch owner, exactly as the SDK's proxy build does.
            let origin = options
                .proxy_for
                .map_or(OriginView::Keys(&unlocked), OriginView::Account);
            let mut intent = refresh_with_origin(client, intent, &origin).await?;
            let executor = rtao::Executor::new(client.clone());
            for attempt in 0..3 {
                let receipt = executor
                    .execute_dyn(intent.as_dyn(), &unlocked, options)
                    .await
                    .map_err(CliError::transaction)?;
                if attempt == 2 || !retriable_pool_rejection(&receipt) {
                    return Ok(receipt);
                }
                let AnyIntent::PowRegister(pow) = &intent else {
                    return Ok(receipt);
                };
                let head = client.at(BlockRef::Best).await.map_err(CliError::from)?;
                if !head
                    .block()
                    .number
                    .checked_sub(pow.work_block)
                    .is_some_and(|age| age >= MAX_WORK_AGE_BLOCKS)
                {
                    return Ok(receipt);
                }
                self.output
                    .message("registration proof expired before inclusion; mining a fresh proof")
                    .map_err(failure)?;
                intent = AnyIntent::PowRegister(mine(client, pow.clone(), &origin).await?);
            }
            Err(CliError::failure("PoW submission attempts exhausted"))
        })
    }
}

struct UnlockedKeys<'a> {
    keys: &'a dyn DynKeySource,
    signer: Arc<dyn Signer>,
    role: SignerRole,
}
impl KeyView for UnlockedKeys<'_> {
    fn coldkey_address(&self) -> Result<rtao::primitives::AccountId, SignerError> {
        self.keys.coldkey_address()
    }
    fn hotkey_address(&self) -> Result<rtao::primitives::AccountId, SignerError> {
        self.keys.hotkey_address()
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        self.keys.hotkey_public_key()
    }
    fn bare_signer_account(&self) -> Option<rtao::primitives::AccountId> {
        self.keys.bare_signer_account()
    }
}
impl DynKeySource for UnlockedKeys<'_> {
    fn signer_for<'a>(
        &'a self,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move {
            if role == self.role {
                Ok(self.signer.clone())
            } else {
                self.keys.signer_for(role).await
            }
        })
    }
}

fn retriable_pool_rejection(receipt: &ExtrinsicResult) -> bool {
    !receipt.success
        && receipt.block_hash.is_none()
        && receipt.extrinsic_id.is_none()
        && receipt.events.is_empty()
        && receipt.fee.is_none()
        && receipt.fate_unknown.is_none()
        && receipt.inclusion == Inclusion::Unobserved
        && receipt
            .error
            .as_ref()
            .and_then(|error| error.name.as_deref())
            .is_some_and(|name| matches!(name, "InvalidWorkBlock" | "BadRequest"))
}

// Check little-endian work * difficulty <= 2^256 - 1 without rounding.
fn meets_difficulty(work: &[u8; 32], difficulty: u64) -> bool {
    let mut carry = 0u128;
    for limb in work.as_chunks::<8>().0 {
        let product = u128::from(u64::from_le_bytes(*limb)) * u128::from(difficulty) + carry;
        carry = product >> 64;
    }
    carry == 0
}

/// Refresh only future, expired or difficulty-invalid work. A fresh invalid seal
/// reaches the SDK's ordinary validation failure instead of being replaced.
/// This helper requires only public identities and starts real CPU workers.
pub async fn refresh_if_needed(
    client: &Client,
    intent: AnyIntent,
    public: &dyn KeyView,
) -> Result<AnyIntent, CliError> {
    refresh_with_origin(client, intent, &OriginView::Keys(public)).await
}

async fn refresh_with_origin(
    client: &Client,
    intent: AnyIntent,
    origin: &OriginView<'_>,
) -> Result<AnyIntent, CliError> {
    let AnyIntent::PowRegister(pow) = intent else {
        return Ok(intent);
    };
    let at = client.at(BlockRef::Best).await.map_err(CliError::from)?;
    let (difficulty, minimum, maximum) = at
        .fetch((
            st::DIFFICULTY.at((pow.netuid,)),
            st::MIN_DIFFICULTY.at((pow.netuid,)),
            st::MAX_DIFFICULTY.at((pow.netuid,)),
        ))
        .await
        .map_err(CliError::from)?;
    let difficulty = effective_difficulty(difficulty, minimum, maximum);
    let text = pow.work_hex.strip_prefix("0x").unwrap_or(&pow.work_hex);
    // AnyIntent has already checked Python's byte-pair whitespace syntax.
    let bytes = hex::decode(
        text.chars()
            .filter(|c| !c.is_ascii_whitespace())
            .collect::<String>(),
    )
    .map_err(|e| CliError::input(e.to_string()))?;
    let work: [u8; 32] = bytes
        .try_into()
        .map_err(|_| CliError::input("PoW seal must be 32 bytes"))?;
    let fresh = at
        .block()
        .number
        .checked_sub(pow.work_block)
        .is_some_and(|age| age < MAX_WORK_AGE_BLOCKS);
    if fresh && meets_difficulty(&work, difficulty) {
        return Ok(AnyIntent::PowRegister(pow));
    }
    Ok(AnyIntent::PowRegister(mine(client, pow, origin).await?))
}

async fn mine(
    client: &Client,
    mut pow: PowRegister,
    public: &dyn KeyView,
) -> Result<PowRegister, CliError> {
    let coldkey = public
        .coldkey_address()
        .or_else(|error| public.bare_signer_account().ok_or(error))
        .map_err(failure)?;
    let hotkey = match &pow.hotkey_ss58 {
        Some(address) => address.account(),
        None => public.hotkey_address().map_err(failure)?,
    };
    validate_backend(&pow.mining_backend, pow.mining_device_ids.as_deref())?;
    let workers = u8::try_from(pow.mining_workers)
        .map_err(|_| CliError::input("PoW workers must be 1..32"))?;
    let seconds = pow
        .mining_timeout
        .as_f64()
        .filter(|v| v.is_finite())
        .ok_or_else(|| CliError::input("PoW timeout must be finite"))?;
    let timeout =
        Duration::try_from_secs_f64(seconds).map_err(|e| CliError::input(e.to_string()))?;
    let solver = PowSolver::new(PowConfig { workers, timeout })
        .map_err(|e| CliError::input(e.to_string()))?;
    let proof = solve_public(
        &solver,
        client,
        pow.netuid,
        hotkey,
        coldkey,
        &pow.mining_backend,
        pow.mining_device_ids.as_deref(),
    )
    .await?;
    pow.work_block = proof.work_block;
    pow.nonce = proof.nonce;
    pow.work_hex = format!("0x{}", hex::encode(proof.work));
    Ok(pow)
}

pub(super) fn validate_backend(backend: &str, devices: Option<&[u64]>) -> Result<(), CliError> {
    if !matches!(backend, "auto" | "cpu" | "gpu") {
        return Err(CliError::input("PoW backend must be auto, cpu or gpu"));
    }
    if let Some(devices) = devices {
        let unique = devices.iter().collect::<std::collections::BTreeSet<_>>();
        if backend == "cpu"
            || devices.is_empty()
            || devices.len() > 32
            || unique.len() != devices.len()
        {
            return Err(CliError::input(
                "select 1..=32 unique GPU devices with auto or gpu",
            ));
        }
        for device in devices {
            usize::try_from(*device)
                .map_err(|_| CliError::input("GPU device id exceeds platform usize"))?;
        }
    }
    if !cfg!(feature = "pow-gpu") && (backend == "gpu" || devices.is_some()) {
        return Err(CliError::input("GPU PoW requires the CLI pow-gpu feature"));
    }
    Ok(())
}

pub(super) async fn solve_public(
    solver: &PowSolver,
    client: &Client,
    netuid: rtao::primitives::NetUid,
    hotkey: rtao::primitives::AccountId,
    coldkey: rtao::primitives::AccountId,
    backend: &str,
    devices: Option<&[u64]>,
) -> Result<rtao_intents::pow::PowProof, CliError> {
    validate_backend(backend, devices)?;
    #[cfg(feature = "pow-gpu")]
    {
        use rtao_intents::pow::{GpuOptions, PowBackend};
        let options = GpuOptions {
            backend: match backend {
                "cpu" => PowBackend::Cpu,
                "gpu" => PowBackend::Gpu,
                _ => PowBackend::Auto,
            },
            device_ids: devices
                .map(|ids| {
                    ids.iter()
                        .map(|id| usize::try_from(*id))
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()
                .map_err(failure)?,
            ..Default::default()
        };
        solver
            .solve_with_backend(
                client,
                netuid,
                hotkey,
                coldkey,
                &CancellationToken::new(),
                &options,
            )
            .await
            .map_err(CliError::from)
    }
    #[cfg(not(feature = "pow-gpu"))]
    solver
        .solve(client, netuid, hotkey, coldkey, &CancellationToken::new())
        .await
        .map_err(CliError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtao::primitives::{Balance, H256, NetUid, Tao};
    use rtao::tx::ChainError;
    use rtao_intents::pow::registration_prefix;
    use serde_json::Map;

    #[test]
    fn difficulty_comparison_matches_the_actual_core_cpu_predicate() -> Result<(), String> {
        let prefix = registration_prefix(
            NetUid(1),
            H256([1; 32]),
            rtao::primitives::AccountId([2; 32]),
            rtao::primitives::AccountId([3; 32]),
        )
        .map_err(|e| e.to_string())?;
        for difficulty in [1, 2, 19, u64::MAX] {
            for nonce in [0, 1, 4, u64::MAX] {
                let seal = rtao::core::pow::seal(&prefix, nonce)?;
                assert_eq!(
                    meets_difficulty(&seal, difficulty),
                    rtao::core::pow::mine_cpu(&prefix, difficulty, nonce, 1)?.is_some()
                );
            }
        }
        assert!(meets_difficulty(&[0; 32], u64::MAX));
        assert!(meets_difficulty(&[255; 32], 1));
        assert!(!meets_difficulty(&[255; 32], 2));
        Ok(())
    }

    #[test]
    fn retry_predicate_refuses_inclusion_acceptance_and_unknown_delivery() {
        let rejection = ExtrinsicResult {
            success: false,
            message: "expired proof".into(),
            block_hash: None,
            extrinsic_id: None,
            explorer_url: None,
            fee: None,
            events: Vec::new(),
            error: Some(ChainError::new(
                "expired proof",
                Some("InvalidWorkBlock".into()),
            )),
            data: Map::new(),
            output: (),
            inclusion: Inclusion::Unobserved,
            fate_unknown: None,
        };
        assert!(retriable_pool_rejection(&rejection));
        let mut receipt = rejection.clone();
        receipt.block_hash = Some(H256([1; 32]));
        assert!(!retriable_pool_rejection(&receipt));
        let mut receipt = rejection.clone();
        receipt.extrinsic_id = Some("100-1".into());
        assert!(!retriable_pool_rejection(&receipt));
        let mut receipt = rejection.clone();
        receipt.fee = Some(Balance::<Tao>::ZERO);
        assert!(!retriable_pool_rejection(&receipt));
        let mut receipt = rejection.clone();
        receipt.fate_unknown = Some(H256([2; 32]));
        assert!(!retriable_pool_rejection(&receipt));
        for inclusion in [
            Inclusion::PoolAccepted,
            Inclusion::InBlock,
            Inclusion::Finalized,
        ] {
            let mut receipt = rejection.clone();
            receipt.inclusion = inclusion;
            assert!(!retriable_pool_rejection(&receipt));
        }
        for name in [
            None,
            Some("InvalidSeal"),
            Some("InvalidDifficulty"),
            Some("Expired"),
        ] {
            let mut receipt = rejection.clone();
            receipt.error = name.map(|name| ChainError::new(name, Some(name.into())));
            assert!(!retriable_pool_rejection(&receipt));
        }
    }
}
