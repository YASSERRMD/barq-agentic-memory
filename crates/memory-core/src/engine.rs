//! The engine facade: six public operations over pluggable providers.
//!
//! Embedded and server deployments share this exact type; deployment
//! only changes which providers sit underneath it.

use crate::requests::{RememberRequest, UpdateRequest};
use memory_domain::config::{StoreConfig, WorkingStoreConfig};
use memory_domain::{
    EngineConfig, MemoryError, MemoryId, MemoryQuery, MemoryRecord, MemoryResult, MemoryScope,
};
use memory_provider_api::{MemoryStoreProvider, WorkingMemoryProvider, WorkingMemoryState};
#[cfg(feature = "postgres")]
use provider_postgres::PostgresStore;
use provider_local::{InMemoryStore, InProcessWorkingStore, LocalStore};
use std::sync::Arc;
use std::time::Duration;

/// High-level memory engine.
///
/// Owns the canonical store plus working-memory storage. Vector,
/// graph, and episodic providers attach in later phases without
/// changing these method signatures.
pub struct MemoryEngine {
    config: EngineConfig,
    store: Arc<dyn MemoryStoreProvider>,
    working: Arc<dyn WorkingMemoryProvider>,
}

impl MemoryEngine {
    /// Assembles an engine from configuration.
    ///
    /// Only embedded backends existed in Phase 1; PostgreSQL joins in
    /// Phase 2 behind the `postgres` feature.
    pub async fn from_config(config: EngineConfig) -> MemoryResult<Self> {
        config.validated()?;

        let store: Arc<dyn MemoryStoreProvider> = match &config.store {
            StoreConfig::Memory => Arc::new(InMemoryStore::new(&config.namespace)),
            StoreConfig::Local { path } => Arc::new(LocalStore::open(path, &config.namespace)?),
            #[cfg(feature = "postgres")]
            StoreConfig::Postgres { url, max_connections } => {
                let pool = sqlx::pool::PoolOptions::<sqlx::Postgres>::new()
                    .max_connections(*max_connections)
                    .connect(url)
                    .await
                    .map_err(|e| {
                        memory_domain::MemoryError::unavailable("postgres", e.to_string())
                    })?;
                Arc::new(PostgresStore::with_pool(pool, &config.namespace).await?)
            }
            #[cfg(not(feature = "postgres"))]
            StoreConfig::Postgres { .. } => {
                return Err(MemoryError::Unsupported(
                    "built without the 'postgres' feature".into(),
                ));
            }
        };

        let working: Arc<dyn WorkingMemoryProvider> = match config.working.as_ref() {
            None | Some(WorkingStoreConfig::InProcess) => {
                Arc::new(InProcessWorkingStore::new(&config.namespace))
            }
            Some(WorkingStoreConfig::Redis { .. }) => {
                return Err(MemoryError::Unsupported(
                    "redis working store lands in phase 03".into(),
                ));
            }
        };

        Ok(Self {
            config,
            store,
            working,
        })
    }

    /// Configuration the engine was assembled with.
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Direct access to the canonical store provider.
    pub fn store(&self) -> Arc<dyn MemoryStoreProvider> {
        self.store.clone()
    }

    /// Stores a new memory and returns the canonical record.
    pub async fn remember(&self, request: RememberRequest) -> MemoryResult<MemoryRecord> {
        request.validated(&self.config)?;
        let record = request.into_record(self.config.default_scope.clone());
        self.store.put(&record).await
    }

    /// Exact lookup by identifier within a scope.
    pub async fn recall_exact(
        &self,
        id: MemoryId,
        scope: &MemoryScope,
    ) -> MemoryResult<Option<MemoryRecord>> {
        self.store.get(&id, scope).await
    }

    /// Filtered lookup (type, status, subject, keyword, temporal).
    ///
    /// Semantic similarity joins this path in later phases; callers
    /// use the same query shape either way.
    pub async fn search(&self, mut query: MemoryQuery) -> MemoryResult<Vec<MemoryRecord>> {
        query = query.validated()?;
        if query.limit > self.config.limits.max_batch_size.min(u32::MAX as usize) as u32 {
            // Batch ceiling doubles as a sane result budget for MVP.
            return Err(MemoryError::validation(
                "limit",
                format!(
                    "exceeds engine max_batch_size ({})",
                    self.config.limits.max_batch_size
                ),
            ));
        }
        self.store.query(&query).await
    }

