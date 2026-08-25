//! Episode storage contract and the embedded implementation.
//!
//! PostgreSQL-backed episode persistence rides the same trait when the
//! server phase lands; the in-memory store keeps embedded mode fully
//! functional today.

use crate::episode::{Episode, EpisodeBuilder};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use memory_domain::{MemoryId, MemoryResult, MemoryScope};
use std::collections::HashMap;
use std::sync::RwLock;

/// Filter for episode retrieval.
#[derive(Clone, Debug, Default)]
pub struct EpisodeQuery {
    /// Scope partition; pinned dimensions must match.
    pub scope: MemoryScope,
    /// Events at or after this instant.
    pub from: Option<DateTime<Utc>>,
    /// Events before this instant.
    pub to: Option<DateTime<Utc>>,
    /// Only successes / only failures; None = both.
    pub success: Option<bool>,
    /// Only episodes citing this canonical memory as evidence.
    pub citing_evidence: Option<MemoryId>,
    /// Maximum results, newest first.
    pub limit: u32,
}

/// Append-mostly storage for episodes.
#[async_trait]
pub trait EpisodeStore: Send + Sync {
    /// Provider name for logs.
    fn name(&self) -> &str;

    /// Appends an episode; ids are caller-generated (UUIDv7).
    async fn append(&self, episode: &Episode) -> MemoryResult<()>;

    /// Fetches one episode by id within a scope.
    async fn get(&self, id: &MemoryId, scope: &MemoryScope)
        -> MemoryResult<Option<Episode>>;

    /// Filtered listing, newest event first.
    async fn query(&self, query: &EpisodeQuery) -> MemoryResult<Vec<Episode>>;

    /// Physical removal reserved for lifecycle sweeps (phase 14).
    async fn purge(&self, id: &MemoryId, scope: &MemoryScope) -> MemoryResult<()>;
}

/// Thread-safe embedded episode store.
#[derive(Default)]
pub struct InMemoryEpisodeStore {
    episodes: RwLock<HashMap<MemoryId, Episode>>,
}

impl InMemoryEpisodeStore {
    /// Empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of stored episodes.
    pub fn len(&self) -> usize {
        self.episodes.read().expect("poisoned").len()
    }

    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl EpisodeStore for InMemoryEpisodeStore {
    fn name(&self) -> &str {
        "in-memory"
    }

    async fn append(&self, episode: &Episode) -> MemoryResult<()> {
        if episode.action.trim().is_empty() || episode.outcome.trim().is_empty() {
            return Err(memory_domain::MemoryError::validation(
                "action/outcome",
                "must not be empty",
            ));
        }
        self.episodes
            .write()
            .expect("poisoned")
            .insert(episode.id, episode.clone());
        Ok(())
    }

    async fn get(
        &self,
        id: &MemoryId,
        scope: &MemoryScope,
    ) -> MemoryResult<Option<Episode>> {
        let guard = self.episodes.read().expect("poisoned");
        Ok(guard
            .get(id)
            .filter(|e| scope.contains(&e.scope))
            .cloned())
    }

    async fn query(&self, q: &EpisodeQuery) -> MemoryResult<Vec<Episode>> {
        let guard = self.episodes.read().expect("poisoned");
        let mut hits: Vec<Episode> = guard
            .values()
            .filter(|e| {
                if !q.scope.contains(&e.scope) {
                    return false;
                }
                if let Some(from) = q.from {
                    if e.event_time < from {
                        return false;
                    }
                }
                if let Some(to) = q.to {
                    if e.event_time >= to {
                        return false;
                    }
                }
                if let Some(success) = q.success {
                    if e.success != success {
                        return false;
                    }
                }
                if let Some(ref_id) = q.citing_evidence {
                    if !e.cites(&ref_id) {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();
        hits.sort_by(|a, b| b.event_time.cmp(&a.event_time));
        if q.limit > 0 {
            hits.truncate(q.limit as usize);
        }
        Ok(hits)
    }

    async fn purge(&self, id: &MemoryId, scope: &MemoryScope) -> MemoryResult<()> {
        let mut guard = self.episodes.write().expect("poisoned");
        if let Some(e) = guard.get(id) {
            if !scope.contains(&e.scope) {
                return Ok(()); // invisible stays invisible
            }
        }
        guard.remove(id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use memory_domain::MemoryScopeBuilder;

    fn ep(tag: &str, days_ago: i64, success: bool) -> Episode {
        let builder = Episode::builder(format!("action {tag}"), format!("outcome {tag}"))
            .at(Utc::now() - Duration::days(days_ago))
            .with_scope(MemoryScopeBuilder::new().tenant("acme").build());
        if success {
            builder.build()
        } else {
            builder.failed().build()
        }
    }

    #[tokio::test]
    async fn append_get_roundtrip_with_scope_isolation() {
        let store = InMemoryEpisodeStore::new();
        let e = ep("deploy", 1, true);
        store.append(&e).await.expect("append");
        assert_eq!(store.len(), 1);

        let acme = MemoryScopeBuilder::new().tenant("acme").build();
        assert!(store.get(&e.id, &acme).await.expect("get").is_some());

        let globex = MemoryScopeBuilder::new().tenant("globex").build();
        assert!(store.get(&e.id, &globex).await.expect("get").is_none());
    }

    #[tokio::test]
    async fn queries_filter_by_time_success_and_evidence() {
        let store = InMemoryEpisodeStore::new();
        let evidence = MemoryId::generate();
        let mut recent_fail = ep("recent", 0, false);
        recent_fail.evidence_refs.push(evidence);
        store.append(&ep("old", 30, true)).await.expect("append");
        store.append(&recent_fail).await.expect("append");

        // Time window.
        let recent_only = store
            .query(&EpisodeQuery {
                from: Some(Utc::now() - Duration::days(7)),
                ..Default::default()
            })
            .await
            .expect("query");
        assert_eq!(recent_only.len(), 1);

        // Success filter.
        let failures = store
            .query(&EpisodeQuery {
                success: Some(false),
                ..Default::default()
            })
            .await
            .expect("query");
        assert_eq!(failures.len(), 1);

        // Evidence citation.
        let cited = store
            .query(&EpisodeQuery {
                citing_evidence: Some(evidence),
                ..Default::default()
            })
            .await
            .expect("query");
        assert_eq!(cited.len(), 1);

        // Newest first ordering across the full set.
        let all = store.query(&EpisodeQuery::default()).await.expect("all");
        assert_eq!(all.len(), 2);
        assert!(all[0].event_time >= all[1].event_time);
    }

    #[tokio::test]
    async fn empty_action_or_outcome_rejected() {
        let store = InMemoryEpisodeStore::new();
        let bad = Episode::builder("  ", "outcome").build();
        assert!(store.append(&bad).await.is_err());
    }

    #[tokio::test]
    async fn purge_respects_scope_and_is_idempotent() {
        let store = InMemoryEpisodeStore::new();
        let e = ep("x", 0, true);
        store.append(&e).await.expect("append");

        let globex = MemoryScopeBuilder::new().tenant("globex").build();
        store.purge(&e.id, &globex).await.expect("invisible purge");
        assert_eq!(store.len(), 1);

        let acme = MemoryScopeBuilder::new().tenant("acme").build();
        store.purge(&e.id, &acme).await.expect("owner purge");
        store.purge(&e.id, &acme).await.expect("purge again");
        assert!(store.is_empty());
    }
}
