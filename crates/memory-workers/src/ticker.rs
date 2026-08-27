//! Drives workers on their cadences without spawning per-worker timers
//! that outlive their usefulness in tests.

use crate::worker::Worker;
use memory_domain::MemoryResult;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Executes workers: `tick_once` for tests and schedulers, `run`
/// for long-lived loops until the shutdown future resolves.
pub struct WorkerTicker {
    workers: Vec<Arc<dyn Worker>>,
    last_run: Vec<Option<Instant>>,
}

impl WorkerTicker {
    /// Creates a ticker for the given workers.
    pub fn new(workers: Vec<Arc<dyn Worker>>) -> Self {
        let n = workers.len();
        Self {
            workers,
            last_run: vec![None; n],
        }
    }

    /// Runs every worker whose interval has elapsed. Returns how many
    /// workers ran; deterministic and instant for tests.
    pub async fn tick_once(&mut self, now: Instant) -> MemoryResult<usize> {
        let mut ran = 0;
        for (i, worker) in self.workers.iter().enumerate() {
            let due = self.last_run[i]
                .map(|at| now.duration_since(at) >= worker.interval())
                .unwrap_or(true);
            if due {
                // Errors never abort the loop (worker contract).
                let _ = worker.run_once().await;
                self.last_run[i] = Some(now);
                ran += 1;
            }
        }
        Ok(ran)
    }

    /// Runs until `shutdown` resolves, ticking at the finest worker
    /// cadence. Used by server mode's background supervisor.
    pub async fn run_until<F>(mut self, mut shutdown: F)
    where
        F: std::future::Future<Output = ()> + Send + Unpin,
    {
        let cadence = self
            .workers
            .iter()
            .map(|w| w.interval())
            .min()
            .unwrap_or(Duration::from_secs(30));
        let mut last = Instant::now();
        loop {
            tokio::select! {
                _ = &mut shutdown => break,
                _ = tokio::time::sleep(cadence) => {
                    let now = last + cadence;
                    let _ = self.tick_once(now).await;
                    last = now;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingWorker {
        interval: Duration,
        runs: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl Worker for CountingWorker {
        fn name(&self) -> &str {
            "counting"
        }
        async fn run_once(&self) -> MemoryResult<()> {
            self.runs.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn interval(&self) -> Duration {
            self.interval
        }
    }

    #[tokio::test]
    async fn ticks_respect_worker_cadences() {
        let fast = Arc::new(CountingWorker {
            interval: Duration::from_secs(1),
            runs: AtomicUsize::new(0),
        });
        let slow = Arc::new(CountingWorker {
            interval: Duration::from_secs(10),
            runs: AtomicUsize::new(0),
        });
        let mut ticker = WorkerTicker::new(vec![fast.clone(), slow.clone()]);

        let t0 = Instant::now();
        assert_eq!(
            ticker.tick_once(t0).await.unwrap(),
            2,
            "first tick runs all"
        );

        // +2s: fast due, slow not.
        assert_eq!(
            ticker.tick_once(t0 + Duration::from_secs(2)).await.unwrap(),
            1
        );
        assert_eq!(fast.runs.load(Ordering::SeqCst), 2);
        assert_eq!(slow.runs.load(Ordering::SeqCst), 1);

        // +11s: both due again.
        assert_eq!(
            ticker
                .tick_once(t0 + Duration::from_secs(11))
                .await
                .unwrap(),
            2
        );
        assert_eq!(fast.runs.load(Ordering::SeqCst), 3);
        assert_eq!(slow.runs.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn run_until_stops_on_shutdown() {
        // Keep a handle so the assertion sees what the loop ran.
        let worker = Arc::new(CountingWorker {
            interval: Duration::from_millis(1),
            runs: AtomicUsize::new(0),
        });
        let handle = Arc::clone(&worker);
        let ticker = WorkerTicker::new(vec![worker]);

        let shutdown = tokio::time::sleep(Duration::from_millis(50));
        tokio::pin!(shutdown);
        ticker.run_until(shutdown).await;
        assert!(
            handle.runs.load(Ordering::SeqCst) >= 1,
            "worker ran at least once"
        );
    }
}
