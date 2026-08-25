//! Engine configuration schema.
//!
//! The same schema drives embedded mode (memory/local backends) and
//! server mode (postgres/redis/vector backends). Parsing is provider
//! neutral: unknown-yet backends parse fine but fail assembly, which
//! keeps config forward-compatible without runtime surprises.

use crate::error::{MemoryError, MemoryResult};
use crate::scope::MemoryScope;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// Default TTL applied to working-memory state when none is given.
pub const DEFAULT_WORKING_MEMORY_TTL: Duration = Duration::from_secs(30 * 60);

/// Top-level engine configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineConfig {
    /// Logical namespace prepended to provider keys/tables.
    pub namespace: String,
    /// Scope stamped onto writes when the caller supplies none.
    pub default_scope: MemoryScope,
    /// Working-memory TTL used when callers omit one.
    #[serde(with = "duration_secs", rename = "working_memory_ttl_secs")]
    pub working_memory_ttl: Duration,
    /// Canonical record store selection.
    pub store: StoreConfig,
    /// Vector index selection; absent disables semantic recall.
    pub vector: Option<VectorStoreConfig>,
    /// Working-state store selection; defaults to the in-process store.
    pub working: Option<WorkingStoreConfig>,
    /// Engine limits.
    pub limits: LimitsConfig,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            namespace: "barq".to_string(),
            default_scope: MemoryScope::default(),
            working_memory_ttl: DEFAULT_WORKING_MEMORY_TTL,
            store: StoreConfig::Memory,
            vector: None,
            working: None,
            limits: LimitsConfig::default(),
        }
    }
}

impl EngineConfig {
    /// Validates cross-field invariants.
    pub fn validated(&self) -> MemoryResult<()> {
        if self.namespace.trim().is_empty() {
            return Err(MemoryError::validation("namespace", "must not be blank"));
        }
        if !self
            .namespace
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(MemoryError::validation(
                "namespace",
                "only [a-zA-Z0-9_-] allowed",
            ));
        }
        self.store.validate()?;
        if let Some(v) = &self.vector {
            v.validate()?;
        }
        if let Some(w) = &self.working {
            w.validate()?;
        }
        self.limits.validate()?;
        Ok(())
    }

    /// True when no external infrastructure is configured.
    pub fn is_embedded(&self) -> bool {
        matches!(self.store, StoreConfig::Memory | StoreConfig::Local { .. })
    }
}

/// Canonical record store backends.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "backend", rename_all = "snake_case")]
pub enum StoreConfig {
    /// Volatile in-process store; tests and ephemeral agents.
    Memory,
    /// Single-file embedded store surviving restarts.
    Local {
        /// Database file path.
        path: PathBuf,
    },
    /// PostgreSQL authoritative store.
    Postgres {
        /// Connection URL.
        url: String,
        /// Pool size cap.
        #[serde(default = "default_max_connections")]
        max_connections: u32,
    },
}

impl StoreConfig {
    fn validate(&self) -> MemoryResult<()> {
        match self {
            StoreConfig::Postgres { url, .. } => {
                if !(url.starts_with("postgres://") || url.starts_with("postgresql://")) {
                    return Err(MemoryError::validation(
                        "store.url",
                        "must start with postgres:// or postgresql://",
                    ));
                }
                Ok(())
            }
            StoreConfig::Local { path } => {
                if path.as_os_str().is_empty() {
                    return Err(MemoryError::validation("store.path", "must not be empty"));
                }
                Ok(())
            }
            StoreConfig::Memory => Ok(()),
        }
    }
}

/// Vector index backends.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "backend", rename_all = "snake_case")]
pub enum VectorStoreConfig {
    /// pgvector sharing the PostgreSQL connection.
    PgVector {
        /// Same connection URL as the canonical store.
        url: String,
    },
    /// In-process flat index; useful for tests before Phase 4 lands.
    InMemory,
}

impl VectorStoreConfig {
    fn validate(&self) -> MemoryResult<()> {
        match self {
            VectorStoreConfig::PgVector { url } => {
                if !(url.starts_with("postgres://") || url.starts_with("postgresql://")) {
                    return Err(MemoryError::validation(
                        "vector.url",
                        "must be a postgres:// URL",
                    ));
                }
                Ok(())
            }
            VectorStoreConfig::InMemory => Ok(()),
        }
    }
}

