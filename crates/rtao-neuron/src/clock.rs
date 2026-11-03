//! Injectable wall clocks, bounded skew estimation and retained epoch status.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;

/// Unix clock used to choose a timelock reveal round.
pub trait Clock: Send + Sync {
    /// Return Unix seconds, including fractional chain slots.
    fn now_unix(&self) -> f64;
    /// Whether this clock depends on the host's wall clock.
    fn is_system(&self) -> bool {
        false
    }
    /// Observe a verified best-head timestamp; wall clocks keep their own source.
    fn observe_timestamp(&self, _timestamp_ms: u64) {}
    /// Observe a verified timestamp of a block this process had already
    /// learned of at `received`. Elapsed time counts from `received`, so the
    /// time spent reading the timestamp is not lost.
    ///
    /// The default ignores `received` and calls [`Self::observe_timestamp`],
    /// which counts from the call: the clock then lags by up to that read's
    /// duration, and never runs ahead.
    fn observe_timestamp_at(&self, timestamp_ms: u64, received: Instant) {
        let _ = received;
        self.observe_timestamp(timestamp_ms);
    }
}

/// Host wall clock, guarded against excessive skew before commits.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;
impl Clock for SystemClock {
    fn now_unix(&self) -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64()
    }
    fn is_system(&self) -> bool {
        true
    }
}

/// Chain timestamp advanced by monotonic elapsed time since its notification.
///
/// Who anchors it: `NeuronRuntime`'s clock worker observes every best head.
/// A `WeightScheduler` driven by the application's own loop anchors
/// it too: before each timelocked commit it reads `Timestamp.Now` at the
/// best block its preflight is pinned to and moves the clock forward to that
/// reading when the clock is unanchored or behind it (D-094). Either way the
/// reading is a lower bound of chain time, since a block is always learned
/// after its timestamp.
#[derive(Clone, Debug)]
pub struct ChainClock(Arc<Mutex<(f64, Instant)>>);
impl ChainClock {
    /// A clock with no anchor yet. [`ClockGuard::check`] refuses a commit
    /// until the clock worker or the weight scheduler observes a verified
    /// timestamp.
    pub fn unanchored() -> Self {
        Self(Arc::new(Mutex::new((f64::NAN, Instant::now()))))
    }
    /// Anchor the clock to a verified timestamp in milliseconds.
    pub fn new(timestamp_ms: u64) -> Self {
        Self(Arc::new(Mutex::new((
            timestamp_ms as f64 / 1000.0,
            Instant::now(),
        ))))
    }
    /// Replace the anchor after receiving another best head.
    pub fn observe(&self, timestamp_ms: u64) {
        self.observe_at(timestamp_ms, Instant::now());
    }
    /// Replace the anchor with the timestamp of a block learned at `received`.
    pub fn observe_at(&self, timestamp_ms: u64, received: Instant) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) =
            (timestamp_ms as f64 / 1000.0, received);
    }
    /// Whether a verified timestamp anchors this clock.
    pub fn is_anchored(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .0
            .is_finite()
    }
}
impl Clock for ChainClock {
    fn now_unix(&self) -> f64 {
        let (timestamp, received) = *self.0.lock().unwrap_or_else(|p| p.into_inner());
        timestamp + received.elapsed().as_secs_f64()
    }
    fn observe_timestamp(&self, timestamp_ms: u64) {
        self.observe(timestamp_ms);
    }
    fn observe_timestamp_at(&self, timestamp_ms: u64, received: Instant) {
        self.observe_at(timestamp_ms, received);
    }
}

/// When a block was pinned: the monotonic instant and the host's Unix time
/// at that instant, taken right after the pinning read returned.
#[cfg(feature = "validator")]
#[derive(Clone, Copy, Debug)]
pub(crate) struct Pinned {
    pub(crate) instant: Instant,
    pub(crate) system: f64,
}
#[cfg(feature = "validator")]
impl Pinned {
    pub(crate) fn now() -> Self {
        Self {
            instant: Instant::now(),
            system: SystemClock.now_unix(),
        }
    }
}

/// Observe a verified timestamp the weight scheduler read itself (D-094).
///
/// The clock moves only forward: it is anchored when it has no anchor or
/// reads earlier than this block's timestamp advanced from `pinned`, so a
/// clock worker's fresher anchor is kept. Wall clocks keep their own source.
/// The guard records the offset at the pinning instant; its minimum filter
/// absorbs the block's age, as it does for propagation delay. Returns
/// whether the clock took this anchor.
#[cfg(feature = "validator")]
pub(crate) fn observe_chain_time(
    clock: &dyn Clock,
    guard: &ClockGuard,
    timestamp_ms: u64,
    pinned: Pinned,
) -> bool {
    if let Err(error) = guard.observe(pinned.system, timestamp_ms) {
        tracing::warn!(%error, "host clock reading is unusable; skew estimate not updated");
    }
    if clock.is_system() {
        return false;
    }
    let observed = timestamp_ms as f64 / 1000.0 + pinned.instant.elapsed().as_secs_f64();
    let now = clock.now_unix();
    let behind = !(now.is_finite() && now >= observed);
    if behind {
        clock.observe_timestamp_at(timestamp_ms, pinned.instant);
    }
    behind
}

