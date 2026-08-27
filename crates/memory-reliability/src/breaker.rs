//! Circuit breaker and health reporting.
//!
//! Blueprint failure mode: when the vector index goes down, exact
//! retrieval through the canonical store must keep serving. Breakers
//! wrap flaky providers; health summarizes what is currently usable.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Breaker state machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CircuitState {
    /// Operating normally.
    Closed,
    /// Failing fast; calls short-circuit without touching the backend.
    Open,
    /// Allowing probe traffic after the cooldown.
    HalfOpen,
}

/// State codes for the atomic.
const CLOSED: u8 = 0;
const OPEN: u8 = 1;
const HALF_OPEN: u8 = 2;

/// Trip-after-N-consecutive-failures circuit breaker.
pub struct CircuitBreaker {
    state: AtomicU8,
    consecutive_failures: AtomicU64,
    threshold: u64,
    cooldown: Mutex<Option<Instant>>,
    cooldown_duration: Duration,
}

impl CircuitBreaker {
    /// Breaks after `threshold` consecutive failures, half-opens after
    /// `cooldown`.
    pub fn new(threshold: u64, cooldown: Duration) -> Self {
        Self {
            state: AtomicU8::new(CLOSED),
            consecutive_failures: AtomicU64::new(0),
            threshold: threshold.max(1),
            cooldown: Mutex::new(None),
            cooldown_duration: cooldown,
        }
    }

    /// Current state, applying the cooldown transition if due.
    pub fn state(&self) -> CircuitState {
        let raw = self.state.load(Ordering::SeqCst);
        if raw == OPEN {
            let guard = self.cooldown.lock().expect("poisoned");
            if guard.is_some_and(|at| Instant::now() >= at) {
                self.state.store(HALF_OPEN, Ordering::SeqCst);
                self.consecutive_failures.store(0, Ordering::SeqCst);
                return CircuitState::HalfOpen;
            }
            return CircuitState::Open;
        }
        match raw {
            OPEN => CircuitState::Open,
            HALF_OPEN => CircuitState::HalfOpen,
            _ => CircuitState::Closed,
        }
    }

    /// True when a call may proceed; open circuits fail fast.
    pub fn allow(&self) -> bool {
        !matches!(self.state(), CircuitState::Open)
    }

    /// Records a success; closes the breaker.
    pub fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::SeqCst);
        self.state.store(CLOSED, Ordering::SeqCst);
    }

    /// Records a failure; opens the circuit past the threshold.
    pub fn record_failure(&self) {
        let failures = self.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;
        if failures >= self.threshold {
            self.state.store(OPEN, Ordering::SeqCst);
            *self.cooldown.lock().expect("poisoned") =
                Some(Instant::now() + self.cooldown_duration);
        }
    }

    /// Runs an operation under the breaker: open circuits fail fast
    /// with [`memory_domain::MemoryError::ProviderUnavailable`];
    /// successes and failures update the state machine.
    pub async fn call<F, Fut, T>(&self, mut operation: F) -> memory_domain::MemoryResult<T>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = memory_domain::MemoryResult<T>>,
    {
        if !self.allow() {
            return Err(memory_domain::MemoryError::unavailable(
                "circuit-open",
                "failing fast until cooldown elapses",
            ));
        }
        match operation().await {
            Ok(v) => {
                self.record_success();
                Ok(v)
            }
            Err(e) => {
                self.record_failure();
                Err(e)
            }
        }
    }
}

/// Coarse status used by health endpoints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthStatus {
    /// Fully operational.
    Healthy,
    /// Serving, but a secondary backend is degraded (e.g. vector down,
    /// exact retrieval still available).
    Degraded,
    /// Cannot serve its primary function.
    Unhealthy,
}

impl HealthStatus {
    /// The worst of a set of statuses.
    pub fn worst(all: &[HealthStatus]) -> HealthStatus {
        all.iter()
            .fold(HealthStatus::Healthy, |acc, s| match (acc, s) {
                (_, HealthStatus::Unhealthy) | (HealthStatus::Unhealthy, _) => {
                    HealthStatus::Unhealthy
                }
                (_, HealthStatus::Degraded) | (HealthStatus::Degraded, _) => HealthStatus::Degraded,
                _ => HealthStatus::Healthy,
            })
    }
}

/// Health of one component.
#[derive(Clone, Debug)]
pub struct Health {
    /// Component name ("store", "vector", "working").
    pub component: &'static str,
    /// Coarse status.
    pub status: HealthStatus,
    /// Free-form detail for operators.
    pub detail: Option<String>,
}

impl Health {
    /// Healthy component.
    pub fn ok(component: &'static str) -> Self {
        Self {
            component,
            status: HealthStatus::Healthy,
            detail: None,
        }
    }

    /// Degraded component with detail.
    pub fn degraded(component: &'static str, detail: impl Into<String>) -> Self {
        Self {
            component,
            status: HealthStatus::Degraded,
            detail: Some(detail.into()),
        }
    }

    /// Dead component with detail.
    pub fn down(component: &'static str, detail: impl Into<String>) -> Self {
        Self {
            component,
            status: HealthStatus::Unhealthy,
            detail: Some(detail.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_domain::{MemoryError, MemoryResult};

    #[tokio::test]
    async fn opens_after_threshold_and_half_opens_after_cooldown() {
        let breaker = CircuitBreaker::new(3, Duration::from_millis(30));

        for _ in 0..2 {
            let _: MemoryResult<()> = breaker
                .call(|| async { Err::<(), MemoryError>(MemoryError::unavailable("db", "flap")) })
                .await;
        }
        assert_eq!(
            breaker.state(),
            CircuitState::Closed,
            "two failures tolerated"
        );

        let _: MemoryResult<()> = breaker
            .call(|| async { Err::<(), MemoryError>(MemoryError::unavailable("db", "flap")) })
            .await;
        assert_eq!(breaker.state(), CircuitState::Open);

        // Open circuit fails fast without invoking the backend.
        let mut invoked = false;
        let result: MemoryResult<()> = breaker
            .call(|| {
                invoked = true;
                async { Ok(()) }
            })
            .await;
        assert!(matches!(
            result,
            Err(MemoryError::ProviderUnavailable { .. })
        ));
        assert!(!invoked, "open breaker must not touch the backend");

        // Cooldown elapses -> half-open -> success closes.
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(breaker.state(), CircuitState::HalfOpen);
        let ok: MemoryResult<&str> = breaker.call(|| async { Ok("probe") }).await;
        assert_eq!(ok.unwrap(), "probe");
        assert_eq!(breaker.state(), CircuitState::Closed);
    }

    #[test]
    fn health_aggregation_is_worst_case() {
        assert_eq!(
            HealthStatus::worst(&[HealthStatus::Healthy, HealthStatus::Degraded]),
            HealthStatus::Degraded
        );
        assert_eq!(
            HealthStatus::worst(&[HealthStatus::Degraded, HealthStatus::Unhealthy]),
            HealthStatus::Unhealthy
        );
        assert_eq!(
            HealthStatus::worst(&[HealthStatus::Healthy, HealthStatus::Healthy]),
            HealthStatus::Healthy
        );
    }
}