/// Working-state backends.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "backend", rename_all = "snake_case")]
pub enum WorkingStoreConfig {
    /// Redis-backed session state.
    Redis {
        /// redis:// connection URL.
        url: String,
    },
    /// In-process map; embedded fallback.
    InProcess,
}

impl WorkingStoreConfig {
    fn validate(&self) -> MemoryResult<()> {
        match self {
            WorkingStoreConfig::Redis { url } => {
                if !(url.starts_with("redis://") || url.starts_with("rediss://")) {
                    return Err(MemoryError::validation(
                        "working.url",
                        "must start with redis:// or rediss://",
                    ));
                }
                Ok(())
            }
            WorkingStoreConfig::InProcess => Ok(()),
        }
    }
}

/// Guardrail limits shared across providers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LimitsConfig {
    /// Maximum characters accepted in a single memory text payload.
    pub max_content_chars: usize,
    /// Maximum records per batch write.
    pub max_batch_size: usize,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            max_content_chars: 100_000,
            max_batch_size: 256,
        }
    }
}

impl LimitsConfig {
    fn validate(&self) -> MemoryResult<()> {
        if self.max_content_chars == 0 {
            return Err(MemoryError::validation(
                "limits.max_content_chars",
                "must be greater than zero",
            ));
        }
        if self.max_batch_size == 0 {
            return Err(MemoryError::validation(
                "limits.max_batch_size",
                "must be greater than zero",
            ));
        }
        Ok(())
    }
}

fn default_max_connections() -> u32 {
    10
}

/// Serde helper: durations as integer seconds for operator-friendly TOML.
mod duration_secs {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        d.as_secs().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        Ok(Duration::from_secs(u64::deserialize(d)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_fully_embedded() {
        let c = EngineConfig::default();
        assert!(c.is_embedded());
        assert!(c.validated().is_ok());
        assert_eq!(c.working_memory_ttl, DEFAULT_WORKING_MEMORY_TTL);
    }

    #[test]
    fn json_config_parses_with_backend_tags() {
        let raw = r#"{
            "namespace": "acme",
            "working_memory_ttl_secs": 600,
            "default_scope": {"tenant_id": "acme"},
            "store": {"backend": "local", "path": "/tmp/barq.db"}
        }"#;
        let c: EngineConfig = serde_json::from_str(raw).expect("parse");
        assert_eq!(
            c.store,
            StoreConfig::Local {
                path: PathBuf::from("/tmp/barq.db")
            }
        );
        assert_eq!(c.default_scope.tenant_id.as_deref(), Some("acme"));
        assert_eq!(c.working_memory_ttl, Duration::from_secs(600));
        assert!(c.validated().is_ok());
    }

    #[test]
    fn omitted_fields_fall_back_to_defaults() {
        let c: EngineConfig = serde_json::from_str("{}").expect("parse");
        assert_eq!(c, EngineConfig::default());
    }

    #[test]
    fn unknown_backends_are_rejected_at_parse_time() {
        let raw = r#"{"store": {"backend": "oracle"}}"#;
        assert!(serde_json::from_str::<EngineConfig>(raw).is_err());
    }

    #[test]
    fn postgres_urls_are_shape_checked() {
        let ok = EngineConfig {
            store: StoreConfig::Postgres {
                url: "postgres://localhost/mem".into(),
                max_connections: 4,
            },
            ..EngineConfig::default()
        };
        assert!(ok.validated().is_ok());

        let bad = EngineConfig {
            store: StoreConfig::Postgres {
                url: "mysql://localhost/mem".into(),
                max_connections: 4,
            },
            ..EngineConfig::default()
        };
        assert_eq!(
            bad.validated().unwrap_err(),
            MemoryError::validation("store.url", "must start with postgres:// or postgresql://")
        );
    }

    #[test]
    fn namespaces_reject_shell_unfriendly_characters() {
        let c = EngineConfig {
            namespace: "bad namespace!".into(),
            ..EngineConfig::default()
        };
        assert!(c.validated().is_err());

        let blank = EngineConfig {
            namespace: "   ".into(),
            ..EngineConfig::default()
        };
        assert!(blank.validated().is_err());
    }

    #[test]
    fn limits_must_be_positive() {
        let c = EngineConfig {
            limits: LimitsConfig {
                max_batch_size: 0,
                ..LimitsConfig::default()
            },
            ..EngineConfig::default()
        };
        assert!(c.validated().is_err());
    }
}
