//! Failure injection: per-method plans driven by call counters.

use std::sync::atomic::{AtomicUsize, Ordering};

/// When a method should fail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FailPlan {
    #[default]
    /// Never fail (default).
    Never,
    /// Fail every call.
    Always,
    /// Fail the first N calls, then recover.
    Times(usize),
}

impl FailPlan {
    pub(crate) fn tripped(&self, counter: &AtomicUsize) -> bool {
        match self {
            FailPlan::Never => false,
            FailPlan::Always => true,
            FailPlan::Times(n) => counter.fetch_add(1, Ordering::SeqCst) < *n,
        }
    }
}

/// All injectable failure points of one mock, defaulting to Never.
#[derive(Clone, Copy, Debug, Default)]
pub struct FailureKnobs {
    pub put: FailPlan,
    pub get: FailPlan,
    pub update: FailPlan,
    pub delete: FailPlan,
    pub query: FailPlan,
}

impl FailureKnobs {
    /// Everything succeeds.
    pub fn none() -> Self {
        Self::default()
    }

    /// Every method fails — for hard-down scenarios.
    pub fn all_always() -> Self {
        Self {
            put: FailPlan::Always,
            get: FailPlan::Always,
            update: FailPlan::Always,
            delete: FailPlan::Always,
            query: FailPlan::Always,
        }
    }
}

/// Shared counters matching FailureKnobs' fields.
#[derive(Default)]
pub(crate) struct FailureCounters {
    pub put: AtomicUsize,
    pub get: AtomicUsize,
    pub update: AtomicUsize,
    pub delete: AtomicUsize,
    pub query: AtomicUsize,
}

pub(crate) fn err(which: &str) -> memory_domain::MemoryError {
    memory_domain::MemoryError::unavailable("mock", format!("injected {which} failure"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_fails_exactly_n_then_recovers() {
        let plan = FailPlan::Times(2);
        let counter = AtomicUsize::new(0);
        assert!(plan.tripped(&counter));
        assert!(plan.tripped(&counter));
        assert!(!plan.tripped(&counter));
        assert!(!plan.tripped(&counter));
    }

    #[test]
    fn never_and_always() {
        let c = AtomicUsize::new(0);
        assert!(!FailPlan::Never.tripped(&c));
        assert!(FailPlan::Always.tripped(&c));
    }
}