/// Clock health derived from the minimum offset of the last twenty heads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClockHealth {
    /// Estimated host-minus-chain offset, in seconds.
    pub skew_seconds: f64,
    /// Absolute offset exceeds btauth's two-second tolerance.
    pub warning: bool,
    /// A system-clock commitment must be refused above twelve seconds.
    pub refuse_system_commit: bool,
}

/// Propagation-delay-resistant clock check using a fixed twenty-head window.
#[derive(Clone, Debug, Default)]
pub struct ClockGuard(Arc<Mutex<VecDeque<f64>>>);
impl ClockGuard {
    /// Record a best-head timestamp against the current system time.
    pub fn observe(&self, system_now: f64, timestamp_ms: u64) -> anyhow::Result<ClockHealth> {
        anyhow::ensure!(
            system_now.is_finite() && system_now >= 0.0,
            "invalid host clock reading"
        );
        let mut offsets = self.0.lock().unwrap_or_else(|p| p.into_inner());
        offsets.push_back(system_now - timestamp_ms as f64 / 1000.0);
        if offsets.len() > 20 {
            offsets.pop_front();
        }
        Ok(health(&offsets))
    }
    /// Return the latest estimate, or no estimate before the first head.
    pub fn health(&self) -> Option<ClockHealth> {
        let offsets = self.0.lock().unwrap_or_else(|p| p.into_inner());
        (!offsets.is_empty()).then(|| health(&offsets))
    }
    /// Refuse an unsafe system-clock commit; chain clocks avoid this dependency.
    pub fn check(&self, clock: &dyn Clock) -> anyhow::Result<()> {
        let now = clock.now_unix();
        anyhow::ensure!(
            now.is_finite() && now >= 0.0,
            "timelock clock has no verified time anchor"
        );
        if clock.is_system() {
            let health = self.health().ok_or_else(|| {
                anyhow::anyhow!("chain timestamp unavailable; cannot check host clock")
            })?;
            anyhow::ensure!(
                !health.refuse_system_commit,
                "host clock skew is {:.3}s; synchronize the clock or select ChainClock",
                health.skew_seconds
            );
        }
        Ok(())
    }
}
fn health(offsets: &VecDeque<f64>) -> ClockHealth {
    let skew_seconds = offsets.iter().copied().fold(f64::INFINITY, f64::min);
    ClockHealth {
        skew_seconds,
        warning: skew_seconds.abs() > 2.0,
        refuse_system_commit: skew_seconds.abs() > 12.0,
    }
}

/// Retained state of the subnet's actual finalized epoch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EpochStatus {
    /// Latest observed finalized block.
    pub block: u64,
    /// Pallet epoch index at that block.
    pub index: u64,
    /// Last actual epoch boundary.
    pub last_epoch_block: u64,
    /// Configured subnet tempo in blocks.
    pub tempo: u16,
}
impl EpochStatus {
    /// Blocks elapsed since the actual epoch boundary.
    pub fn elapsed(self) -> u64 {
        self.block.saturating_sub(self.last_epoch_block)
    }
    /// Nominal epoch duration; chain events may defer or trigger it.
    pub fn nominal_duration(self, block_time: Duration) -> Duration {
        block_time.saturating_mul(u32::from(self.tempo))
    }
}

