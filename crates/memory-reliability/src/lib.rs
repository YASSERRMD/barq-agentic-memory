//! Reliability primitives: retries, timeouts, circuit breakers, health.

pub mod breaker;
pub mod retry;

pub use breaker::{CircuitBreaker, CircuitState, Health, HealthStatus};
pub use retry::{RetryPolicy, with_retry};
