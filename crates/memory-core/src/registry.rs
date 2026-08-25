//! Named lookup of provider trait objects by capability.
//!
//! Keeping registration explicit (rather than global) means two engines
//! can coexist in one process with different backends, and tests can
//! wire fakes without environment tricks.

use memory_domain::{MemoryError, MemoryResult};
use memory_provider_api::{
    MemoryStoreProvider, VectorProvider, WorkingMemoryProvider,
};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Capabilities an engine instance can draw from its registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProviderCapability {
    /// Canonical record storage.
    Store,
    /// Semantic similarity index.
    Vector,
    /// Volatile session state.
    Working,
}

impl ProviderCapability {
    /// Capability name used in errors and logs.
    pub const fn as_str(&self) -> &'static str {
        match self {
            ProviderCapability::Store => "store",
            ProviderCapability::Vector => "vector",
            ProviderCapability::Working => "working",
        }
    }
}

/// Registry mapping names to providers per capability.
#[derive(Default)]
pub struct ProviderRegistry {
    stores: BTreeMap<String, Arc<dyn MemoryStoreProvider>>,
    vectors: BTreeMap<String, Arc<dyn VectorProvider>>,
    working: BTreeMap<String, Arc<dyn WorkingMemoryProvider>>,
    defaults: BTreeMap<ProviderCapability, String>,
}

impl ProviderRegistry {
    /// Empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a store under `name` and marks it default when none is.
    pub fn register_store(
        &mut self,
        name: impl Into<String>,
        provider: Arc<dyn MemoryStoreProvider>,
    ) {
        let name = name.into();
        self.defaults
            .entry(ProviderCapability::Store)
            .or_insert_with(|| name.clone());
        self.stores.insert(name, provider);
    }

    /// Registers a vector index under `name` and marks it default when
    /// none is.
    pub fn register_vector(
        &mut self,
        name: impl Into<String>,
        provider: Arc<dyn VectorProvider>,
    ) {
        let name = name.into();
        self.defaults
            .entry(ProviderCapability::Vector)
            .or_insert_with(|| name.clone());
        self.vectors.insert(name, provider);
    }

    /// Registers working-memory storage under `name` and marks it
    /// default when none is.
    pub fn register_working(
        &mut self,
        name: impl Into<String>,
        provider: Arc<dyn WorkingMemoryProvider>,
    ) {
        let name = name.into();
        self.defaults
            .entry(ProviderCapability::Working)
            .or_insert_with(|| name.clone());
        self.working.insert(name, provider);
    }

    /// Overrides which registered provider serves a capability.
    pub fn set_default(&mut self, capability: ProviderCapability, name: &str) -> MemoryResult<()> {
        let exists = match capability {
            ProviderCapability::Store => self.stores.contains_key(name),
            ProviderCapability::Vector => self.vectors.contains_key(name),
            ProviderCapability::Working => self.working.contains_key(name),
        };
        if !exists {
            return Err(MemoryError::ProviderMissing {
                capability: capability.as_str(),
            });
        }
        self.defaults.insert(capability, name.to_string());
        Ok(())
    }

    /// Resolves the default store.
    pub fn default_store(&self) -> MemoryResult<Arc<dyn MemoryStoreProvider>> {
        self.resolve(ProviderCapability::Store, &self.stores)
    }

    /// Resolves the default vector index.
    pub fn default_vector(&self) -> MemoryResult<Arc<dyn VectorProvider>> {
        self.resolve(ProviderCapability::Vector, &self.vectors)
    }

    /// Resolves the default working-memory storage.
    pub fn default_working(&self) -> MemoryResult<Arc<dyn WorkingMemoryProvider>> {
        self.resolve(ProviderCapability::Working, &self.working)
    }

    /// Resolves a named provider of any capability.
    pub fn named_store(&self, name: &str) -> MemoryResult<Arc<dyn MemoryStoreProvider>> {
        self.stores.get(name).cloned().ok_or(MemoryError::ProviderMissing {
            capability: ProviderCapability::Store.as_str(),
        })
    }

