// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_pow_registration.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

use super::*;
use rtao_client::{ClientConfig, Network};
use rtao_testkit::{FakeChain, FakeState};

const NETUID: NetUid = NetUid(1);
const HOT: AccountId = AccountId([2; 32]);
const COLD: AccountId = AccountId([3; 32]);

// Real CPU threads are outside Tokio's paused-clock scheduler. Keep one task
// runnable so automatic time advances cannot skip past their in-flight work.
struct ManualClock(tokio::task::JoinHandle<()>);
impl ManualClock {
    fn new() -> Self {
        Self(tokio::spawn(async {
            loop {
                tokio::task::yield_now().await;
            }
        }))
    }
}
impl Drop for ManualClock {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn seed(state: &mut FakeState, item: &str, value: u64) {
    state
        .seed_typed("SubtensorModule", item, (NETUID,), value)
        .unwrap();
}

async fn fixture() -> (Client, FakeChain) {
    let mut state = FakeState::new(FakeState::default().runtime);
    state.set_finalized(98).unwrap();
    state
        .seed_typed(
            "SubtensorModule",
            "NetworkRegistrationAllowed",
            (NETUID,),
            false,
        )
        .unwrap();
    state
        .seed_typed(
            "SubtensorModule",
            "NetworkPowRegistrationAllowed",
            (NETUID,),
            true,
        )
        .unwrap();
    seed(&mut state, "Difficulty", 1);
    seed(&mut state, "MinDifficulty", 1);
    seed(&mut state, "MaxDifficulty", u64::MAX);
    state
        .seed_typed(
            "SubtensorModule",
            "MaxRegistrationsPerBlock",
            (NETUID,),
            1u16,
        )
        .unwrap();
    state
        .seed_typed("SubtensorModule", "RegistrationsThisBlock", (NETUID,), 0u16)
        .unwrap();
    let fake = FakeChain::from_state(state);
    let mut config =
        ClientConfig::new(Network::resolve_with("ws://127.0.0.1:9944", |_| None).unwrap()).unwrap();
    config.runtime_cache_dir = None;
    config.config_store = None;
    config.track_heads = false;
    let client = Client::from_rpc(fake.clone().into(), config).await.unwrap();
    fake.mutate_state(|state| state.requests.clear());
    (client, fake)
}

fn solver(timeout: Duration) -> PowSolver {
    PowSolver::new(PowConfig {
        workers: 1,
        timeout,
    })
    .unwrap()
}

async fn mine(client: &Client) -> Result<PowProof, PowError> {
    solver(Duration::from_secs(10))
        .solve(client, NETUID, HOT, COLD, &CancellationToken::new())
        .await
}

// pyt: unit/test_pow_registration.py::test_challenge_encoding_binds_both_keys_subnet_block_and_nonce
// The exact vector and valid typed runtime challenge are retained; the Python
// helper's 2^256 difficulty input is outside the native u64 difficulty API.
#[test]
fn runtime_prefix_and_authenticated_reference_vector() {
    let mut hot = [0; 32];
    hot[0] = 11;
    let mut cold = [0; 32];
    cold[0] = 22;
    let prefix =
        registration_prefix(NETUID, H256([1; 32]), AccountId(hot), AccountId(cold)).unwrap();
    let mut expected = DOMAIN.to_vec();
    expected.extend_from_slice(&[1, 0]);
    expected.extend_from_slice(&[1; 32]);
    expected.extend_from_slice(&hot);
    expected.extend_from_slice(&cold);
    assert_eq!(prefix.as_slice(), expected);
    assert_eq!(
        hex::encode(rtao_core::pow::seal(&prefix, 7).unwrap()),
        "06eef1fff090c8a0c4fbbbd17093346c9a45b3208fb3ff62524474cf4ba2cbc1"
    );
    assert_eq!(
        rtao_core::pow::mine_cpu(&prefix, 1, 7, 1).unwrap(),
        Some((7, rtao_core::pow::seal(&prefix, 7).unwrap()))
    );
    for changed in [
        registration_prefix(NetUid(2), H256([1; 32]), AccountId(hot), AccountId(cold)).unwrap(),
        registration_prefix(NETUID, H256([4; 32]), AccountId(hot), AccountId(cold)).unwrap(),
        registration_prefix(NETUID, H256([1; 32]), HOT, AccountId(cold)).unwrap(),
        registration_prefix(NETUID, H256([1; 32]), AccountId(hot), COLD).unwrap(),
    ] {
        assert_ne!(
            rtao_core::pow::seal(&changed, 7).unwrap(),
            rtao_core::pow::seal(&prefix, 7).unwrap()
        );
    }
    assert_ne!(
        rtao_core::pow::seal(&prefix, 8).unwrap(),
        rtao_core::pow::seal(&prefix, 7).unwrap()
    );
    assert!(matches!(
        registration_prefix(NetUid(0), H256([1; 32]), HOT, COLD),
        Err(PowError::RootSubnet)
    ));
    assert!(registration_prefix(NetUid(u16::MAX), H256([1; 32]), HOT, COLD).is_ok());
}

// pyt: unit/test_pow_registration.py::test_effective_difficulty_preserves_team_floor
// Python's None storage defaults become the native metadata's typed defaults.
#[test]
fn config_and_effective_bounds() {
    for workers in [0, 33, u8::MAX] {
        assert!(matches!(
            PowSolver::new(PowConfig {
                workers,
                ..PowConfig::default()
            }),
            Err(PowError::InvalidConfig)
        ));
    }
    for workers in [1, 32] {
        assert!(
            PowSolver::new(PowConfig {
                workers,
                ..PowConfig::default()
            })
            .is_ok()
        );
    }
    for timeout in [Duration::ZERO, Duration::from_secs(3601)] {
        assert!(matches!(
            PowSolver::new(PowConfig {
                timeout,
                ..PowConfig::default()
            }),
            Err(PowError::InvalidConfig)
        ));
    }
    assert!(
        PowSolver::new(PowConfig {
            timeout: Duration::from_nanos(1),
            ..PowConfig::default()
        })
        .is_ok()
    );
    assert!(
        PowSolver::new(PowConfig {
            timeout: Duration::from_secs(3600),
            ..PowConfig::default()
        })
        .is_ok()
    );
    for (value, minimum, maximum, expected) in [
        (1, 1000, 1, 1000),
        (2000, 1000, 1500, 1500),
        (0, 0, 0, 1),
        (u64::MAX, 1, u64::MAX, u64::MAX),
    ] {
        assert_eq!(effective_difficulty(value, minimum, maximum), expected);
    }
}

// pyt: unit/test_pow_registration.py::test_public_miner_returns_fresh_intent_without_a_signing_key
// PowProof replaces the Python intent return; the facade bridge has its own test.
#[tokio::test]
async fn public_cpu_proof_and_snapshot_pinned_admission() {
    let (client, fake) = fixture().await;
    let proof = mine(&client).await.unwrap();
    assert_eq!(proof.work_block, 100);
    assert_eq!((proof.hotkey, proof.coldkey), (HOT, COLD));
    let prefix = registration_prefix(NETUID, proof.block_hash, HOT, COLD).unwrap();
    assert_eq!(
        proof.work,
        rtao_core::pow::seal(&prefix, proof.nonce).unwrap()
    );
    let queries: Vec<_> = fake
        .requests()
        .into_iter()
        .filter(|r| r.method == "state_queryStorageAt")
        .collect();
    assert_eq!(queries.len(), 2);
    for query in queries {
        assert_eq!(query.params[0].as_array().unwrap().len(), 8);
        assert_eq!(
            query.params[1],
            format!("0x{}", hex::encode(proof.block_hash.0))
        );
    }
    assert!(
        fake.requests()
            .iter()
            .all(|r| !r.method.starts_with("author_"))
    );
}

// pyt: unit/test_pow_registration.py::test_miner_refuses_disabled_or_already_registered_subnets
#[tokio::test]
async fn rejects_disabled_uid_zero_and_zero_limit_before_workers() {
    let (client, fake) = fixture().await;
    fake.mutate_state(|s| {
        s.seed_typed(
            "SubtensorModule",
            "NetworkPowRegistrationAllowed",
            (NETUID,),
            false,
        )
        .unwrap()
    });
    assert!(matches!(mine(&client).await, Err(PowError::Disabled)));
    fake.mutate_state(|s| {
        s.seed_typed(
            "SubtensorModule",
            "NetworkPowRegistrationAllowed",
            (NETUID,),
            true,
        )
        .unwrap();
        s.seed_typed("SubtensorModule", "Uids", (NETUID, HOT), 0u16)
            .unwrap();
    });
    assert!(matches!(
        mine(&client).await,
        Err(PowError::AlreadyRegistered)
    ));
    let (client, fake) = fixture().await;
    fake.mutate_state(|s| {
        s.seed_typed(
            "SubtensorModule",
            "MaxRegistrationsPerBlock",
            (NETUID,),
            0u16,
        )
        .unwrap()
    });
    assert!(matches!(
        mine(&client).await,
        Err(PowError::ZeroRegistrationLimit)
    ));
    assert!(matches!(
        solver(Duration::from_secs(1))
            .solve(&client, NetUid(0), HOT, COLD, &CancellationToken::new())
            .await,
        Err(PowError::RootSubnet)
    ));
}

// pyt: unit/test_pow_registration.py::test_miner_waits_for_next_block_when_registration_capacity_is_used
// The native test observes noncompletion and the newly admitted block rather
// than replacing asyncio.sleep and asserting its exact call list.
#[tokio::test(start_paused = true)]
async fn waits_for_capacity_and_last_registration_block() {
    let _clock = ManualClock::new();
    for reused in [false, true] {
        let (client, fake) = fixture().await;
        fake.mutate_state(|s| {
            if reused {
                s.seed_typed(
                    "SubtensorModule",
                    "LastPowRegistrationBlock",
                    (HOT,),
                    100u64,
                )
                .unwrap();
            } else {
                s.seed_typed("SubtensorModule", "RegistrationsThisBlock", (NETUID,), 1u16)
                    .unwrap();
            }
        });
        let running = tokio::spawn(async move { mine(&client).await });
        while !fake
            .requests()
            .iter()
            .any(|r| r.method == "state_queryStorageAt")
        {
            tokio::task::yield_now().await;
        }
        assert!(!running.is_finished());
        fake.advance(1).unwrap();
        fake.mutate_state(|s| {
            s.seed_typed("SubtensorModule", "RegistrationsThisBlock", (NETUID,), 0u16)
                .unwrap()
        });
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(running.await.unwrap().unwrap().work_block, 101);
    }
}

// pyt: unit/test_pow_registration.py::test_miner_rechecks_solution_at_pool_age_boundary
#[tokio::test]
async fn pool_age_four_accepted_five_remined() {
    for advance in [4, 5] {
        let (client, fake) = fixture().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let miner: Miner = Arc::new(move |prefix, difficulty, start, attempts| {
            if count.fetch_add(1, Ordering::SeqCst) == 0 {
                fake.mutate_state(|s| {
                    s.advance(advance);
                });
            }
            rtao_core::pow::mine_cpu(prefix, difficulty, start, attempts)
        });
        let proof = solver(Duration::from_secs(10))
            .solve_with(&client, NETUID, HOT, COLD, &CancellationToken::new(), miner)
            .await
            .unwrap();
        assert_eq!(proof.work_block, if advance == 4 { 100 } else { 105 });
        assert_eq!(
            calls.load(Ordering::SeqCst),
            if advance == 4 { 1 } else { 2 }
        );
    }
}

#[tokio::test]
async fn canonical_reorg_and_head_rollback_remine() {
    for rollback in [false, true] {
        let (client, fake) = fixture().await;
        let original = client.at(BlockRef::Best).await.unwrap().block().hash;
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let miner: Miner = Arc::new(move |prefix, difficulty, start, attempts| {
            if count.fetch_add(1, Ordering::SeqCst) == 0 {
                fake.mutate_state(|s| {
                    let parent = s.block_hash(99).unwrap();
                    let best = if rollback {
                        parent
                    } else {
                        s.fork(&parent).unwrap()
                    };
                    s.set_best(&best).unwrap();
                });
            }
            rtao_core::pow::mine_cpu(prefix, difficulty, start, attempts)
        });
        let proof = solver(Duration::from_secs(10))
            .solve_with(&client, NETUID, HOT, COLD, &CancellationToken::new(), miner)
            .await
            .unwrap();
        assert_ne!(proof.block_hash, original);
        assert_eq!(proof.work_block, if rollback { 99 } else { 100 });
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test]
async fn latest_difficulty_and_team_floor_are_rechecked() {
    for item in ["Difficulty", "MinDifficulty"] {
        let (client, fake) = fixture().await;
        let difficulties = Arc::new(Mutex::new(Vec::new()));
        let observed = difficulties.clone();
        let miner: Miner = Arc::new(move |prefix, difficulty, start, attempts| {
            let mut observed = observed.lock().unwrap();
            observed.push(difficulty);
            if observed.len() == 1 {
                fake.mutate_state(|s| seed(s, item, 2));
                let nonce = (0..1024)
                    .find(|&nonce| {
                        rtao_core::pow::mine_cpu(prefix, 2, nonce, 1)
                            .unwrap()
                            .is_none()
                    })
                    .unwrap();
                Ok(Some((nonce, rtao_core::pow::seal(prefix, nonce).unwrap())))
            } else {
                rtao_core::pow::mine_cpu(prefix, difficulty, start, attempts)
            }
        });
        let proof = solver(Duration::from_secs(10))
            .solve_with(&client, NETUID, HOT, COLD, &CancellationToken::new(), miner)
            .await
            .unwrap();
        assert_eq!(*difficulties.lock().unwrap(), [1, 2]);
        let prefix = registration_prefix(NETUID, proof.block_hash, HOT, COLD).unwrap();
        assert_eq!(
            rtao_core::pow::mine_cpu(&prefix, 2, proof.nonce, 1).unwrap(),
            Some((proof.nonce, proof.work))
        );
    }
}

#[tokio::test]
async fn winning_proof_rechecks_admission_and_cpu_seal() {
    for corrupt in [false, true] {
        let (client, fake) = fixture().await;
        let miner: Miner = Arc::new(move |prefix, difficulty, start, attempts| {
            if corrupt {
                Ok(Some((0, [0; 32])))
            } else {
                fake.mutate_state(|s| {
                    s.seed_typed("SubtensorModule", "Uids", (NETUID, HOT), 0u16)
                        .unwrap()
                });
                rtao_core::pow::mine_cpu(prefix, difficulty, start, attempts)
            }
        });
        let result = solver(Duration::from_secs(10))
            .solve_with(&client, NETUID, HOT, COLD, &CancellationToken::new(), miner)
            .await;
        assert!(if corrupt {
            matches!(result, Err(PowError::InvalidProof))
        } else {
            matches!(result, Err(PowError::AlreadyRegistered))
        });
    }
}

// pyt: unit/test_pow_registration.py::test_long_gpu_search_refreshes_at_twelve_seconds_without_batch_head_polls
// CPU/fake-GPU pools share this search loop. Subscription-driven early refresh
// and the native final same-head read intentionally change Python's RPC count.
#[tokio::test(start_paused = true)]
async fn refreshes_after_twelve_seconds_without_polling_empty_batches() {
    let _clock = ManualClock::new();
    let (client, fake) = fixture().await;
    let first = client.at(BlockRef::Best).await.unwrap().block().hash;
    fake.mutate_state(|s| s.requests.clear());
    let (started, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let miner: Miner = Arc::new(move |prefix, difficulty, start, attempts| {
        let _ = started.send(prefix.to_vec());
        if prefix[27..59] == first.0 {
            Ok(None)
        } else {
            rtao_core::pow::mine_cpu(prefix, difficulty, start, attempts)
        }
    });
    let running = tokio::spawn(async move {
        solver(Duration::from_secs(60))
            .solve_with(&client, NETUID, HOT, COLD, &CancellationToken::new(), miner)
            .await
    });
    for _ in 0..3 {
        observed.recv().await.unwrap();
    }
    assert_eq!(
        fake.requests()
            .iter()
            .filter(|r| r.method == "chain_getBlockHash" && r.params.as_array().unwrap().is_empty())
            .count(),
        1
    );
    fake.advance(1).unwrap();
    tokio::time::advance(CHALLENGE_REFRESH).await;
    let proof = running.await.unwrap().unwrap();
    assert_eq!(proof.work_block, 101);
}

#[tokio::test]
async fn best_heads_refresh_before_five_block_age() {
    let (client, fake) = fixture().await;
    let first = client.at(BlockRef::Best).await.unwrap().block().hash;
    let (started, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let miner: Miner = Arc::new(move |prefix, difficulty, start, attempts| {
        let _ = started.send(());
        if prefix[27..59] == first.0 {
            Ok(None)
        } else {
            rtao_core::pow::mine_cpu(prefix, difficulty, start, attempts)
        }
    });
    let running = tokio::spawn(async move {
        solver(Duration::from_secs(10))
            .solve_with(&client, NETUID, HOT, COLD, &CancellationToken::new(), miner)
            .await
    });
    observed.recv().await.unwrap();
    fake.advance(4).unwrap();
    assert_eq!(running.await.unwrap().unwrap().work_block, 104);
}

#[tokio::test]
async fn timeout_cancel_and_dropped_future_join_running_workers() {
    for (mode, worker_count) in (0..3).flat_map(|mode| [1, 32].map(|workers| (mode, workers))) {
        let (client, _) = fixture().await;
        let active = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let activity = active.clone();
        let count = calls.clone();
        let (started, mut observed) = tokio::sync::mpsc::unbounded_channel();
        let miner: Miner = Arc::new(move |_, _, _, attempts| {
            assert_eq!(attempts, workers::ATTEMPTS);
            activity.fetch_add(1, Ordering::SeqCst);
            count.fetch_add(1, Ordering::SeqCst);
            let _ = started.send(());
            std::thread::sleep(Duration::from_millis(10));
            activity.fetch_sub(1, Ordering::SeqCst);
            Ok(None)
        });
        let cancellation = CancellationToken::new();
        let token = cancellation.clone();
        let running = tokio::spawn(async move {
            PowSolver::new(PowConfig {
                workers: worker_count,
                timeout: Duration::from_millis(if mode == 0 { 100 } else { 1000 }),
            })
            .unwrap()
            .solve_with(&client, NETUID, HOT, COLD, &token, miner)
            .await
        });
        observed.recv().await.unwrap();
        if mode == 1 {
            cancellation.cancel();
        } else if mode == 2 {
            running.abort();
        }
        let result = running.await;
        match mode {
            0 => assert!(matches!(result.unwrap(), Err(PowError::Timeout))),
            1 => assert!(matches!(result.unwrap(), Err(PowError::Cancelled))),
            _ => assert!(result.unwrap_err().is_cancelled()),
        }
        assert_eq!(active.load(Ordering::SeqCst), 0);
        let completed = calls.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(15)).await;
        assert_eq!(calls.load(Ordering::SeqCst), completed);
    }
}

#[tokio::test]
async fn success_joins_other_running_worker_batches() {
    let (client, _) = fixture().await;
    let active = Arc::new(AtomicUsize::new(0));
    let activity = active.clone();
    let miner: Miner = Arc::new(move |prefix, difficulty, start, attempts| {
        activity.fetch_add(1, Ordering::SeqCst);
        let result = if start == 0 {
            rtao_core::pow::mine_cpu(prefix, difficulty, start, attempts)
        } else {
            std::thread::sleep(Duration::from_millis(20));
            Ok(None)
        };
        activity.fetch_sub(1, Ordering::SeqCst);
        result
    });
    let solver = PowSolver::new(PowConfig {
        workers: 32,
        timeout: Duration::from_secs(10),
    })
    .unwrap();
    let proof = solver
        .solve_with(&client, NETUID, HOT, COLD, &CancellationToken::new(), miner)
        .await
        .unwrap();
    assert_eq!(proof.nonce, 0);
    assert_eq!(active.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn native_high_difficulty_timeout_and_pre_cancel() {
    let (client, fake) = fixture().await;
    fake.mutate_state(|s| seed(s, "Difficulty", u64::MAX));
    assert!(matches!(
        solver(Duration::from_millis(20))
            .solve(&client, NETUID, HOT, COLD, &CancellationToken::new())
            .await,
        Err(PowError::Timeout)
    ));
    let token = CancellationToken::new();
    token.cancel();
    let before = fake.requests().len();
    assert!(matches!(
        solver(Duration::from_secs(1))
            .solve(&client, NETUID, HOT, COLD, &token)
            .await,
        Err(PowError::Cancelled)
    ));
    assert_eq!(fake.requests().len(), before);
}

// pyt: unit/test_pow_registration.py::test_miner_does_not_return_solution_after_deadline
#[tokio::test]
async fn validation_cannot_return_after_timeout_or_cancellation() {
    for cancel in [false, true] {
        let (client, fake) = fixture().await;
        let token = CancellationToken::new();
        let cancellation = token.clone();
        fake.mutate_state(|state| {
            let mut reads = 0;
            state.set_handler("state_queryStorageAt", move |state, request| {
                reads += 1;
                if reads == 2 {
                    if cancel {
                        cancellation.cancel();
                    } else {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
                state.dispatch(request)
            });
        });
        let result = solver(Duration::from_millis(80))
            .solve(&client, NETUID, HOT, COLD, &token)
            .await;
        assert!(if cancel {
            matches!(result, Err(PowError::Cancelled))
        } else {
            matches!(result, Err(PowError::Timeout))
        });
    }
}

#[tokio::test]
async fn workers_are_bounded_and_wrap_disjoint_nonce_chunks() {
    let jobs = Arc::new(Mutex::new(Vec::new()));
    let recorded = jobs.clone();
    let miner: Miner = Arc::new(move |_, _, start, attempts| {
        recorded.lock().unwrap().push((start, attempts));
        Ok(None)
    });
    let mut pool = CpuWorkers::new(32, miner).unwrap();
    let prefix = Arc::new(registration_prefix(NETUID, H256([1; 32]), HOT, COLD).unwrap());
    let count = pool.dispatch(prefix, u64::MAX, u64::MAX).unwrap();
    assert!((1..=32).contains(&count));
    assert_eq!(
        usize::from(count),
        32usize.min(std::thread::available_parallelism().map_or(1, usize::from))
    );
    pool.drain(count).await.unwrap();
    drop(pool);
    let mut actual = jobs.lock().unwrap().clone();
    let mut expected: Vec<_> = (0..count)
        .map(|index| {
            (
                u64::MAX.wrapping_add(u64::from(index) * u64::from(workers::ATTEMPTS)),
                workers::ATTEMPTS,
            )
        })
        .collect();
    actual.sort_unstable();
    expected.sort_unstable();
    assert_eq!(actual, expected);
    const {
        assert!(workers::ATTEMPTS <= rtao_core::pow::MAX_BATCH);
    }
}

#[cfg(feature = "pow-gpu")]
mod gpu_tests {
    #![allow(clippy::panic)]

    use super::*;
    use crate::pow::gpu;

    struct FakeGpu {
        devices: usize,
        miner: Miner,
        dropped: Arc<AtomicUsize>,
    }

    impl gpu::Backend for FakeGpu {
        fn device_count(&self) -> usize {
            self.devices
        }

        fn mine(
            &mut self,
            prefix: &[u8],
            difficulty: u64,
            start: u64,
            attempts: u32,
        ) -> workers::Solution {
            (self.miner)(prefix, difficulty, start, attempts)
        }
    }

    impl Drop for FakeGpu {
        fn drop(&mut self) {
            self.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn factory(devices: usize, miner: Miner, dropped: Arc<AtomicUsize>) -> gpu::Factory {
        Arc::new(move |_| {
            Ok(Some(Box::new(FakeGpu {
                devices,
                miner: miner.clone(),
                dropped: dropped.clone(),
            })))
        })
    }

    async fn solve_gpu(
        client: &Client,
        options: GpuOptions,
        factory: gpu::Factory,
    ) -> Result<PowProof, PowError> {
        solver(Duration::from_secs(10))
            .solve_with_workers(
                client,
                NETUID,
                HOT,
                COLD,
                &CancellationToken::new(),
                WorkerFactory::Gpu { options, factory },
            )
            .await
    }

    // pyt: unit/test_pow_registration.py::test_miner_rejects_duplicate_or_cpu_device_selection
    // pyt: unit/test_pow_registration.py::test_auto_discovers_all_gpus_and_explicit_selection_is_forwarded
    // Device IDs are usize, so a negative ID cannot enter this typed API.
    // None selects every discovered GPU in the pinned core; the factory test
    // below separately observes forwarding of an explicit device list.
    #[test]
    fn gpu_configuration_rejects_invalid_selections_and_unbounded_batches() {
        for (backend, ids, attempts) in [
            (PowBackend::Cpu, Some(vec![0]), 1),
            (PowBackend::Auto, Some(vec![]), 1),
            (PowBackend::Gpu, Some(vec![1, 1]), 1),
            (PowBackend::Gpu, Some((0..33).collect()), 1),
            (PowBackend::Auto, None, 0),
            (PowBackend::Auto, None, rtao_core::pow::MAX_BATCH + 1),
        ] {
            assert!(
                GpuOptions {
                    backend,
                    device_ids: ids,
                    attempts_per_device: attempts,
                }
                .validate()
                .is_err()
            );
        }
        let defaults = GpuOptions::default();
        assert_eq!(defaults.backend, PowBackend::Auto);
        assert_eq!(defaults.device_ids, None);
        assert!(defaults.validate().is_ok());
        assert!(
            GpuOptions {
                backend: PowBackend::Gpu,
                device_ids: Some(vec![0, 31]),
                attempts_per_device: rtao_core::pow::MAX_BATCH,
            }
            .validate()
            .is_ok()
        );
    }

    // pyt: unit/test_pow_registration.py::test_explicit_gpu_requires_hardware_and_cpu_skips_discovery
    #[test]
    fn automatic_fallback_does_not_hide_explicit_selection_or_driver_failure() {
        let options = GpuOptions::default();
        assert!(
            gpu::select_backend(&options, || Ok(false), || panic!("no device to open"))
                .unwrap()
                .is_none()
        );
        for options in [
            GpuOptions {
                backend: PowBackend::Gpu,
                ..GpuOptions::default()
            },
            GpuOptions {
                device_ids: Some(vec![0]),
                ..GpuOptions::default()
            },
        ] {
            assert!(
                gpu::select_backend(&options, || Ok(false), || panic!("no device to open"))
                    .err()
                    .unwrap()
                    .contains("no usable OpenCL GPUs")
            );
        }
        assert_eq!(
            gpu::select_backend(
                &options,
                || Ok(true),
                || Err("startup hash self-test failed".into()),
            )
            .err()
            .unwrap(),
            "startup hash self-test failed"
        );
        assert_eq!(
            gpu::select_backend(
                &options,
                || Err("device enumeration failed".into()),
                || panic!("discovery failed"),
            )
            .err()
            .unwrap(),
            "device enumeration failed"
        );
        let cpu = GpuOptions {
            backend: PowBackend::Cpu,
            ..GpuOptions::default()
        };
        assert!(
            gpu::select_backend(
                &cpu,
                || panic!("CPU must not discover devices"),
                || panic!("CPU must not load a driver"),
            )
            .unwrap()
            .is_none()
        );
    }

    // pyt: unit/test_pow_registration.py::test_gpu_miner_preserves_device_selection_and_pow_only_admission
    // Proof output excludes Python intent mining settings; the explicit options
    // reach the selected factory, while the CLI retains its constructor fields.
    #[tokio::test]
    async fn gpu_worker_initializes_and_mines_off_runtime_with_exact_public_challenge() {
        let (client, fake) = fixture().await;
        let runtime_thread = std::thread::current().id();
        let observed = Arc::new(Mutex::new(Vec::new()));
        let jobs = observed.clone();
        let miner: Miner = Arc::new(move |prefix, difficulty, start, attempts| {
            assert_ne!(std::thread::current().id(), runtime_thread);
            jobs.lock()
                .unwrap()
                .push((prefix.to_vec(), difficulty, start, attempts));
            rtao_core::pow::mine_cpu(prefix, difficulty, start, 1)
        });
        let dropped = Arc::new(AtomicUsize::new(0));
        let create = factory(2, miner, dropped.clone());
        let checked: gpu::Factory = Arc::new(move |options| {
            assert_ne!(std::thread::current().id(), runtime_thread);
            assert_eq!(options.device_ids, Some(vec![1, 3]));
            create(options)
        });
        let options = GpuOptions {
            backend: PowBackend::Gpu,
            device_ids: Some(vec![1, 3]),
            attempts_per_device: 7,
        };
        let proof = solve_gpu(&client, options, checked).await.unwrap();
        let prefix = registration_prefix(NETUID, proof.block_hash, HOT, COLD).unwrap();
        assert_eq!(*observed.lock().unwrap(), [(prefix.to_vec(), 1, 0, 7)]);
        assert_eq!(proof.nonce, 0);
        assert_eq!(proof.work, rtao_core::pow::seal(&prefix, 0).unwrap());
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        assert!(
            fake.requests()
                .iter()
                .all(|r| !r.method.starts_with("author_"))
        );
    }

    #[tokio::test]
    async fn gpu_nonce_ranges_cover_every_device_and_wrap_without_overlap() {
        let jobs = Arc::new(Mutex::new(Vec::new()));
        let observed = jobs.clone();
        let dropped = Arc::new(AtomicUsize::new(0));
        let miner: Miner = Arc::new(move |_, _, start, attempts| {
            observed.lock().unwrap().push((start, attempts));
            Ok(None)
        });
        let options = GpuOptions {
            attempts_per_device: 7,
            ..GpuOptions::default()
        };
        let mut worker = gpu::GpuWorker::start(options, factory(3, miner, dropped.clone()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(worker.nonce_span(), 21);
        let prefix = Arc::new(registration_prefix(NETUID, H256([1; 32]), HOT, COLD).unwrap());
        let start = u64::MAX - 5;
        assert_eq!(worker.dispatch(prefix.clone(), 1, start).unwrap(), 1);
        assert!(worker.result().await.unwrap().unwrap().is_none());
        worker
            .dispatch(prefix, 1, start.wrapping_add(worker.nonce_span()))
            .unwrap();
        assert!(worker.result().await.unwrap().unwrap().is_none());
        drop(worker);
        assert_eq!(*jobs.lock().unwrap(), [(u64::MAX - 5, 7), (15, 7)]);
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }

    // pyt: unit/test_pow_registration.py::test_gpu_driver_cannot_return_a_corrupt_proof
    #[tokio::test]
    async fn gpu_proofs_still_reject_wrong_seals_and_out_of_range_nonces() {
        for mode in 0..2 {
            let (client, _) = fixture().await;
            let miner: Miner = Arc::new(move |prefix, _, start, _| {
                let nonce = if mode == 1 {
                    start.wrapping_add(7)
                } else {
                    start
                };
                let work = if mode == 0 {
                    [0; 32]
                } else {
                    rtao_core::pow::seal(prefix, nonce).unwrap()
                };
                Ok(Some((nonce, work)))
            });
            let dropped = Arc::new(AtomicUsize::new(0));
            let options = GpuOptions {
                backend: PowBackend::Gpu,
                attempts_per_device: 7,
                ..GpuOptions::default()
            };
            let result = solve_gpu(&client, options, factory(1, miner, dropped.clone())).await;
            assert!(if mode == 0 {
                matches!(result, Err(PowError::InvalidProof))
            } else {
                matches!(&result, Err(PowError::Core(message)) if message.contains("assigned range"))
            });
            assert_eq!(dropped.load(Ordering::SeqCst), 1);
        }
    }

    // pyt: unit/test_pow_registration.py::test_gpu_solution_is_remined_when_difficulty_increases
    #[tokio::test]
    async fn gpu_winners_recheck_difficulty_and_canonical_hash() {
        for reorg in [false, true] {
            let (client, fake) = fixture().await;
            let observed = Arc::new(Mutex::new(Vec::new()));
            let jobs = observed.clone();
            let miner: Miner = Arc::new(move |prefix, difficulty, start, attempts| {
                let mut jobs = jobs.lock().unwrap();
                jobs.push(difficulty);
                if jobs.len() == 1 {
                    if reorg {
                        fake.mutate_state(|state| {
                            let parent = state.block_hash(99).unwrap();
                            let fork = state.fork(&parent).unwrap();
                            state.set_best(&fork).unwrap();
                        });
                    } else {
                        fake.mutate_state(|state| seed(state, "Difficulty", 2));
                        let nonce = (0..attempts)
                            .map(|offset| start.wrapping_add(u64::from(offset)))
                            .find(|&nonce| {
                                rtao_core::pow::mine_cpu(prefix, 2, nonce, 1)
                                    .unwrap()
                                    .is_none()
                            })
                            .unwrap();
                        return Ok(Some((nonce, rtao_core::pow::seal(prefix, nonce).unwrap())));
                    }
                }
                rtao_core::pow::mine_cpu(prefix, difficulty, start, attempts)
            });
            let dropped = Arc::new(AtomicUsize::new(0));
            let original = client.at(BlockRef::Best).await.unwrap().block().hash;
            let proof = solve_gpu(
                &client,
                GpuOptions::default(),
                factory(1, miner, dropped.clone()),
            )
            .await
            .unwrap();
            assert_eq!(
                *observed.lock().unwrap(),
                if reorg { [1, 1] } else { [1, 2] }
            );
            assert_eq!(proof.block_hash != original, reorg);
            let prefix = registration_prefix(NETUID, proof.block_hash, HOT, COLD).unwrap();
            assert_eq!(
                rtao_core::pow::mine_cpu(&prefix, if reorg { 1 } else { 2 }, proof.nonce, 1)
                    .unwrap(),
                Some((proof.nonce, proof.work))
            );
            assert_eq!(dropped.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn gpu_timeout_cancel_and_future_drop_join_current_batch_and_release_device() {
        let _clock = ManualClock::new();
        for mode in 0..3 {
            let (client, _) = fixture().await;
            let dropped = Arc::new(AtomicUsize::new(0));
            let active = Arc::new(AtomicUsize::new(0));
            let activity = active.clone();
            let (started, mut observed) = tokio::sync::mpsc::unbounded_channel();
            let miner: Miner = Arc::new(move |_, _, _, _| {
                activity.fetch_add(1, Ordering::SeqCst);
                let _ = started.send(());
                std::thread::sleep(Duration::from_millis(10));
                activity.fetch_sub(1, Ordering::SeqCst);
                Ok(None)
            });
            let token = CancellationToken::new();
            let cancel = token.clone();
            let create = factory(1, miner, dropped.clone());
            let running = tokio::spawn(async move {
                solver(Duration::from_secs(60))
                    .solve_with_workers(
                        &client,
                        NETUID,
                        HOT,
                        COLD,
                        &token,
                        WorkerFactory::Gpu {
                            options: GpuOptions::default(),
                            factory: create,
                        },
                    )
                    .await
            });
            observed.recv().await.unwrap();
            if mode == 0 {
                tokio::time::advance(Duration::from_secs(60)).await;
            } else if mode == 1 {
                cancel.cancel();
            } else if mode == 2 {
                running.abort();
            }
            let result = running.await;
            match mode {
                0 => assert!(matches!(result.unwrap(), Err(PowError::Timeout))),
                1 => assert!(matches!(result.unwrap(), Err(PowError::Cancelled))),
                _ => assert!(result.unwrap_err().is_cancelled()),
            }
            assert_eq!(active.load(Ordering::SeqCst), 0);
            assert_eq!(dropped.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn gpu_initialization_is_lazy_and_cancelled_initialization_is_joined() {
        let (client, fake) = fixture().await;
        fake.mutate_state(|state| {
            state
                .seed_typed(
                    "SubtensorModule",
                    "NetworkPowRegistrationAllowed",
                    (NETUID,),
                    false,
                )
                .unwrap();
        });
        let never: gpu::Factory = Arc::new(|_| panic!("admission must run before GPU setup"));
        assert!(matches!(
            solve_gpu(&client, GpuOptions::default(), never).await,
            Err(PowError::Disabled)
        ));
        let (client, _) = fixture().await;
        let dropped = Arc::new(AtomicUsize::new(0));
        let release = dropped.clone();
        let (started, mut observed) = tokio::sync::mpsc::unbounded_channel();
        let (continue_setup, gate) = std::sync::mpsc::channel();
        let gate = Mutex::new(gate);
        let create: gpu::Factory = Arc::new(move |_| {
            let _ = started.send(());
            gate.lock().unwrap().recv().unwrap();
            Ok(Some(Box::new(FakeGpu {
                devices: 1,
                miner: Arc::new(|_, _, _, _| panic!("cancelled setup must not mine")),
                dropped: release.clone(),
            })))
        });
        let running =
            tokio::spawn(async move { solve_gpu(&client, GpuOptions::default(), create).await });
        observed.recv().await.unwrap();
        running.abort();
        continue_setup.send(()).unwrap();
        assert!(running.await.unwrap_err().is_cancelled());
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn auto_fallback_solves_on_cpu_and_setup_failure_remains_visible() {
        let (client, _) = fixture().await;
        let no_devices: gpu::Factory = Arc::new(|_| Ok(None));
        assert!(
            solve_gpu(&client, GpuOptions::default(), no_devices)
                .await
                .is_ok()
        );
        let failed: gpu::Factory = Arc::new(|_| Err("GPU startup failed".into()));
        assert!(matches!(
            solve_gpu(&client, GpuOptions::default(), failed).await,
            Err(PowError::Core(message)) if message == "GPU startup failed"
        ));
        for devices in [0, 33] {
            let dropped = Arc::new(AtomicUsize::new(0));
            let never: Miner = Arc::new(|_, _, _, _| panic!("invalid device count must not mine"));
            let result = gpu::GpuWorker::start(
                GpuOptions::default(),
                factory(devices, never, dropped.clone()),
            )
            .await;
            assert!(matches!(
                result,
                Err(PowError::Core(message)) if message == "GPU miner must select 1..=32 devices"
            ));
            assert_eq!(dropped.load(Ordering::SeqCst), 1);
        }
    }
}

#[tokio::test]
async fn head_changes_during_validation_cannot_return_obsolete_state() {
    for advance in [1, 5] {
        let (client, fake) = fixture().await;
        fake.mutate_state(|state| {
            let mut reads = 0;
            state.set_handler("state_queryStorageAt", move |state, request| {
                reads += 1;
                let reply = state.dispatch(request);
                if reads == 2 {
                    state.advance(advance);
                    seed(state, "Difficulty", 2);
                }
                reply
            });
        });
        let proof = mine(&client).await.unwrap();
        assert_eq!(proof.work_block, 100 + advance);
        let prefix = registration_prefix(NETUID, proof.block_hash, HOT, COLD).unwrap();
        assert_eq!(
            rtao_core::pow::mine_cpu(&prefix, 2, proof.nonce, 1).unwrap(),
            Some((proof.nonce, proof.work))
        );
    }
}