/// Cloneable epoch watch used by scoring and scheduling services.
#[derive(Clone, Debug)]
pub struct EpochClock {
    tx: watch::Sender<EpochStatus>,
}
impl EpochClock {
    /// Create a retained watch from one coherent snapshot.
    pub fn new(initial: EpochStatus) -> Self {
        Self {
            tx: watch::channel(initial).0,
        }
    }
    /// Current observed state.
    pub fn status(&self) -> EpochStatus {
        *self.tx.borrow()
    }
    /// Subscribe to actual epoch and finalized block changes.
    pub fn subscribe(&self) -> watch::Receiver<EpochStatus> {
        self.tx.subscribe()
    }
    /// Publish coherent finalized state. The runtime publishes the newest
    /// finalized head it reads; heads that arrived while it was busy
    /// coalesce, as they would for any watch subscriber.
    pub fn observe(&self, status: EpochStatus) {
        self.tx.send_replace(status);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn skew_window_filters_delay_and_refuses_both_signs() -> anyhow::Result<()> {
        let guard = ClockGuard::default();
        guard.observe(1000.0, 999_000)?;
        assert!(!guard.observe(1004.0, 1_000_000)?.warning);
        for _ in 0..20 {
            guard.observe(1020.0, 1_000_000)?;
        }
        assert!(guard.check(&SystemClock).is_err());
        assert!(guard.check(&ChainClock::new(1_000_000)).is_ok());
        let early = ClockGuard::default();
        assert!(early.observe(985.0, 1_000_000)?.refuse_system_commit);
        Ok(())
    }
    #[test]
    fn timestamp_required_and_nonfinite_rejected() {
        let guard = ClockGuard::default();
        assert!(guard.check(&SystemClock).is_err());
        assert!(guard.observe(f64::NAN, 0).is_err());
        assert!(guard.observe(-1.0, 0).is_err());
        let chain = ChainClock::unanchored();
        assert!(guard.check(&chain).is_err());
        chain.observe_timestamp(1_000_000);
        assert!(guard.check(&chain).is_ok());
        assert!((1_000.0..1_001.0).contains(&chain.now_unix()));
    }
    #[test]
    fn observe_at_counts_elapsed_time_from_the_pinning_instant() -> anyhow::Result<()> {
        let chain = ChainClock::unanchored();
        assert!(!chain.is_anchored());
        let pinned = Instant::now()
            .checked_sub(Duration::from_secs(5))
            .ok_or_else(|| anyhow::anyhow!("monotonic clock too young"))?;
        chain.observe_timestamp_at(1_000_000, pinned);
        assert!(chain.is_anchored());
        assert!((1_005.0..1_006.0).contains(&chain.now_unix()));
        Ok(())
    }

    /// A clock that records what it observed through the default method.
    struct Recorded(Mutex<Vec<u64>>);
    impl Clock for Recorded {
        fn now_unix(&self) -> f64 {
            f64::NAN
        }
        fn observe_timestamp(&self, timestamp_ms: u64) {
            self.0
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(timestamp_ms);
        }
    }

    #[test]
    fn default_observe_at_falls_back_to_observe_timestamp() {
        let clock = Recorded(Mutex::new(Vec::new()));
        clock.observe_timestamp_at(42, Instant::now());
        assert_eq!(*clock.0.lock().unwrap_or_else(|p| p.into_inner()), [42]);
    }

    #[cfg(feature = "validator")]
    #[test]
    fn scheduler_observations_only_move_a_chain_clock_forward() -> anyhow::Result<()> {
        let guard = ClockGuard::default();
        let chain = ChainClock::unanchored();
        let pinned = |seconds_ago: u64, system: f64| -> anyhow::Result<Pinned> {
            Ok(Pinned {
                instant: Instant::now()
                    .checked_sub(Duration::from_secs(seconds_ago))
                    .ok_or_else(|| anyhow::anyhow!("monotonic clock too young"))?,
                system,
            })
        };
        // An unanchored clock takes the first verified timestamp.
        assert!(observe_chain_time(
            &chain,
            &guard,
            1_000_000,
            pinned(2, 1_003.0)?
        ));
        assert!((1_002.0..1_003.0).contains(&chain.now_unix()));
        // The guard records the offset at the pinning instant.
        let health = guard.health().ok_or_else(|| anyhow::anyhow!("no sample"))?;
        assert!((health.skew_seconds - 3.0).abs() < 1e-9);
        // An older block never moves the clock back.
        assert!(!observe_chain_time(
            &chain,
            &guard,
            990_000,
            pinned(0, 1_003.0)?
        ));
        assert!((1_002.0..1_003.0).contains(&chain.now_unix()));
        // A block the clock reads earlier than moves it forward.
        assert!(observe_chain_time(
            &chain,
            &guard,
            1_010_000,
            pinned(0, 1_010.5)?
        ));
        assert!((1_010.0..1_011.0).contains(&chain.now_unix()));
        // A wall clock keeps its own source; its guard still learns the skew.
        let system_guard = ClockGuard::default();
        assert!(!observe_chain_time(
            &SystemClock,
            &system_guard,
            (SystemClock.now_unix() * 1000.0) as u64,
            Pinned::now()
        ));
        system_guard.check(&SystemClock)?;
        // An unusable host reading only skips the skew sample.
        let skipped = ClockGuard::default();
        let fresh = ChainClock::unanchored();
        assert!(observe_chain_time(
            &fresh,
            &skipped,
            1_000_000,
            pinned(0, f64::NAN)?
        ));
        assert!(skipped.health().is_none());
        assert!(fresh.is_anchored());
        Ok(())
    }

    #[tokio::test]
    async fn epoch_watch_retains_last_observation_without_subscribers() -> anyhow::Result<()> {
        let clock = EpochClock::new(EpochStatus::default());
        clock.observe(EpochStatus {
            block: 101,
            index: 3,
            last_epoch_block: 100,
            tempo: 20,
        });
        let mut watch = clock.subscribe();
        assert_eq!(watch.borrow_and_update().index, 3);
        clock.observe(EpochStatus {
            block: 102,
            ..clock.status()
        });
        watch.changed().await?;
        assert_eq!(watch.borrow().elapsed(), 2);
        Ok(())
    }
}