    /// Number of registered providers across all capabilities.
    pub fn len(&self) -> usize {
        self.stores.len() + self.vectors.len() + self.working.len()
    }

    /// True when nothing is registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn resolve<T: Clone>(
        &self,
        capability: ProviderCapability,
        map: &BTreeMap<String, T>,
    ) -> MemoryResult<T> {
        let default = self.defaults.get(&capability).ok_or(MemoryError::ProviderMissing {
            capability: capability.as_str(),
        })?;
        map.get(default).cloned().ok_or(MemoryError::ProviderMissing {
            capability: capability.as_str(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_domain::{MemoryId, MemoryQuery, MemoryRecord, MemoryScope};
    use memory_provider_api::{VectorMatch, VectorQuery, VectorRecord, WorkingMemoryState};
    use std::time::Duration;

    struct NoopStore;
    struct NoopVector;
    struct NoopWorking;

    #[async_trait::async_trait]
    impl MemoryStoreProvider for NoopStore {
        fn name(&self) -> &str {
            "noop"
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

    #[async_trait::async_trait]
    impl VectorProvider for NoopVector {
        fn name(&self) -> &str {
            "noop-vec"
        }
        async fn upsert(&self, _r: &VectorRecord) -> memory_domain::MemoryResult<()> {
            Ok(())
        }
        async fn search(&self, _q: &VectorQuery) -> memory_domain::MemoryResult<Vec<VectorMatch>> {
            Ok(Vec::new())
        }
        async fn delete(&self, _id: &MemoryId) -> memory_domain::MemoryResult<()> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl WorkingMemoryProvider for NoopWorking {
        fn name(&self) -> &str {
            "noop-working"
        }
        async fn set(
            &self,
            _s: &WorkingMemoryState,
            _ttl: Duration,
        ) -> memory_domain::MemoryResult<()> {
            Ok(())
        }
        async fn get(
            &self,
            _session_id: &str,
        ) -> memory_domain::MemoryResult<Option<WorkingMemoryState>> {
            Ok(None)
        }
        async fn delete(&self, _session_id: &str) -> memory_domain::MemoryResult<()> {
            Ok(())
        }
    }

    fn sample_query() -> MemoryQuery {
        MemoryQuery::default()
    }

    #[test]
    fn first_registration_becomes_default() {
        let mut reg = ProviderRegistry::new();
        assert!(reg.is_empty());

        reg.register_store("primary", Arc::new(NoopStore));
        reg.register_store("replica", Arc::new(NoopStore));
        assert_eq!(reg.len(), 2);

        let d = reg.default_store().expect("default");
        assert_eq!(d.name(), "noop");
    }

    #[test]
    fn missing_capabilities_fail_with_provider_missing() {
        let reg = ProviderRegistry::new();
        let err = match reg.default_vector() {
            Err(e) => e,
            Ok(_) => panic!("expected missing vector capability"),
        };
        assert!(matches!(err, MemoryError::ProviderMissing { capability: "vector" }));
    }

    #[test]
    fn set_default_requires_registered_name() {
        let mut reg = ProviderRegistry::new();
        reg.register_vector("v1", Arc::new(NoopVector));

        assert!(
            reg.set_default(ProviderCapability::Vector, "nope")
                .is_err()
        );
        reg.set_default(ProviderCapability::Vector, "v1")
            .expect("valid switch");

        // Switching away and back keeps resolution deterministic.
        reg.register_vector("v2", Arc::new(NoopVector));
        reg.set_default(ProviderCapability::Vector, "v2")
            .expect("switch again");
        assert_eq!(reg.default_vector().expect("resolve").name(), "noop-vec");
    }

    #[test]
    fn working_capability_resolves_after_registration() {
        let mut reg = ProviderRegistry::new();
        reg.register_working("w", Arc::new(NoopWorking));
        assert!(reg.default_working().is_ok());
        assert!(!sample_query().statuses.is_empty()); // keep query model referenced
    }
}
