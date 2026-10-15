//! User callback supervision with cancellation, panic isolation and backoff.

#[cfg(any(feature = "validator", test))]
use crate::telemetry::Metrics;
#[cfg(any(feature = "validator", test))]
use std::future::Future;
use std::time::Duration;
#[cfg(any(feature = "validator", test))]
use tokio::{sync::watch, time::Instant};
#[cfg(any(feature = "validator", test))]
use tokio_util::sync::CancellationToken;

/// How often the framework invokes a validator's scoring callback.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StepCadence {
    /// Once for each advancing finalized head; the default avoids RPC busy loops.
    #[default]
    EveryFinalizedBlock,
    /// Repeated at a wall-time interval, skipping missed ticks.
    Interval(Duration),
    /// Start the next step only after this minimum interval following completion.
    Continuous {
        /// Minimum delay between completed scoring attempts.
        min_interval: Duration,
    },
}

/// Exponential error delay, reset after a successful scoring step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Backoff {
    /// Delay after the first error, panic or timeout.
    pub initial: Duration,
    /// Maximum delay after consecutive errors.
    pub maximum: Duration,
}
impl Default for Backoff {
    fn default() -> Self {
        Self {
            initial: Duration::from_secs(1),
            maximum: Duration::from_secs(60),
        }
    }
}
impl Backoff {
    /// Delay after the given number of consecutive failures.
    pub fn delay(self, failures: u32) -> Duration {
        let shift = failures.saturating_sub(1).min(31);
        self.initial.saturating_mul(1u32 << shift).min(self.maximum)
    }
}

/// Timing and failure policy for validator scoring callbacks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StepPolicy {
    /// Callback cadence.
    pub cadence: StepCadence,
    /// Cancel an overrunning callback; `None` leaves its duration unbounded.
    pub timeout: Option<Duration>,
    /// Delay between unsuccessful callback attempts.
    pub backoff: Backoff,
}
impl StepPolicy {
    /// Reject configurations that would create busy loops or immediate timeouts.
    pub fn validate(self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.backoff.initial.is_zero() && self.backoff.maximum >= self.backoff.initial,
            "step backoff must have positive initial delay and maximum >= initial"
        );
        anyhow::ensure!(
            self.timeout.is_none_or(|timeout| !timeout.is_zero()),
            "step timeout must be positive"
        );
        match self.cadence {
            StepCadence::Interval(duration)
            | StepCadence::Continuous {
                min_interval: duration,
            } => anyhow::ensure!(!duration.is_zero(), "step cadence must be positive"),
            StepCadence::EveryFinalizedBlock => {}
        }
        Ok(())
    }
}

