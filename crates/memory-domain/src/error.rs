//! The engine error model.
//!
//! One error type crosses every boundary: provider backends, the engine
//! core, and (later) server transports all surface these variants so
//! callers handle a single taxonomy.

use crate::id::MemoryId;
use thiserror::Error;

/// Errors produced by the memory engine and its providers.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum MemoryError {
    /// No record with the given id was visible in the requested scope.
    #[error("memory {memory_id} not found")]
    NotFound {
        /// The missing identifier.
        memory_id: MemoryId,
    },

    /// Input failed validation before reaching storage.
    #[error("invalid {field}: {reason}")]
    Validation {
        /// Name of the offending field or input.
        field: &'static str,
        /// Human-readable explanation.
        reason: String,
    },

    /// Concurrent writers diverged; retry with fresh state.
    #[error("concurrent modification of memory {memory_id}: expected version {expected}, found {actual}")]
    VersionConflict {
        /// Record that changed underneath us.
        memory_id: MemoryId,
        /// Version the caller assumed.
        expected: u64,
        /// Version actually stored.
        actual: u64,
    },

    /// A configured provider backend is not reachable or healthy.
    #[error("provider '{provider}' unavailable: {message}")]
    ProviderUnavailable {
        /// Provider name from configuration.
        provider: String,
        /// Underlying cause description.
        message: String,
    },

    /// A configured capability has no provider registered.
    #[error("no provider registered for capability '{capability}'")]
    ProviderMissing {
        /// Capability name ("store", "vector", "working", ...).
        capability: &'static str,
    },

    /// A provider backend failed while honoring a valid request.
    #[error("storage failure in '{provider}': {message}")]
    Storage {
        /// Provider name where the failure happened.
        provider: String,
        /// Backend error detail.
        message: String,
    },

    /// An operation is valid but not supported by the current build or
    /// provider configuration.
    #[error("unsupported operation: {0}")]
    Unsupported(String),
}

impl MemoryError {
    /// Convenience constructor for [`MemoryError::Validation`].
    pub fn validation(field: &'static str, reason: impl Into<String>) -> Self {
        Self::Validation {
            field,
            reason: reason.into(),
        }
    }

    /// Convenience constructor for [`MemoryError::Storage`].
    pub fn storage(provider: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Storage {
            provider: provider.into(),
            message: message.into(),
        }
    }

    /// Convenience constructor for [`MemoryError::ProviderUnavailable`].
    pub fn unavailable(provider: impl Into<String>, message: impl Into<String>) -> Self {
        Self::ProviderUnavailable {
            provider: provider.into(),
            message: message.into(),
        }
    }

    /// True when retrying the same request could plausibly succeed.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            MemoryError::ProviderUnavailable { .. } | MemoryError::VersionConflict { .. }
        )
    }
}

/// Result alias used across the engine.
pub type MemoryResult<T> = Result<T, MemoryError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_name_the_offending_entity() {
        let id = MemoryId::generate();
        let e = MemoryError::NotFound { memory_id: id };
        assert_eq!(e.to_string(), format!("memory {id} not found"));

        let v = MemoryError::validation("content", "text must not be empty");
        assert_eq!(v.to_string(), "invalid content: text must not be empty");
    }

    #[test]
    fn retryability_covers_transient_and_conflict_cases() {
        let id = MemoryId::generate();
        assert!(MemoryError::unavailable("redis", "conn refused").is_retryable());
        assert!(
            MemoryError::VersionConflict {
                memory_id: id,
                expected: 3,
                actual: 4
            }
            .is_retryable()
        );
        assert!(!MemoryError::NotFound { memory_id: id }.is_retryable());
        assert!(!MemoryError::Unsupported("x".into()).is_retryable());
    }

    #[test]
    fn non_exhaustive_matches_with_wildcards() {
        let e = MemoryError::ProviderMissing {
            capability: "vector",
        };
        let described = match &e {
            MemoryError::ProviderMissing { capability } => format!("missing:{capability}"),
            _ => String::new(),
        };
        assert_eq!(described, "missing:vector");
    }
}
