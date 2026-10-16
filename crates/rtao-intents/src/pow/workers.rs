// Derived from RaoFoundation/subtensor sdk/python/bittensor/pow_registration.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Task-owned bounded CPU threads. Workers receive public bytes only.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
};

use super::PowError;

pub(super) const ATTEMPTS: u32 = 1024;
pub(super) type Solution = Result<Option<(u64, [u8; 32])>, String>;
pub(super) type Miner = Arc<dyn Fn(&[u8], u64, u64, u32) -> Solution + Send + Sync>;

pub(super) enum WorkerFactory {
    Cpu(Miner),
    #[cfg(feature = "pow-gpu")]
    Gpu {
        options: super::GpuOptions,
        factory: super::gpu::Factory,
    },
}

impl WorkerFactory {
    pub(super) async fn start(&self, cpu_workers: u8) -> Result<WorkerPool, PowError> {
        match self {
            Self::Cpu(miner) => Ok(WorkerPool::Cpu(CpuWorkers::new(
                cpu_workers,
                miner.clone(),
            )?)),
            #[cfg(feature = "pow-gpu")]
            Self::Gpu { options, factory } => {
                match super::gpu::GpuWorker::start(options.clone(), factory.clone()).await? {
                    Some(worker) => Ok(WorkerPool::Gpu(worker)),
                    None => Ok(WorkerPool::Cpu(CpuWorkers::new(
                        cpu_workers,
                        Arc::new(rtao_core::pow::mine_cpu),
                    )?)),
                }
            }
        }
    }
}

pub(super) enum WorkerPool {
    Cpu(CpuWorkers),
    #[cfg(feature = "pow-gpu")]
    Gpu(super::gpu::GpuWorker),
}

impl WorkerPool {
    pub(super) fn dispatch(
        &self,
        prefix: Arc<[u8; rtao_core::pow::PREFIX_LEN]>,
        difficulty: u64,
        start: u64,
    ) -> Result<u8, PowError> {
        match self {
            Self::Cpu(pool) => pool.dispatch(prefix, difficulty, start),
            #[cfg(feature = "pow-gpu")]
            Self::Gpu(pool) => pool.dispatch(prefix, difficulty, start),
        }
    }

    pub(super) fn nonce_span(&self) -> u64 {
        match self {
            Self::Cpu(pool) => (pool.senders.len() as u64) * u64::from(ATTEMPTS),
            #[cfg(feature = "pow-gpu")]
            Self::Gpu(pool) => pool.nonce_span(),
        }
    }

    pub(super) async fn result(&mut self) -> Result<Solution, PowError> {
        match self {
            Self::Cpu(pool) => pool.result().await,
            #[cfg(feature = "pow-gpu")]
            Self::Gpu(pool) => pool.result().await,
        }
    }

    pub(super) async fn drain(&mut self, remaining: u8) -> Result<(), PowError> {
        for _ in 0..remaining {
            self.result().await?.map_err(PowError::Core)?;
        }
        Ok(())
    }
}

struct Job {
    prefix: Arc<[u8; rtao_core::pow::PREFIX_LEN]>,
    difficulty: u64,
    start: u64,
}

pub(super) struct CpuWorkers {
    senders: Vec<mpsc::SyncSender<Job>>,
    threads: Vec<JoinHandle<()>>,
    stopped: Arc<AtomicBool>,
    results: tokio::sync::mpsc::Receiver<Solution>,
}

impl CpuWorkers {
    pub(super) fn new(requested: u8, miner: Miner) -> Result<Self, PowError> {
        let count =
            usize::from(requested).min(std::thread::available_parallelism().map_or(1, usize::from));
        let (results, receiver) = tokio::sync::mpsc::channel(count);
        let mut pool = Self {
            senders: Vec::new(),
            threads: Vec::new(),
            stopped: Arc::new(AtomicBool::new(false)),
            results: receiver,
        };
        for index in 0..count {
            let (sender, jobs) = mpsc::sync_channel::<Job>(1);
            let stopped = pool.stopped.clone();
            let results = results.clone();
            let miner = miner.clone();
            let thread = std::thread::Builder::new()
                .name(format!("rtao-pow-{index}"))
                .spawn(move || {
                    while let Ok(job) = jobs.recv() {
                        if stopped.load(Ordering::Acquire) {
                            break;
                        }
                        let result =
                            miner(job.prefix.as_ref(), job.difficulty, job.start, ATTEMPTS);
                        if stopped.load(Ordering::Acquire) || results.try_send(result).is_err() {
                            break;
                        }
                    }
                })
                .map_err(|error| PowError::Worker(error.to_string()))?;
            pool.senders.push(sender);
            pool.threads.push(thread);
        }
        Ok(pool)
    }

    pub(super) fn dispatch(
        &self,
        prefix: Arc<[u8; rtao_core::pow::PREFIX_LEN]>,
        difficulty: u64,
        start: u64,
    ) -> Result<u8, PowError> {
        for (index, sender) in self.senders.iter().enumerate() {
            sender
                .try_send(Job {
                    prefix: prefix.clone(),
                    difficulty,
                    start: start.wrapping_add((index as u64) * u64::from(ATTEMPTS)),
                })
                .map_err(|error| PowError::Worker(error.to_string()))?;
        }
        u8::try_from(self.senders.len()).map_err(|error| PowError::Worker(error.to_string()))
    }

    pub(super) async fn result(&mut self) -> Result<Solution, PowError> {
        self.results
            .recv()
            .await
            .ok_or_else(|| PowError::Worker("CPU worker result channel closed".into()))
    }

    #[cfg(test)]
    pub(super) async fn drain(&mut self, remaining: u8) -> Result<(), PowError> {
        for _ in 0..remaining {
            self.result().await?.map_err(PowError::Core)?;
        }
        Ok(())
    }
}

impl Drop for CpuWorkers {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.senders.clear();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}
