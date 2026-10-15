// Derived from RaoFoundation/subtensor sdk/python/bittensor/pow_registration.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! One task-owned OpenCL worker; only public challenges enter device memory.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
};

use super::{PowError, workers::Solution};

/// Select the public registration miner without changing the default CPU API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowBackend {
    /// Use all discovered GPUs, or CPU when no usable devices are discovered.
    Auto,
    /// Use CPU workers without discovery or loading the OpenCL driver.
    Cpu,
    /// Require GPUs and report missing drivers/devices as an error.
    Gpu,
}

/// Optional OpenCL selection and bounded work per device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuOptions {
    /// Automatic, CPU-only, or required GPU execution.
    pub backend: PowBackend,
    /// Discovered device identifiers. `None` selects all GPUs.
    ///
    /// A selection must contain 1..=32 distinct identifiers and requires GPU
    /// availability even when `backend` is automatic.
    pub device_ids: Option<Vec<usize>>,
    /// Work in one kernel launch per device, in `1..=rtao_core::pow::MAX_BATCH`.
    pub attempts_per_device: u32,
}

impl Default for GpuOptions {
    fn default() -> Self {
        Self {
            backend: PowBackend::Auto,
            device_ids: None,
            attempts_per_device: 100_000,
        }
    }
}

impl GpuOptions {
    pub(super) fn validate(&self) -> Result<(), String> {
        if self.attempts_per_device == 0 || self.attempts_per_device > rtao_core::pow::MAX_BATCH {
            return Err(format!(
                "GPU batches must contain 1..={} attempts per device",
                rtao_core::pow::MAX_BATCH
            ));
        }
        if let Some(ids) = &self.device_ids {
            if self.backend == PowBackend::Cpu || ids.is_empty() || ids.len() > 32 {
                return Err(
                    "select 1..=32 unique GPU device ids with automatic or GPU execution".into(),
                );
            }
            let mut unique = ids.clone();
            unique.sort_unstable();
            unique.dedup();
            if unique.len() != ids.len() {
                return Err(
                    "select 1..=32 unique GPU device ids with automatic or GPU execution".into(),
                );
            }
        }
        Ok(())
    }
}

/// Enumerate OpenCL GPUs off the async runtime. An absent driver returns an
/// empty list. This operation is explicit; the CPU solver never calls it.
pub async fn gpu_devices() -> Result<Vec<rtao_core::pow::opencl::GpuDevice>, PowError> {
    tokio::task::spawn_blocking(rtao_core::pow::opencl::gpu_devices)
        .await
        .map_err(|error| PowError::Worker(error.to_string()))?
        .map_err(PowError::Core)
}

pub(super) trait Backend: Send {
    fn device_count(&self) -> usize;
    fn mine(&mut self, prefix: &[u8], difficulty: u64, start: u64, attempts: u32) -> Solution;
}

impl Backend for rtao_core::pow::opencl::GpuMiner {
    fn device_count(&self) -> usize {
        self.devices().len()
    }

    fn mine(&mut self, prefix: &[u8], difficulty: u64, start: u64, attempts: u32) -> Solution {
        self.mine(prefix, difficulty, start, attempts)
    }
}

pub(super) type Factory =
    Arc<dyn Fn(&GpuOptions) -> Result<Option<Box<dyn Backend>>, String> + Send + Sync>;

pub(super) fn open_backend(options: &GpuOptions) -> Result<Option<Box<dyn Backend>>, String> {
    select_backend(
        options,
        || rtao_core::pow::opencl::gpu_devices().map(|devices| !devices.is_empty()),
        || {
            rtao_core::pow::opencl::GpuMiner::new(options.device_ids.as_deref())
                .map(|miner| Box::new(miner) as Box<dyn Backend>)
        },
    )
}

