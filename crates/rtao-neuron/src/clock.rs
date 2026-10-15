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
#[derive(Clone, Debug)]
pub struct ChainClock(Arc<Mutex<(f64, Instant)>>);
impl ChainClock {
    /// Wait for the runtime's first verified timestamp before preparing a commit.
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
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) =
            (timestamp_ms as f64 / 1000.0, Instant::now());
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
    /// Publish coherent finalized state, including intermediate gap-recovered heads.
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