#[cfg(any(feature = "validator", test))]
pub(crate) async fn supervise<F, Fut>(
    mut invoke: F,
    mut finalized: watch::Receiver<u64>,
    policy: StepPolicy,
    metrics: Metrics,
    cancel: CancellationToken,
) -> anyhow::Result<()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    policy.validate()?;
    let mut failures = 0u32;
    let mut next_interval = Instant::now();
    loop {
        let cadence = async {
            match policy.cadence {
                StepCadence::EveryFinalizedBlock => {
                    finalized
                        .changed()
                        .await
                        .map_err(|_| anyhow::anyhow!("finalized-head watch ended"))?;
                }
                StepCadence::Interval(_) | StepCadence::Continuous { .. } => {
                    tokio::time::sleep_until(next_interval).await;
                }
            }
            Ok::<_, anyhow::Error>(())
        };
        tokio::select! { biased; () = cancel.cancelled() => return Ok(()), ready = cadence => ready? }
        let started = Instant::now();
        let mut task = tokio::spawn(invoke());
        let result = {
            let completion = async {
                if let Some(timeout) = policy.timeout {
                    match tokio::time::timeout(timeout, &mut task).await {
                        Ok(result) => result.map_err(anyhow::Error::from)?,
                        Err(_) => {
                            task.abort();
                            let _ = (&mut task).await;
                            Err(anyhow::anyhow!("validator step exceeded its timeout"))
                        }
                    }
                } else {
                    (&mut task).await.map_err(anyhow::Error::from)?
                }
            };
            tokio::select! {
                biased;
                () = cancel.cancelled() => None,
                result = completion => Some(result),
            }
        };
        let Some(result) = result else {
            task.abort();
            let _ = task.await;
            return Ok(());
        };
        match result {
            Ok(()) => {
                failures = 0;
                metrics.update_readiness(|ready| ready.consecutive_step_failures = 0);
            }
            Err(error) => {
                failures = failures.saturating_add(1);
                metrics.increment("rtao_step_errors_total");
                metrics.update_readiness(|ready| ready.consecutive_step_failures = failures);
                tracing::warn!(%error, failures, "validator scoring step failed");
                tokio::select! { biased; () = cancel.cancelled() => return Ok(()), () = tokio::time::sleep(policy.backoff.delay(failures)) => {} }
            }
        }
        next_interval = match policy.cadence {
            StepCadence::Interval(interval) => (started + interval).max(Instant::now()),
            StepCadence::Continuous { min_interval } => Instant::now() + min_interval,
            StepCadence::EveryFinalizedBlock => Instant::now(),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    #[test]
    fn backoff_saturates_and_invalid_cadences_are_rejected() {
        let backoff = Backoff::default();
        assert_eq!(backoff.delay(1), Duration::from_secs(1));
        assert_eq!(backoff.delay(6), Duration::from_secs(32));
        assert_eq!(backoff.delay(u32::MAX), Duration::from_secs(60));
        assert!(
            StepPolicy {
                cadence: StepCadence::Interval(Duration::ZERO),
                ..StepPolicy::default()
            }
            .validate()
            .is_err()
        );
    }
    #[tokio::test]
    async fn errors_panics_and_timeouts_do_not_end_supervision() -> anyhow::Result<()> {
        let count = Arc::new(AtomicUsize::new(0));
        let invoked = count.clone();
        let metrics = Metrics::default();
        let cancel = CancellationToken::new();
        let (_, watch) = watch::channel(0);
        let policy = StepPolicy {
            cadence: StepCadence::Continuous {
                min_interval: Duration::from_millis(1),
            },
            timeout: Some(Duration::from_millis(5)),
            backoff: Backoff {
                initial: Duration::from_millis(1),
                maximum: Duration::from_millis(2),
            },
        };
        let worker = tokio::spawn(supervise(
            move || {
                let index = invoked.fetch_add(1, Ordering::SeqCst);
                async move {
                    match index {
                        0 => Err(anyhow::anyhow!("temporary RPC failure")),
                        1 => std::panic::resume_unwind(Box::new("application panic")),
                        2 => {
                            tokio::time::sleep(Duration::from_secs(60)).await;
                            Ok(())
                        }
                        _ => Ok(()),
                    }
                }
            },
            watch,
            policy,
            metrics.clone(),
            cancel.clone(),
        ));
        tokio::time::timeout(Duration::from_secs(2), async {
            while count.load(Ordering::SeqCst) < 5 {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        cancel.cancel();
        worker.await??;
        assert!(metrics.render().contains("rtao_step_errors_total 3\n"));
        assert_eq!(metrics.readiness().consecutive_step_failures, 0);
        Ok(())
    }
    #[tokio::test]
    async fn finalized_cadence_waits_and_shutdown_cancels_inflight_callback() -> anyhow::Result<()>
    {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let (sender, watch) = watch::channel(0);
        let cancel = CancellationToken::new();
        let worker = tokio::spawn(supervise(
            move || {
                observed.fetch_add(1, Ordering::SeqCst);
                async {
                    futures::future::pending::<()>().await;
                    Ok(())
                }
            },
            watch,
            StepPolicy::default(),
            Metrics::default(),
            cancel.clone(),
        ));
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        sender.send(1)?;
        tokio::time::timeout(Duration::from_secs(1), async {
            while calls.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(1), worker).await???;
        Ok(())
    }
}
