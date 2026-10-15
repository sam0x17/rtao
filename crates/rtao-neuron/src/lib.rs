//! Bittensor validator, miner and owner runtimes.

#![deny(missing_docs)]

mod clock;
mod config;
#[cfg(feature = "miner")]
mod miner;
#[cfg(feature = "owner")]
mod owner;
#[cfg(feature = "miner")]
mod prune;
#[cfg(feature = "miner")]
mod publisher;
#[cfg(any(feature = "validator", feature = "miner"))]
mod runtime;
#[cfg(any(feature = "validator", feature = "miner"))]
mod shutdown;
mod state;
mod step;
/// Process readiness and Prometheus exposition.
pub mod telemetry;
/// Best-head weight scheduling and actual-epoch reveal attribution.
#[cfg(feature = "validator")]
pub mod weights;

/// Application error type accepted by framework callbacks.
pub use anyhow;
pub use clock::{ChainClock, Clock, ClockGuard, ClockHealth, EpochClock, EpochStatus, SystemClock};
pub use config::{ConfigError, LogLevel, NeuronConfig};
#[cfg(feature = "miner")]
pub use miner::{Miner, MinerRuntime};
#[cfg(feature = "owner")]
pub use owner::{HparamArg, HparamPlan, HyperparamPlanner, WindowWait};
#[cfg(feature = "miner")]
pub use prune::{PruneError, PruneNeuron, PruneParameters, PruneRisk};
#[cfg(feature = "miner")]
pub use publisher::{
    AxonConfig, AxonPublishOutcome, AxonPublisher, NeuronCertificate, PublisherError,
};
#[cfg(feature = "validator")]
pub use runtime::Validator;
#[cfg(any(feature = "validator", feature = "miner"))]
pub use runtime::{NeuronCx, NeuronRuntime, client_config};
pub use state::{EmaScores, LoadedState, ScoreError, StateCell, StateError, StateStore};
pub use step::{Backoff, StepCadence, StepPolicy};
pub use telemetry::{Metrics, Readiness};
#[cfg(feature = "validator")]
pub use weights::{
    VersionKey, WeightCadence, WeightPolicy, WeightScheduler, WeightsInput, WeightsProposal,
};
