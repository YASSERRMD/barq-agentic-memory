//! Configuration-driven assembly planning.
//!
//! Phase 0 freezes *what* must be assembled; backend constructors land
//! with their provider crates. An [`AssemblyPlan`] names the required
//! capabilities so the engine builder (Phase 1+) can fail fast when a
//! configured backend has no registered constructor.

use crate::registry::{ProviderCapability, ProviderRegistry};
use memory_domain::config::{EngineConfig, StoreConfig, VectorStoreConfig, WorkingStoreConfig};
use memory_domain::MemoryResult;

/// The capabilities a given configuration requires at assembly time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssemblyPlan {
    /// Capabilities that must resolve from the registry.
    pub required: Vec<ProviderCapability>,
    /// Capabilities that are optional and degrade gracefully.
    pub optional: Vec<ProviderCapability>,
}

impl AssemblyPlan {
    /// Derives the plan for a configuration.
    ///
    /// The canonical store is always mandatory. A configured vector or
    /// working backend becomes mandatory too: silently ignoring declared
    /// infrastructure would make recall behavior differ between
    /// environments for no visible reason.
    pub fn from_config(config: &EngineConfig) -> Self {
        let mut required = vec![ProviderCapability::Store];
        if config.vector.is_some() {
            required.push(ProviderCapability::Vector);
        }
        if config.working.is_some() {
            required.push(ProviderCapability::Working);
        }
        Self {
            required,
            optional: Vec::new(),
        }
    }

    /// True when every required capability resolves in `registry`.
    pub fn is_satisfiable(&self, registry: &ProviderRegistry) -> bool {
        self.required.iter().all(|c| registry_resolves(registry, *c))
    }

    /// Missing capabilities in registration order.
    pub fn missing(&self, registry: &ProviderRegistry) -> Vec<ProviderCapability> {
        self.required
            .iter()
            .filter(|c| !registry_resolves(registry, **c))
            .copied()
            .collect()
    }
}

fn registry_resolves(registry: &ProviderRegistry, capability: ProviderCapability) -> bool {
    match capability {
        ProviderCapability::Store => registry.default_store().is_ok(),
        ProviderCapability::Vector => registry.default_vector().is_ok(),
        ProviderCapability::Working => registry.default_working().is_ok(),
    }
}

/// Backend kind named by configuration, for diagnostics before
/// constructors exist.
pub fn describe_store(config: &StoreConfig) -> &'static str {
    match config {
        StoreConfig::Memory => "memory",
        StoreConfig::Local { .. } => "local",
        StoreConfig::Postgres { .. } => "postgres",
    }
}

/// Backend kind of the vector configuration.
pub fn describe_vector(config: &VectorStoreConfig) -> &'static str {
    match config {
        VectorStoreConfig::PgVector { .. } => "pgvector",
        VectorStoreConfig::InMemory => "in-memory",
    }
}

/// Backend kind of the working configuration.
pub fn describe_working(config: &WorkingStoreConfig) -> &'static str {
    match config {
        WorkingStoreConfig::Redis { .. } => "redis",
        WorkingStoreConfig::InProcess => "in-process",
    }
}

/// Convenience check combining plan + registry into an error.
pub fn ensure_satisfiable(
    config: &EngineConfig,
    registry: &ProviderRegistry,
) -> MemoryResult<()> {
    config.validated()?;
    let plan = AssemblyPlan::from_config(config);
    let missing = plan.missing(registry);
    match missing.first() {
        None => Ok(()),
        Some(capability) => Err(memory_domain::MemoryError::ProviderMissing {
            capability: capability.as_str(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embedded_config() -> EngineConfig {
        EngineConfig::default()
    }

    #[test]
    fn store_is_always_required() {
        let plan = AssemblyPlan::from_config(&embedded_config());
        assert_eq!(plan.required, vec![ProviderCapability::Store]);
    }

    #[test]
    fn configured_optional_backends_become_required() {
        let mut config = embedded_config();
        config.vector = Some(VectorStoreConfig::InMemory);
        config.working = Some(WorkingStoreConfig::InProcess);

        let plan = AssemblyPlan::from_config(&config);
        assert!(plan.required.contains(&ProviderCapability::Vector));
        assert!(plan.required.contains(&ProviderCapability::Working));
    }

    #[test]
    fn satisfiability_tracks_registry_state() {
        use crate::registry::ProviderRegistry;
        use memory_domain::{MemoryId, MemoryRecord, MemoryScope};
        use memory_provider_api::MemoryStoreProvider;
        use std::sync::Arc;

        struct StubStore;

        #[async_trait::async_trait]
        impl MemoryStoreProvider for StubStore {
            fn name(&self) -> &str {
                "stub"
            }
            async fn put(&self, m: &MemoryRecord) -> memory_domain::MemoryResult<MemoryRecord> {
                Ok(m.clone())
            }
            async fn get(
                &self,
                _id: &MemoryId,
                _scope: &MemoryScope,
            ) -> memory_domain::MemoryResult<Option<MemoryRecord>> {
                Ok(None)
            }
            async fn update(&self, m: &MemoryRecord) -> memory_domain::MemoryResult<MemoryRecord> {
                Ok(m.clone())
            }
            async fn delete(&self, _id: &MemoryId, _scope: &MemoryScope) -> memory_domain::MemoryResult<()> {
                Ok(())
            }
        }

        let mut reg = ProviderRegistry::new();
        let plan = AssemblyPlan::from_config(&embedded_config());
        assert!(!plan.is_satisfiable(&reg));
        assert_eq!(plan.missing(&reg), vec![ProviderCapability::Store]);

        reg.register_store("mem", Arc::new(StubStore));
        assert!(plan.is_satisfiable(&reg));
    }

    #[test]
    fn backend_descriptions_are_stable_labels() {
        assert_eq!(describe_store(&StoreConfig::Memory), "memory");
        assert_eq!(
            describe_store(&StoreConfig::Local {
                path: "/tmp/x".into()
            }),
            "local"
        );
        assert_eq!(
            describe_vector(&VectorStoreConfig::InMemory),
            "in-memory"
        );
        assert_eq!(
            describe_working(&WorkingStoreConfig::InProcess),
            "in-process"
        );
    }

    #[test]
    fn ensure_satisfiable_reports_first_missing_capability() {
        use crate::registry::ProviderRegistry;

        let mut config = embedded_config();
        config.working = Some(WorkingStoreConfig::InProcess);
        let err = ensure_satisfiable(&config, &ProviderRegistry::new()).unwrap_err();

        match err {
            memory_domain::MemoryError::ProviderMissing { capability } => {
                assert!(
                    capability == "store" || capability == "working"
                );
            }
            other => panic!("unexpected error: {other}"),
        }
    }
}
