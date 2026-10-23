//! Bittensor validator, miner and owner runtimes.

#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
#![cfg_attr(docsrs, feature(doc_cfg))]

mod clock;
mod config;
#[cfg(feature = "miner")]
mod miner;
#[cfg(feature = "owner")]
mod owner;
#[cfg(feature = "owner")]
mod owner_monitor;
#[cfg(feature = "owner")]
mod owner_ops;
#[cfg(any(feature = "miner", feature = "owner"))]
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
#[cfg_attr(docsrs, doc(cfg(feature = "validator")))]
pub mod weights;

/// Application error type accepted by framework callbacks.
pub use anyhow;
pub use clock::{ChainClock, Clock, ClockGuard, ClockHealth, EpochClock, EpochStatus, SystemClock};
pub use config::{ConfigError, LogLevel, NeuronConfig};
#[cfg(feature = "miner")]
#[cfg_attr(docsrs, doc(cfg(feature = "miner")))]
pub use miner::{Miner, MinerRuntime};
#[cfg(feature = "owner")]
#[cfg_attr(docsrs, doc(cfg(feature = "owner")))]
pub use owner::{HparamArg, HparamPlan, HyperparamPlanner, WindowWait};
#[cfg(feature = "owner")]
#[cfg_attr(docsrs, doc(cfg(feature = "owner")))]
pub use owner_monitor::{
    OwnerConviction, OwnerEvent, OwnerEventCursor, OwnerEventStream, OwnerMonitor,
    OwnerMonitorError, OwnerObservation, OwnerProxyAnnouncement,
};
#[cfg(feature = "owner")]
#[cfg_attr(docsrs, doc(cfg(feature = "owner")))]
pub use owner_ops::{
    OwnerConfig, OwnerOps, RegistrationWait, SubnetIdentity, TrimNullAction, TrimNullConfig,
    TrimNullSession, TrimNullStep, TrimReport,
};
#[cfg(any(feature = "miner", feature = "owner"))]
#[cfg_attr(docsrs, doc(cfg(any(feature = "miner", feature = "owner"))))]
pub use prune::{PruneError, PruneNeuron, PruneParameters, PruneRisk};
#[cfg(feature = "miner")]
#[cfg_attr(docsrs, doc(cfg(feature = "miner")))]
pub use publisher::{
    AxonConfig, AxonPublishOutcome, AxonPublisher, NeuronCertificate, PublisherError,
};
#[cfg(feature = "validator")]
#[cfg_attr(docsrs, doc(cfg(feature = "validator")))]
pub use runtime::Validator;
#[cfg(any(feature = "validator", feature = "miner"))]
#[cfg_attr(docsrs, doc(cfg(any(feature = "validator", feature = "miner"))))]
pub use runtime::{NeuronCx, NeuronRuntime, client_config};
pub use state::{EmaScores, LoadedState, ScoreError, StateCell, StateError, StateStore};
pub use step::{Backoff, StepCadence, StepPolicy};
pub use telemetry::{Metrics, Readiness};
#[cfg(feature = "validator")]
#[cfg_attr(docsrs, doc(cfg(feature = "validator")))]
pub use weights::{
    VersionKey, WeightCadence, WeightOpportunity, WeightPolicy, WeightScheduler, WeightsInput,
    WeightsProposal,
};
