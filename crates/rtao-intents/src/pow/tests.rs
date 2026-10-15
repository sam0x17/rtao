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
