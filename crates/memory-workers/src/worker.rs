//! The worker contract.

use async_trait::async_trait;
use memory_domain::MemoryResult;

/// One recurring background job.
#[async_trait]
pub trait Worker: Send + Sync {
    /// Worker name for logs and health.
    fn name(&self) -> &str;

    /// Runs one iteration. Tickers call this on their cadence; errors
    /// are logged by the ticker and never abort the loop.
    async fn run_once(&self) -> MemoryResult<()>;

    /// Cadence the ticker should use for this worker.
    fn interval(&self) -> std::time::Duration;
}

/// A fixed set of workers with a name-sorted registry for operators.
pub struct WorkerRegistry {
    workers: Vec<std::sync::Arc<dyn Worker>>,
}

impl WorkerRegistry {
    /// Empty registry.
    pub fn new() -> Self {
        Self {
            workers: Vec::new(),
        }
    }

    /// Adds a worker.
    pub fn register(mut self, worker: std::sync::Arc<dyn Worker>) -> Self {
        self.workers.push(worker);
        self
    }

    /// Registered workers.
    pub fn workers(&self) -> &[std::sync::Arc<dyn Worker>] {
        &self.workers
    }

    /// Worker names, sorted.
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.workers.iter().map(|w| w.name()).collect();
        names.sort_unstable();
        names
    }
}

impl Default for WorkerRegistry {
    fn default() -> Self {
        Self::new()
    }
}
