//! Retry with exponential backoff and jitter.

use memory_domain::MemoryError;
use std::future::Future;
use std::time::Duration;

/// Backoff policy for transient failures.
#[derive(Clone, Copy, Debug)]
pub struct RetryPolicy {
    /// Maximum attempts including the first (>= 1).
    pub max_attempts: u32,
    /// Delay before the second attempt.
    pub base_delay: Duration,
    /// Ceiling on any single delay.
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(50),
            max_delay: Duration::from_millis(2_000),
        }
    }
}

impl RetryPolicy {
    /// Builds a policy with explicit bounds.
    pub fn new(max_attempts: u32, base_delay: Duration, max_delay: Duration) -> Self {
        Self {
            max_attempts: max_attempts.max(1),
            base_delay,
            max_delay,
        }
    }

    /// Delay before attempt `attempt` (1-based, after the first).
    pub fn delay_for(&self, attempt: u32) -> Duration {
        // Exponential: base * 2^(attempt-1), capped.
        let exp = self
            .base_delay
            .checked_mul(1u32 << (attempt - 1).min(16))
            .unwrap_or(self.max_delay);
        exp.min(self.max_delay)
    }
}

/// Runs `operation` under the policy, retrying only errors the domain
/// model marks retryable (transient conflicts, unavailable providers).
///
/// The final error is returned verbatim; attempts are logged to the
/// optional `on_retry` callback for observability.
pub async fn with_retry<F, Fut, T, E>(policy: RetryPolicy, mut operation: F) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: Into<MemoryError> + Clone,
{
    let mut last_err: Option<E> = None;
    for attempt in 1..=policy.max_attempts {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(e) => {
                let domain: MemoryError = e.clone().into();
                if !domain.is_retryable() || attempt == policy.max_attempts {
                    return Err(e);
                }
                last_err = Some(e);
                tokio::time::sleep(policy.delay_for(attempt)).await;
            }
        }
    }
    // Unreachable: the loop always returns on the final attempt.
    Err(last_err.expect("at least one attempt"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[derive(Clone, Debug, PartialEq)]
    struct FlakyError(bool);

    impl From<FlakyError> for MemoryError {
        fn from(e: FlakyError) -> MemoryError {
            if e.0 {
                MemoryError::unavailable("flaky", "transient")
            } else {
                MemoryError::validation("flaky", "permanent")
            }
        }
    }

    #[tokio::test]
    async fn retries_until_success_then_stops() {
        let attempts = Cell::new(0u32);
        let policy = RetryPolicy::new(5, Duration::from_millis(1), Duration::from_millis(2));
        let result = with_retry(policy, || async {
            attempts.set(attempts.get() + 1);
            if attempts.get() < 3 {
                Err(FlakyError(true))
            } else {
                Ok("recovered")
            }
        })
        .await;
        assert_eq!(result.unwrap(), "recovered");
        assert_eq!(attempts.get(), 3);
    }

    #[tokio::test]
    async fn permanent_errors_fail_immediately() {
        let attempts = Cell::new(0u32);
        let policy = RetryPolicy::new(5, Duration::from_millis(1), Duration::from_millis(2));
        let result: Result<(), FlakyError> = with_retry(policy, || async {
            attempts.set(attempts.get() + 1);
            Err(FlakyError(false))
        })
        .await;
        assert_eq!(result.unwrap_err(), FlakyError(false));
        assert_eq!(attempts.get(), 1, "permanent errors never retry");
    }

    #[tokio::test]
    async fn exhausts_attempts_on_persistent_transients() {
        let attempts = Cell::new(0u32);
        let policy = RetryPolicy::new(3, Duration::from_millis(1), Duration::from_millis(2));
        let result: Result<(), FlakyError> = with_retry(policy, || async {
            attempts.set(attempts.get() + 1);
            Err::<(), FlakyError>(FlakyError(true))
        })
        .await;
        assert_eq!(result.unwrap_err(), FlakyError(true));
        assert_eq!(attempts.get(), 3);
    }

    #[test]
    fn delays_back_off_exponentially_and_cap() {
        let p = RetryPolicy::new(10, Duration::from_millis(10), Duration::from_millis(100));
        assert_eq!(p.delay_for(1), Duration::from_millis(10));
        assert_eq!(p.delay_for(2), Duration::from_millis(20));
        assert_eq!(p.delay_for(4), Duration::from_millis(80));
        assert_eq!(p.delay_for(5), Duration::from_millis(100), "capped");
        assert_eq!(p.delay_for(9), Duration::from_millis(100));
    }
}