pub(super) fn select_backend(
    options: &GpuOptions,
    discover: impl FnOnce() -> Result<bool, String>,
    open: impl FnOnce() -> Result<Box<dyn Backend>, String>,
) -> Result<Option<Box<dyn Backend>>, String> {
    if options.backend == PowBackend::Cpu {
        return Ok(None);
    }
    if !discover()? {
        if options.backend == PowBackend::Auto && options.device_ids.is_none() {
            return Ok(None);
        }
        return Err("no usable OpenCL GPUs found; install the GPU vendor's OpenCL driver".into());
    }
    // A discovered device's startup/self-test failure must remain visible even
    // in automatic mode. The core validates device ids and owns all driver I/O.
    Ok(Some(open()?))
}

struct Job {
    prefix: Arc<[u8; rtao_core::pow::PREFIX_LEN]>,
    difficulty: u64,
    start: u64,
}

pub(super) struct GpuWorker {
    sender: Option<mpsc::SyncSender<Job>>,
    thread: Option<JoinHandle<()>>,
    stopped: Arc<AtomicBool>,
    results: tokio::sync::mpsc::Receiver<Solution>,
    nonce_span: u64,
}

impl GpuWorker {
    pub(super) async fn start(
        options: GpuOptions,
        factory: Factory,
    ) -> Result<Option<Self>, PowError> {
        options.validate().map_err(PowError::Core)?;
        let (sender, jobs) = mpsc::sync_channel::<Job>(1);
        let (ready_sender, ready) = tokio::sync::oneshot::channel();
        let (results, receiver) = tokio::sync::mpsc::channel(1);
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let attempts = options.attempts_per_device;
        let thread = std::thread::Builder::new()
            .name("rtao-pow-gpu".into())
            .spawn(move || {
                let mut backend = match factory(&options) {
                    Ok(Some(backend)) => backend,
                    Ok(None) => {
                        let _ = ready_sender.send(Ok(None));
                        return;
                    }
                    Err(error) => {
                        let _ = ready_sender.send(Err(error));
                        return;
                    }
                };
                let count = backend.device_count();
                if !(1..=32).contains(&count) {
                    let _ = ready_sender.send(Err("GPU miner must select 1..=32 devices".into()));
                    return;
                }
                let nonce_span = (count as u64) * u64::from(attempts);
                if ready_sender.send(Ok(Some(nonce_span))).is_err() {
                    return;
                }
                while let Ok(job) = jobs.recv() {
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    let result = backend
                        .mine(job.prefix.as_ref(), job.difficulty, job.start, attempts)
                        .and_then(|solution| {
                            if solution.is_some_and(|(nonce, _)| {
                                nonce.wrapping_sub(job.start) >= nonce_span
                            }) {
                                Err("GPU returned a nonce outside its assigned range".into())
                            } else {
                                Ok(solution)
                            }
                        });
                    if stop.load(Ordering::Acquire) || results.try_send(result).is_err() {
                        break;
                    }
                }
            })
            .map_err(|error| PowError::Worker(error.to_string()))?;
        // Install the owning guard before awaiting initialization. Cancellation
        // or a dropped startup future therefore joins and drops this backend.
        let mut worker = Self {
            sender: Some(sender),
            thread: Some(thread),
            stopped,
            results: receiver,
            nonce_span: 0,
        };
        match ready
            .await
            .map_err(|_| PowError::Worker("GPU initialization channel closed".into()))?
            .map_err(PowError::Core)?
        {
            Some(nonce_span) => {
                worker.nonce_span = nonce_span;
                Ok(Some(worker))
            }
            None => Ok(None),
        }
    }

    pub(super) fn dispatch(
        &self,
        prefix: Arc<[u8; rtao_core::pow::PREFIX_LEN]>,
        difficulty: u64,
        start: u64,
    ) -> Result<u8, PowError> {
        self.sender
            .as_ref()
            .ok_or_else(|| PowError::Worker("GPU worker is closed".into()))?
            .try_send(Job {
                prefix,
                difficulty,
                start,
            })
            .map_err(|error| PowError::Worker(error.to_string()))?;
        Ok(1)
    }

    pub(super) fn nonce_span(&self) -> u64 {
        self.nonce_span
    }

    pub(super) async fn result(&mut self) -> Result<Solution, PowError> {
        self.results
            .recv()
            .await
            .ok_or_else(|| PowError::Worker("GPU worker result channel closed".into()))
    }
}

impl Drop for GpuWorker {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
