//! Retention sweeps with coordinated deletion.
//!
//! Blueprint contract: deletion must remove or invalidate
//! representations in canonical store, vector index, graph, cache,
//! object references, and local indexes. This sweeper coordinates all
//! providers attached to the engine; hooks observe every removal.

use crate::hooks::ArchivalHook;
use chrono::{DateTime, Utc};
use memory_domain::{
    MemoryError, MemoryId, MemoryQuery, MemoryRecord, MemoryResult, MemoryScope, RetentionClass,
};
use memory_graph::GraphProvider;
use memory_provider_api::{MemoryStoreProvider, VectorProvider};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Everything the sweep touched, for audits and tests.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SweepReport {
    /// Hard-deleted everywhere (ephemeral/session classes).
    pub purged: Vec<MemoryId>,
    /// Moved to Archived status (standard class past expiry).
    pub archived: Vec<MemoryId>,
    /// Permanent-class records skipped by design.
    pub skipped: usize,
}

impl SweepReport {
    /// Total records the sweep acted on.
    pub fn acted_on(&self) -> usize {
        self.purged.len() + self.archived.len()
    }
}

/// Provider bundle for lifecycle work.
pub struct LifecycleProviders {
    pub store: Arc<dyn MemoryStoreProvider>,
    pub vector: Option<Arc<dyn VectorProvider>>,
    pub graph: Option<Arc<dyn GraphProvider>>,
}

/// Runs retention sweeps across providers.
pub struct RetentionSweeper {
    providers: LifecycleProviders,
    hooks: Vec<Arc<dyn ArchivalHook>>,
}

/// Grace period after expiry before standard-class records archive.
const ARCHIVE_GRACE: chrono::Duration = chrono::Duration::days(7);

impl RetentionSweeper {
    /// Builds a sweeper over the given providers.
    pub fn new(providers: LifecycleProviders) -> Self {
        Self {
            providers,
            hooks: Vec::new(),
        }
    }

    /// Registers an archival hook.
    pub fn with_hook(mut self, hook: Arc<dyn ArchivalHook>) -> Self {
        self.hooks.push(hook);
        self
    }

    /// Sweeps expired records in scope.
    ///
    /// - Ephemeral/Session past expiry: purged from every representation.
    /// - Standard past expiry + grace: archived (status flip), kept.
    /// - Permanent: never touched.
    pub async fn sweep(&self, scope: &MemoryScope, now: DateTime<Utc>) -> MemoryResult<SweepReport> {
        // Candidates: anything retention could touch. History mode so
        // already-retired records are still purgeable when their
        // ephemeral TTL lapsed long ago.
        let query = MemoryQuery {
            scope: scope.clone(),
            memory_types: Vec::new(),
            statuses: memory_domain::MemoryStatus::ALL_STATUSES.to_vec(),
            subject: None,
            text: None,
            valid_at: None,
            limit: 1_000,
        };
        let candidates = match self.providers.store.query(&query).await {
            Ok(hits) => hits,
            Err(MemoryError::Unsupported(_)) => return Err(MemoryError::Unsupported(
                "attached store cannot list candidates for sweeps".into(),
            )),
            Err(other) => return Err(other),
        };

        let mut report = SweepReport::default();
        for record in candidates {
            let Some(expires_at) = record.retention.expires_at else {
                if record.retention.class == RetentionClass::Permanent
                    || matches!(record.retention.class, RetentionClass::Standard | RetentionClass::Session)
                {
                    // No deadline yet: nothing due. Permanent counted for visibility.
                    if record.retention.class == RetentionClass::Permanent && record.is_valid_at(now) {
                        report.skipped += 1;
                    }
                }
                continue;
            };
            if expires_at > now {
                continue;
            }

            match record.retention.class {
                RetentionClass::Ephemeral | RetentionClass::Session => {
                    self.run_hooks(&record).await;
                    self.coordinate_delete(&record.id, scope).await?;
                    report.purged.push(record.id);
                }
                RetentionClass::Standard => {
                    if now >= expires_at + ARCHIVE_GRACE && record.status == memory_domain::MemoryStatus::Active {
                        self.run_hooks(&record).await;
                        let mut archived = record.clone();
                        archived.status = memory_domain::MemoryStatus::Archived;
                        archived.updated_at = now;
                        self.providers.store.update(&archived).await?;
                        report.archived.push(record.id);
                    } else if record.status == memory_domain::MemoryStatus::Archived {
                        report.skipped += 1;
                    }
                }
                RetentionClass::Permanent => report.skipped += 1,
            }
        }
        Ok(report)
    }

    async fn run_hooks(&self, record: &MemoryRecord) {
        for hook in &self.hooks {
            // Hooks must not fail sweeps; per-hook errors are swallowed
            // deliberately here (documented in the trait contract).
            let _ = hook.on_archive(record).await;
        }
    }