    /// Replaces content by deriving a successor; history is preserved.
    ///
    /// Returns the new record. The predecessor is retired to
    /// [`memory_domain::MemoryStatus::Superseded`].
    pub async fn update(&self, request: UpdateRequest) -> MemoryResult<MemoryRecord> {
        if request.content.is_empty() {
            return Err(MemoryError::validation("content", "must not be empty"));
        }
        let existing =
            self.store
                .get(&request.id, &request.scope)
                .await?
                .ok_or(MemoryError::NotFound {
                    memory_id: request.id,
                })?;

        let mut successor = existing.derive_successor(request.content);
        successor.scope = existing.scope.clone();
        if let Some(c) = request.confidence {
            successor.confidence = c.clamp(0.0, 1.0);
        }
        if let Some(i) = request.importance {
            successor.importance = i.clamp(0.0, 1.0);
        }
        let successor = self.store.put(&successor).await?;

        let mut retired = existing;
        retired.status = memory_domain::MemoryStatus::Superseded;
        retired.updated_at = chrono::Utc::now();
        self.store.update(&retired).await?;

        Ok(successor)
    }

    /// Soft-deletes a memory (tombstone); physical removal happens in
    /// lifecycle sweeps. Returns whether this call changed anything.
    pub async fn forget(&self, id: MemoryId, scope: &MemoryScope) -> MemoryResult<bool> {
        let Some(mut record) = self.store.get(&id, scope).await? else {
            return Ok(false);
        };
        if record.status == memory_domain::MemoryStatus::Deleted {
            return Ok(false);
        }
        record.status = memory_domain::MemoryStatus::Deleted;
        record.updated_at = chrono::Utc::now();
        self.store.update(&record).await?;
        Ok(true)
    }

    /// Hard-deletes immediately. Prefer [`forget`] except for
    /// compliance erasure, which is what this exists for.
    pub async fn purge(&self, id: MemoryId, scope: &MemoryScope) -> MemoryResult<()> {
        self.store.delete(&id, scope).await
    }

    /// The supersession chain ending at `id`, oldest first.
    pub async fn history(
        &self,
        id: MemoryId,
        scope: &MemoryScope,
    ) -> MemoryResult<Vec<MemoryRecord>> {
        let tip = match self.store.get(&id, scope).await? {
            Some(r) => r,
            None => return Ok(Vec::new()),
        };
        let mut chain = vec![tip.clone()];
        let mut cursor = tip;
        while let Some(prev) = cursor.supersedes {
            let Some(record) = self.store.get(&prev, scope).await? else {
                break;
            };
            chain.push(record.clone());
            cursor = record;
        }
        chain.reverse();
        Ok(chain)
    }

    /// Writes session state using the configured default TTL.
    pub async fn set_working_state(
        &self,
        session_id: impl Into<String>,
        data: serde_json::Value,
    ) -> MemoryResult<()> {
        let state = WorkingMemoryState::initial(session_id, data);
        self.working
            .set(&state, self.config.working_memory_ttl)
            .await
    }

    /// Writes session state with an explicit TTL.
    pub async fn set_working_state_with_ttl(
        &self,
        session_id: impl Into<String>,
        data: serde_json::Value,
        ttl: Duration,
    ) -> MemoryResult<()> {
        let state = WorkingMemoryState::initial(session_id, data);
        self.working.set(&state, ttl).await
    }

    /// Reads live session state; expired entries vanish.
    pub async fn working_state(
        &self,
        session_id: &str,
    ) -> MemoryResult<Option<WorkingMemoryState>> {
        self.working.get(session_id).await
    }

    /// Drops session state immediately.
    pub async fn clear_working_state(&self, session_id: &str) -> MemoryResult<()> {
        self.working.delete(session_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_domain::{MemoryScopeBuilder, MemoryType, RetentionPolicy};

    async fn embedded() -> MemoryEngine {
        MemoryEngine::from_config(EngineConfig::default()).await.expect("engine")
    }

    #[tokio::test]
    async fn remember_then_search_roundtrip() {
        let engine = embedded().await;
        let saved = engine
            .remember(RememberRequest::new(
                MemoryType::Semantic,
                "Customer prefers email contact",
            ))
            .await
            .expect("remember");

        let hits = engine
            .search(MemoryQuery::default().with_text("email"))
            .await
            .expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, saved.id);
    }

    #[tokio::test]
    async fn update_creates_supersession_chain() {
        let engine = embedded().await;
        let v1 = engine
            .remember(RememberRequest::new(
                MemoryType::Semantic,
                "Atlas uses MySQL",
            ))
            .await
            .expect("remember");

        let v2 = engine
            .update(UpdateRequest::content(
                v1.id,
                MemoryScope::default(),
                "Atlas uses PostgreSQL",
            ))
            .await
            .expect("update");

        assert_eq!(v2.supersedes, Some(v1.id));

        let retired = engine
            .recall_exact(v1.id, &MemoryScope::default())
            .await
            .expect("get");
        assert_eq!(
            retired.unwrap().status,
            memory_domain::MemoryStatus::Superseded
        );

        let chain = engine
            .history(v2.id, &MemoryScope::default())
            .await
            .expect("history");
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].content.text, "Atlas uses MySQL");
        assert_eq!(chain[1].content.text, "Atlas uses PostgreSQL");