    /// Deletes one memory from every representation at once.
    ///
    /// Order matters: downstream indexes first, canonical row last, so
    /// a crash mid-sweep can strand an index entry but never a row
    /// pointing at nothing.
    pub async fn coordinate_delete(
        &self,
        id: &MemoryId,
        scope: &MemoryScope,
    ) -> MemoryResult<()> {
        if let Some(vector) = &self.providers.vector {
            vector.delete(id).await?;
        }
        if let Some(graph) = &self.providers.graph {
            graph.remove_evidence(id).await?;
        }
        self.providers.store.delete(id, scope).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use memory_domain::{
        MemoryContent, MemoryScopeBuilder, MemoryType, RetentionPolicy,
    };
    use provider_local::{InMemoryStore, InMemoryVectorStore};

    fn expiring(class: RetentionClass, seconds: i64) -> RetentionPolicy {
        RetentionPolicy {
            class,
            expires_at: Some(Utc::now() + Duration::seconds(seconds)),
        }
    }

    #[tokio::test]
    async fn ephemeral_records_are_purged_everywhere() {
        let store = Arc::new(InMemoryStore::new("sweep"));
        let vectors = Arc::new(InMemoryVectorStore::new("sweep"));
        let graph = Arc::new(memory_graph::InMemoryGraphStore::new());

        let mut r = MemoryRecord::new(MemoryType::Working, MemoryContent::from_text("scratch"))
            .with_retention(expiring(RetentionClass::Ephemeral, -10));
        r.scope = MemoryScopeBuilder::new().tenant("acme").build();
        store.put(&r).await.unwrap();
        vectors
            .upsert(&memory_provider_api::VectorRecord::new(r.id, vec![0.1], "m", "1"))
            .await
            .unwrap();

        let sweeper = RetentionSweeper::new(LifecycleProviders {
            store: store.clone(),
            vector: Some(vectors.clone()),
            graph: Some(graph.clone()),
        });
        let report = sweeper.sweep(&MemoryScope::default(), Utc::now()).await.unwrap();

        assert_eq!(report.purged, vec![r.id]);
        assert!(store.get(&r.id, &MemoryScope::default()).await.unwrap().is_none());
        // Coordinated deletion cleared the index too.
        let hits = vectors
            .search(&memory_provider_api::VectorQuery {
                embedding: vec![0.1],
                top_k: 5,
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn permanent_records_are_untouched() {
        let store = Arc::new(InMemoryStore::new("perm"));
        let permanent = MemoryRecord::new(
            MemoryType::Semantic,
            MemoryContent::from_text("keep forever"),
        )
        .with_retention(RetentionPolicy::permanent());
        store.put(&permanent).await.unwrap();

        let sweeper = RetentionSweeper::new(LifecycleProviders {
            store: store.clone(),
            vector: None,
            graph: None,
        });
        let report = sweeper.sweep(&MemoryScope::default(), Utc::now()).await.unwrap();
        assert_eq!(report.skipped, 1);
        assert_eq!(report.acted_on(), 0);
        assert!(store.get(&permanent.id, &MemoryScope::default()).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn standard_records_archive_only_after_grace() {
        let store = Arc::new(InMemoryStore::new("std"));

        let fresh_expired = MemoryRecord::new(MemoryType::Semantic, MemoryContent::from_text("recently expired"))
            .with_retention(expiring(RetentionClass::Standard, -60));
        let long_expired = MemoryRecord::new(MemoryType::Semantic, MemoryContent::from_text("long expired"))
            .with_retention(expiring(RetentionClass::Standard, -(ARCHIVE_GRACE.num_seconds() * 2)));
        store.put(&fresh_expired).await.unwrap();
        store.put(&long_expired).await.unwrap();

        let sweeper = RetentionSweeper::new(LifecycleProviders {
            store: store.clone(),
            vector: None,
            graph: None,
        });
        let report = sweeper.sweep(&MemoryScope::default(), Utc::now()).await.unwrap();

        assert!(report.archived.contains(&long_expired.id));
        assert!(!report.archived.contains(&fresh_expired.id), "grace protects fresh expiries");

        let got = store.get(&long_expired.id, &MemoryScope::default()).await.unwrap().unwrap();
        assert_eq!(got.status, memory_domain::MemoryStatus::Archived);
        assert!(got.status.is_retrievable(), "archived stays addressable");
    }

    #[tokio::test]
    async fn future_expiries_are_left_alone() {
        let store = Arc::new(InMemoryStore::new("future"));
        let later = MemoryRecord::new(MemoryType::Working, MemoryContent::from_text("later"))
            .with_retention(expiring(RetentionClass::Ephemeral, 3_600));
        store.put(&later).await.unwrap();

        let sweeper = RetentionSweeper::new(LifecycleProviders {
            store: store.clone(),
            vector: None,
            graph: None,
        });
        let report = sweeper.sweep(&MemoryScope::default(), Utc::now()).await.unwrap();
        assert_eq!(report.acted_on(), 0);
    }

}