        // Default search hides retired facts but history keeps them.
        let hits = engine
            .search(MemoryQuery::default().with_text("MySQL"))
            .await
            .expect("search");
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn forget_tombstones_but_keeps_record_addressable() {
        let engine = embedded().await;
        let r = engine
            .remember(RememberRequest::new(MemoryType::Episodic, "one-off event"))
            .await
            .expect("remember");

        assert!(
            engine
                .forget(r.id, &MemoryScope::default())
                .await
                .expect("forget")
        );
        let gone = engine
            .recall_exact(r.id, &MemoryScope::default())
            .await
            .expect("get")
            .unwrap();
        assert_eq!(gone.status, memory_domain::MemoryStatus::Deleted);

        assert!(
            !engine
                .forget(r.id, &MemoryScope::default())
                .await
                .expect("forget again")
        );
    }

    #[tokio::test]
    async fn purge_physically_removes() {
        let engine = embedded().await;
        let r = engine
            .remember(RememberRequest::new(MemoryType::Working, "scratch"))
            .await
            .expect("remember");
        engine
            .purge(r.id, &MemoryScope::default())
            .await
            .expect("purge");
        assert!(
            engine
                .recall_exact(r.id, &MemoryScope::default())
                .await
                .expect("get")
                .is_none()
        );
    }

    #[tokio::test]
    async fn scope_isolation_hides_foreign_memories() {
        let engine = embedded().await;
        let acme = MemoryScopeBuilder::new().tenant("acme").build();
        let globex = MemoryScopeBuilder::new().tenant("globex").build();

        let r = engine
            .remember(
                RememberRequest::new(MemoryType::Semantic, "acme secret").with_scope(acme.clone()),
            )
            .await
            .expect("remember");

        assert!(
            engine
                .recall_exact(r.id, &globex)
                .await
                .expect("get")
                .is_none()
        );
        assert!(
            engine
                .recall_exact(r.id, &acme)
                .await
                .expect("get")
                .is_some()
        );
    }

    #[tokio::test]
    async fn working_state_expires_by_default_ttl() {
        let config = EngineConfig {
            working_memory_ttl: Duration::from_millis(30),
            ..EngineConfig::default()
        };
        let engine = MemoryEngine::from_config(config).await.expect("engine");

        engine
            .set_working_state("s-1", serde_json::json!({"goal": "deploy"}))
            .await
            .expect("set");
        assert!(engine.working_state("s-1").await.expect("get").is_some());

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(engine.working_state("s-1").await.expect("get").is_none());
    }

    #[tokio::test]
    async fn unreachable_postgres_fails_fast_with_clear_error() {
        let config = EngineConfig {
            store: StoreConfig::Postgres {
                url: "postgres://localhost:59999/none".into(),
                max_connections: 1,
            },
            ..EngineConfig::default()
        };
        let err = match MemoryEngine::from_config(config).await {
            Err(e) => e,
            Ok(_) => panic!("unreachable backend must not assemble"),
        };
        // Without the feature: Unsupported. With it: ProviderUnavailable.
        assert!(matches!(
            err,
            MemoryError::Unsupported(_) | MemoryError::ProviderUnavailable { .. }
        ));
    }

    #[tokio::test]
    async fn retention_policy_flows_through_remember() {
        let engine = embedded().await;
        let expiry = chrono::Utc::now() + chrono::Duration::hours(1);
        let r = engine
            .remember(
                RememberRequest::new(MemoryType::Working, "short-lived")
                    .with_retention(RetentionPolicy::expiring_at(expiry)),
            )
            .await
            .expect("remember");
        assert_eq!(r.retention.class, memory_domain::RetentionClass::Ephemeral);
    }

    #[tokio::test]
    async fn content_validation_surfaces_before_storage() {
        let engine = embedded().await;
        let err = engine
            .remember(RememberRequest::new(MemoryType::Semantic, ""))
            .await
            .unwrap_err();
        assert!(matches!(err, MemoryError::Validation { .. }));
    }
}
