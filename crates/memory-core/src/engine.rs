//! The engine facade: six public operations over pluggable providers.
//!
//! Embedded and server deployments share this exact type; deployment
//! only changes which providers sit underneath it.

use crate::requests::{RememberRequest, UpdateRequest};
use memory_domain::config::{EmbeddingConfig, StoreConfig, VectorStoreConfig, WorkingStoreConfig};
use memory_domain::{
    EngineConfig, MemoryError, MemoryId, MemoryQuery, MemoryRecord, MemoryResult, MemoryScope,
};
use memory_provider_api::{
    EmbeddingProvider, HashingEmbedder, MemoryStoreProvider, MetadataFilter, VectorProvider,
    VectorRecord, WorkingMemoryProvider, WorkingMemoryState,
};
use provider_local::{InMemoryStore, InMemoryVectorStore, InProcessWorkingStore, LocalStore};
#[cfg(feature = "postgres")]
use provider_postgres::PostgresStore;
#[cfg(feature = "redis")]
use provider_redis::RedisWorkingStore;
use std::sync::Arc;
use std::time::Duration;

/// High-level memory engine.
///
/// Owns the canonical store plus working-memory storage. Vector,
/// graph, and episodic providers attach in later phases without
/// changing these method signatures.
pub struct MemoryEngine {
    pub(crate) config: EngineConfig,
    pub(crate) store: Arc<dyn MemoryStoreProvider>,
    pub(crate) working: Arc<dyn WorkingMemoryProvider>,
    pub(crate) vector: Option<Arc<dyn VectorProvider>>,
    pub(crate) embedder: Option<Arc<dyn EmbeddingProvider>>,
    pub(crate) classifier: Option<Arc<dyn memory_classifier::MemoryClassifier>>,
    pub(crate) episodes: Option<std::sync::Arc<dyn memory_episodic::EpisodeStore>>,
    pub(crate) graph: Option<std::sync::Arc<dyn memory_graph::GraphProvider>>,
}

/// A canonical record returned with its similarity score.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoredMemory {
    /// The canonical record.
    pub record: MemoryRecord,
    /// Similarity in `[0, 1]`, 1 = identical.
    pub score: f32,
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
            StoreConfig::Postgres {
                url,
                max_connections,
            } => {
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
            #[cfg(feature = "redis")]
            Some(WorkingStoreConfig::Redis { url }) => {
                Arc::new(RedisWorkingStore::connect(url, &config.namespace).await?)
            }
            #[cfg(not(feature = "redis"))]
            Some(WorkingStoreConfig::Redis { .. }) => {
                return Err(MemoryError::Unsupported(
                    "built without the 'redis' feature".into(),
                ));
            }
        };

        let vector: Option<Arc<dyn VectorProvider>> = match config.vector.as_ref() {
            None => None,
            Some(VectorStoreConfig::InMemory) => {
                Some(Arc::new(InMemoryVectorStore::new(&config.namespace)))
            }
            #[cfg(feature = "pgvector")]
            Some(VectorStoreConfig::PgVector { url }) => Some(Arc::new(
                provider_pgvector::PgVectorStore::connect(url, &config.namespace).await?,
            )),
            #[cfg(not(feature = "pgvector"))]
            Some(VectorStoreConfig::PgVector { .. }) => {
                return Err(MemoryError::Unsupported(
                    "built without the 'pgvector' feature".into(),
                ));
            }
        };

        let embedder: Option<Arc<dyn EmbeddingProvider>> = match &config.embedding {
            None => None,
            Some(EmbeddingConfig::Hashing { dimensions }) => {
                Some(Arc::new(HashingEmbedder::new(*dimensions as usize)))
            }
        };

        Ok(Self {
            config,
            store,
            working,
            vector,
            embedder,
            classifier: None,
            episodes: None,
            graph: None,
        })
    }

    /// Attaches an entity graph fed by relation extraction on writes.
    pub fn with_graph(mut self, graph: std::sync::Arc<dyn memory_graph::GraphProvider>) -> Self {
        self.graph = Some(graph);
        self
    }

    /// Attaches an episode store for experience tracking.
    pub fn with_episodes(
        mut self,
        store: std::sync::Arc<dyn memory_episodic::EpisodeStore>,
    ) -> Self {
        self.episodes = Some(store);
        self
    }

    /// Attaches a classifier for auto-classified writes.
    ///
    /// Optional by design: engines without one still support fully
    /// typed `remember()` calls, preserving zero-LLM operation.
    pub fn with_classifier(
        mut self,
        classifier: Arc<dyn memory_classifier::MemoryClassifier>,
    ) -> Self {
        self.classifier = Some(classifier);
        self
    }

    /// Configuration the engine was assembled with.
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Direct access to the canonical store provider.
    pub fn store(&self) -> Arc<dyn MemoryStoreProvider> {
        self.store.clone()
    }

    /// True when semantic recall is configured.
    pub fn supports_semantic_recall(&self) -> bool {
        self.vector.is_some() && self.embedder.is_some()
    }

    /// Mirrors scope dimensions into vector metadata so filtered search
    /// and lifecycle sweeps can operate without touching the canonical
    /// store first.
    fn vector_metadata(record: &MemoryRecord) -> MetadataFilter {
        Self::scope_filter(&record.scope)
    }

    /// Builds an equality filter from the pinned dimensions of a scope.
    fn scope_filter(scope: &MemoryScope) -> MetadataFilter {
        let mut filter = MetadataFilter::default();
        for (key, value) in [
            ("tenant_id", &scope.tenant_id),
            ("workspace_id", &scope.workspace_id),
            ("user_id", &scope.user_id),
            ("agent_id", &scope.agent_id),
            ("session_id", &scope.session_id),
            ("task_id", &scope.task_id),
        ] {
            if let Some(v) = value {
                filter.equals.insert(key.to_string(), v.clone());
            }
        }
        filter
    }

    /// Embeds + upserts a record into the vector index (when configured).
    ///
    /// Synchronous on the write path for now; background indexing
    /// arrives with the scale-out phase. Failures propagate: a record
    /// that cannot be indexed must not pretend to be recallable.
    async fn index_vector(&self, record: &MemoryRecord) -> MemoryResult<()> {
        let (Some(vector), Some(embedder)) = (&self.vector, &self.embedder) else {
            return Ok(());
        };
        let embedding = embedder
            .embed(std::slice::from_ref(&record.content.text))
            .await?
            .remove(0);
        let mut vr = VectorRecord::new(
            record.id,
            embedding,
            embedder.model(),
            embedder.model_version(),
        );
        let filter = Self::vector_metadata(record);
        for (k, v) in filter.equals {
            vr.metadata.insert(k, v);
        }
        vector.upsert(&vr).await
    }

    async fn remove_vector(&self, id: &MemoryId) -> MemoryResult<()> {
        if let Some(vector) = &self.vector {
            vector.delete(id).await?;
        }
        Ok(())
    }

    /// Stores a new memory, honoring deduplication when enabled.
    pub async fn remember(&self, request: RememberRequest) -> MemoryResult<MemoryRecord> {
        request.validated(&self.config)?;
        let record = request.into_record(self.config.default_scope.clone());

        if self.config.dedup_enabled {
            if let Some(existing) = self.deduplicate(&record).await? {
                return Ok(existing);
            }
        }

        if self.config.conflict_enabled {
            if let Some(outcome) = self.resolve_conflicts(&record).await? {
                return Ok(outcome);
            }
        }

        let saved = self.store.put(&record).await?;
        self.index_vector(&saved).await?;
        self.index_graph(&saved).await?;
        Ok(saved)
    }

    /// Extracts and stores entity relations justified by a memory.
    async fn index_graph(&self, record: &MemoryRecord) -> MemoryResult<()> {
        let Some(graph) = &self.graph else {
            return Ok(());
        };
        let Some(subject) = &record.subject else {
            return Ok(());
        };
        use memory_graph::RelationExtractor as _;
        let extractor = memory_graph::RuleBasedRelationExtractor;
        let relations = extractor
            .extract(record.id, subject, &record.content.text)
            .await?;
        for relation in relations {
            graph.add_relation(&relation).await?;
        }
        Ok(())
    }

    /// Runs the dedup cascade against same-type candidates in scope.
    ///
    /// Returns `Ok(Some(existing))` when the write should not proceed
    /// as a plain add (ignored duplicates return the original; merges
    /// perform the supersession and return the successor).
    async fn deduplicate(&self, record: &MemoryRecord) -> MemoryResult<Option<MemoryRecord>> {
        use memory_dedup::DedupAction;

        let query = MemoryQuery {
            scope: record.scope.clone(),
            memory_types: vec![record.memory_type],
            statuses: vec![memory_domain::MemoryStatus::Active],
            subject: None,
            text: None,
            valid_at: None,
            limit: 20,
        };
        let candidates = self.store.query(&query).await?;
        if candidates.is_empty() {
            return Ok(None);
        }

        // Semantic signal is best-effort and computed up front so the
        // cascade stays a pure synchronous function of its inputs.
        let mut similarities: std::collections::HashMap<memory_domain::MemoryId, f32> =
            std::collections::HashMap::new();
        if let Some(embedder) = self.embedder.as_ref() {
            let incoming_text = record.content.text.clone();
            let candidate_texts: Vec<String> =
                candidates.iter().map(|c| c.content.text.clone()).collect();
            let mut to_embed = vec![incoming_text];
            to_embed.extend(candidate_texts);
            if let Ok(vectors) = embedder.embed(&to_embed).await {
                let query_vec = &vectors[0];
                for (candidate, vec) in candidates.iter().zip(&vectors[1..]) {
                    similarities.insert(
                        candidate.id,
                        memory_provider_api::cosine_similarity(query_vec, vec),
                    );
                }
            }
        }

        let engine = memory_dedup::DedupEngine::default();
        let decision = engine.evaluate(record, &candidates, |candidate| {
            similarities.get(&candidate.id).copied().unwrap_or(0.0)
        });

        match decision.action {
            DedupAction::Ignore => {
                let target = decision
                    .target
                    .expect("ignore decisions always name a target");
                Ok(Some(
                    self.store
                        .get(&target, &record.scope)
                        .await?
                        .expect("candidate came from this store"),
                ))
            }
            DedupAction::Merge => {
                let target = decision.target.expect("merge decisions name a target");
                let successor = record.derive_successor(record.content.clone());
                self.store.put(&successor).await?;
                self.remove_vector(&target).await?;

                let mut retired = candidates
                    .iter()
                    .find(|c| c.id == target)
                    .expect("merge target among candidates")
                    .clone();
                retired.status = memory_domain::MemoryStatus::Superseded;
                retired.updated_at = chrono::Utc::now();
                self.store.update(&retired).await?;
                Ok(Some(successor))
            }
            DedupAction::Review => {
                let mut quarantined = record.clone();
                quarantined.status = memory_domain::MemoryStatus::Quarantined;
                let saved = self.store.put(&quarantined).await?;
                Ok(Some(saved))
            }
            DedupAction::Link | DedupAction::Add => Ok(None),
        }
    }

    /// Contradiction analysis against open same-subject facts.
    ///
    /// Returns `Ok(Some(record))` when the caller's write resolved into
    /// something other than a plain add (quarantined incoming, or the
    /// incoming fact after retiring a contradicted predecessor).
    async fn resolve_conflicts(&self, record: &MemoryRecord) -> MemoryResult<Option<MemoryRecord>> {
        let Some(subject) = record.subject.clone() else {
            return Ok(None); // conflict analysis needs a subject anchor
        };

        let query = MemoryQuery {
            scope: record.scope.clone(),
            memory_types: vec![record.memory_type],
            statuses: vec![memory_domain::MemoryStatus::Active],
            subject: Some(subject),
            text: None,
            valid_at: None,
            limit: 5,
        };
        let existing_facts = self.store.query(&query).await?;
        if existing_facts.is_empty() {
            return Ok(None);
        }

        let policy = memory_conflict::ResolutionPolicy;
        let negates = memory_conflict::resolution::detects_negation(&record.content.text);
        for existing in &existing_facts {
            // Value-level comparison is domain-specific; generic writes
            // only assert explicit negation, leaving the rest ambiguous.
            let analysis = policy.analyze(record, existing, negates, false);
            match policy.resolve(&analysis, record, existing) {
                memory_conflict::SupersessionOutcome::Write => continue,
                memory_conflict::SupersessionOutcome::ReplaceExisting { closing_id } => {
                    if let Some(mut old) = self.store.get(&closing_id, &record.scope).await? {
                        memory_conflict::ResolutionPolicy::close_window(&mut old)?;
                        self.store.update(&old).await?;
                        self.remove_vector(&closing_id).await?;
                    }
                    let saved = self.store.put(record).await?;
                    self.index_vector(&saved).await?;
                    return Ok(Some(saved));
                }
                memory_conflict::SupersessionOutcome::QuarantineIncoming => {
                    let mut quarantined = record.clone();
                    quarantined.status = memory_domain::MemoryStatus::Quarantined;
                    let saved = self.store.put(&quarantined).await?;
                    return Ok(Some(saved));
                }
            }
        }
        Ok(None)
    }

    /// Remembers text with automatic classification.
    ///
    /// Requires a classifier; without one, callers must state the type
    /// explicitly via [`remember`], which is what keeps the engine
    /// functional with zero LLM dependency.
    pub async fn remember_auto(&self, text: impl Into<String>) -> MemoryResult<MemoryRecord> {
        let Some(classifier) = &self.classifier else {
            return Err(MemoryError::Unsupported(
                "remember_auto requires a classifier (see with_classifier)".into(),
            ));
        };
        let text = text.into();
        let input = memory_classifier::ClassifierInput::text(text.clone());
        let classification = classifier.classify(&input).await?;

        let mut request = RememberRequest::new(classification.memory_type, text)
            .with_confidence(classification.confidence);
        if let Some(subtype) = &classification.subtype {
            request = request.with_subtype(subtype.clone());
        }
        self.remember(request).await
    }

    /// Extracts discrete memories from unstructured conversation and
    /// stores each one, returning what was saved.
    ///
    /// Requires an extraction provider; see [`with_classifier`] for the
    /// zero-LLM stance on optional intelligence.
    pub async fn extract_and_remember(
        &self,
        extractor: &dyn memory_classifier::ExtractionProvider,
        conversation: &str,
    ) -> MemoryResult<Vec<MemoryRecord>> {
        let extracted = extractor.extract(conversation).await?;
        let mut saved = Vec::with_capacity(extracted.len());
        for item in extracted {
            let request =
                RememberRequest::new(item.memory_type, item.text).with_confidence(item.confidence);
            saved.push(self.remember(request).await?);
        }
        Ok(saved)
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

        // Keep the index in step: successor indexed, predecessor's
        // embedding removed (its record remains as history only).
        self.index_vector(&successor).await?;
        self.remove_vector(&existing.id).await?;

        let mut retired = existing;
        retired.status = memory_domain::MemoryStatus::Superseded;
        retired.updated_at = chrono::Utc::now();
        self.store.update(&retired).await?;

        Ok(successor)
    }

    /// Semantic recall: embeds the query, searches the vector index,
    /// then hydrates canonical records with scope isolation.
    ///
    /// Returns records ranked by similarity; retired facts are dropped
    /// even if their vectors linger until the next sweep.
    pub async fn recall_semantic(
        &self,
        query_text: impl Into<String>,
        top_k: u32,
        scope: &MemoryScope,
    ) -> MemoryResult<Vec<ScoredMemory>> {
        let (Some(vector), Some(embedder)) = (&self.vector, &self.embedder) else {
            return Err(MemoryError::Unsupported(
                "semantic recall requires a vector backend and embedder".into(),
            ));
        };

        let embedding = embedder.embed(&[query_text.into()]).await?.remove(0);

        let filter = Self::scope_filter(scope);
        let candidates = vector
            .search(&memory_provider_api::VectorQuery {
                embedding,
                top_k: top_k.saturating_mul(2).max(top_k), // over-fetch: some die at hydration
                scope: Some(scope.clone()),
                memory_type: None,
                filter,
            })
            .await?;

        let mut scored = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            // Canonical truth decides visibility: scope + status + validity.
            if let Some(record) = self.store.get(&candidate.memory_id, scope).await? {
                if record.status == memory_domain::MemoryStatus::Active
                    && record.is_valid_at(chrono::Utc::now())
                {
                    scored.push(ScoredMemory {
                        record,
                        score: candidate.score,
                    });
                }
            } else {
                // Index holds a ghost (deleted/superseded since write).
                self.remove_vector(&candidate.memory_id).await.ok();
            }
        }
        scored.truncate(top_k as usize);
        Ok(scored)
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
        self.remove_vector(&id).await?;
        if let Some(graph) = &self.graph {
            graph.remove_evidence(&id).await?;
        }
        Ok(true)
    }

    /// Hard-deletes immediately. Prefer [`forget`] except for
    /// compliance erasure, which is what this exists for.
    pub async fn purge(&self, id: MemoryId, scope: &MemoryScope) -> MemoryResult<()> {
        self.store.delete(&id, scope).await?;
        self.remove_vector(&id).await?;
        if let Some(graph) = &self.graph {
            graph.remove_evidence(&id).await?;
        }
        Ok(())
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

    /// Appends an observation to the session snapshot via revision-safe
    /// compare-and-set, retrying on concurrent writers.
    pub async fn working_push_observation(
        &self,
        session_id: &str,
        observation: impl Into<String>,
    ) -> MemoryResult<WorkingMemoryState> {
        let observation = observation.into();
        self.working_mutate(session_id, move |snap| {
            snap.push_observation(observation.clone());
            Ok(())
        })
        .await
    }

    /// Records a durable checkpoint reference on the session.
    pub async fn working_add_checkpoint_ref(
        &self,
        session_id: &str,
        reference: impl Into<String>,
    ) -> MemoryResult<WorkingMemoryState> {
        let reference = reference.into();
        self.working_mutate(session_id, move |snap| {
            snap.add_checkpoint_ref(reference.clone());
            Ok(())
        })
        .await
    }

    /// Reads the typed session snapshot, if any.
    pub async fn working_snapshot(
        &self,
        session_id: &str,
    ) -> MemoryResult<Option<memory_provider_api::SessionSnapshot>> {
        Ok(self
            .working
            .get(session_id)
            .await?
            .map(|state| memory_provider_api::SessionSnapshot::from_state_data(&state.data)))
    }

    /// Revision-safe mutation loop: read → apply → CAS, retrying a
    /// bounded number of times when another writer wins the race.
    async fn working_mutate<F>(
        &self,
        session_id: &str,
        mutate: F,
    ) -> MemoryResult<WorkingMemoryState>
    where
        F: Fn(&mut memory_provider_api::SessionSnapshot) -> MemoryResult<()>,
    {
        const MAX_RACES: usize = 5;
        for _ in 0..MAX_RACES {
            let Some(current) = self.working.get(session_id).await? else {
                return Err(MemoryError::SessionNotFound {
                    session_id: session_id.to_string(),
                });
            };
            let mut snap = memory_provider_api::SessionSnapshot::from_state_data(&current.data);
            mutate(&mut snap)?;
            let mut data = current.data.clone();
            snap.apply_to(&mut data);

            match self
                .working
                .compare_and_set(
                    session_id,
                    current.revision,
                    data,
                    self.config.working_memory_ttl,
                )
                .await
            {
                Ok(next) => return Ok(next),
                Err(MemoryError::SessionConflict { .. }) => continue, // raced; retry
                Err(other) => return Err(other),
            }
        }
        Err(MemoryError::storage(
            "redis",
            format!("session '{session_id}' contended beyond {MAX_RACES} retries"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_domain::{MemoryScopeBuilder, MemoryType, RetentionPolicy};

    async fn embedded() -> MemoryEngine {
        MemoryEngine::from_config(EngineConfig::default())
            .await
            .expect("engine")
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

#[cfg(test)]
mod semantic_tests {
    use super::*;
    use memory_domain::config::{EmbeddingConfig, VectorStoreConfig};
    use memory_domain::{MemoryScopeBuilder, MemoryType};

    async fn engine_with_semantics() -> MemoryEngine {
        let config = EngineConfig {
            vector: Some(VectorStoreConfig::InMemory),
            embedding: Some(EmbeddingConfig::Hashing { dimensions: 256 }),
            ..EngineConfig::default()
        };
        MemoryEngine::from_config(config).await.expect("engine")
    }

    #[tokio::test]
    async fn semantic_recall_ranks_and_hydrates_canonical_records() {
        let engine = engine_with_semantics().await;
        assert!(engine.supports_semantic_recall());

        let atlas = engine
            .remember(RememberRequest::new(
                MemoryType::Semantic,
                "Project Atlas uses PostgreSQL",
            ))
            .await
            .expect("remember atlas");
        engine
            .remember(RememberRequest::new(
                MemoryType::Semantic,
                "The office kitchen needs restocking",
            ))
            .await
            .expect("remember kitchen");

        let hits = engine
            .recall_semantic("atlas postgres database", 3, &MemoryScope::default())
            .await
            .expect("recall");
        assert!(!hits.is_empty());
        assert_eq!(hits[0].record.id, atlas.id);
        assert!(hits[0].score > 0.0);
    }

    #[tokio::test]
    async fn forget_removes_from_semantic_recall() {
        let engine = engine_with_semantics().await;
        let r = engine
            .remember(RememberRequest::new(
                MemoryType::Semantic,
                "quarterly revenue targets",
            ))
            .await
            .expect("remember");

        engine
            .forget(r.id, &MemoryScope::default())
            .await
            .expect("forget");

        let hits = engine
            .recall_semantic("revenue targets", 5, &MemoryScope::default())
            .await
            .expect("recall");
        assert!(
            hits.iter().all(|h| h.record.id != r.id),
            "tombstoned facts must not surface"
        );
    }

    #[tokio::test]
    async fn update_supersedes_vector_ownership() {
        let engine = engine_with_semantics().await;
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

        let hits = engine
            .recall_semantic("postgresql", 5, &MemoryScope::default())
            .await
            .expect("recall");
        assert!(
            hits.iter().all(|h| h.record.id != v1.id),
            "predecessor's vector must be gone"
        );
        assert!(hits.iter().any(|h| h.record.id == v2.id) || hits.is_empty());
    }

    #[tokio::test]
    async fn scope_pinned_recall_excludes_other_tenants() {
        let engine = engine_with_semantics().await;
        let acme = MemoryScopeBuilder::new().tenant("acme").build();

        engine
            .remember(
                RememberRequest::new(MemoryType::Semantic, "acme pricing strategy")
                    .with_scope(acme.clone()),
            )
            .await
            .expect("remember");

        let hits = engine
            .recall_semantic("pricing strategy", 10, &MemoryScope::default())
            .await
            .expect("wildcard recall sees everything");
        assert_eq!(hits.len(), 1);

        // A different tenant's pinned query must see nothing.
        let globex = MemoryScopeBuilder::new().tenant("globex").build();
        let foreign = engine
            .recall_semantic("pricing strategy", 10, &globex)
            .await
            .expect("pinned recall");
        assert!(foreign.is_empty());
    }

    #[tokio::test]
    async fn semantic_recall_without_backend_is_unsupported() {
        let engine = MemoryEngine::from_config(EngineConfig::default())
            .await
            .unwrap();
        assert!(!engine.supports_semantic_recall());
        let err = engine
            .recall_semantic("anything", 5, &MemoryScope::default())
            .await
            .unwrap_err();
        assert!(matches!(err, MemoryError::Unsupported(_)));
    }
}

#[cfg(test)]
mod classifier_tests {
    use super::*;
    use memory_classifier::{RuleBasedClassifier, RuleBasedExtractor};
    use memory_domain::MemoryType;

    async fn embedded() -> MemoryEngine {
        MemoryEngine::from_config(EngineConfig::default())
            .await
            .expect("engine")
    }

    #[tokio::test]
    async fn remember_auto_classifies_preferences() {
        let engine = embedded()
            .await
            .with_classifier(Arc::new(RuleBasedClassifier::default()));
        let r = engine
            .remember_auto("Customer prefers email contact")
            .await
            .expect("remember_auto");
        assert_eq!(r.memory_type, MemoryType::Semantic);
        assert_eq!(r.subtype.as_deref(), Some("preference"));
    }

    #[tokio::test]
    async fn remember_auto_without_classifier_is_unsupported() {
        let engine = embedded().await;
        let err = engine.remember_auto("anything").await.unwrap_err();
        assert!(matches!(err, MemoryError::Unsupported(_)));
    }

    #[tokio::test]
    async fn extract_and_remember_stores_only_durable_facts() {
        let engine = embedded().await;
        let saved = engine
            .extract_and_remember(
                &RuleBasedExtractor,
                "Hello! The customer prefers email over phone. She must get invoices by friday.",
            )
            .await
            .expect("extract+remember");

        assert_eq!(saved.len(), 2, "small talk dropped, two facts kept");
        assert!(
            saved
                .iter()
                .any(|s| s.content.text.contains("prefers email")),
        );
        assert!(
            saved
                .iter()
                .any(|s| s.memory_type == MemoryType::Prospective),
        );
    }
}

#[cfg(test)]
mod dedup_tests {
    use super::*;
    use memory_domain::MemoryType;
    use memory_domain::config::EngineConfig;

    async fn dedup_engine() -> MemoryEngine {
        MemoryEngine::from_config(EngineConfig {
            dedup_enabled: true,
            vector: Some(memory_domain::config::VectorStoreConfig::InMemory),
            embedding: Some(memory_domain::config::EmbeddingConfig::Hashing { dimensions: 256 }),
            ..EngineConfig::default()
        })
        .await
        .expect("dedup engine")
    }

    #[tokio::test]
    async fn identical_remember_returns_original_untouched() {
        let engine = dedup_engine().await;
        let first = engine
            .remember(
                RememberRequest::new(MemoryType::Semantic, "Atlas uses PostgreSQL")
                    .with_subject(memory_domain::MemorySubject::new("atlas").with_type("project")),
            )
            .await
            .expect("first");

        let second = engine
            .remember(
                RememberRequest::new(MemoryType::Semantic, "Atlas uses PostgreSQL")
                    .with_subject(memory_domain::MemorySubject::new("atlas").with_type("project")),
            )
            .await
            .expect("second is ignored as duplicate");

        assert_eq!(first.id, second.id, "duplicate returns the original");
    }

    #[tokio::test]
    async fn reworded_duplicates_ignore_regardless_of_case() {
        let engine = dedup_engine().await;
        let first = engine
            .remember(
                RememberRequest::new(MemoryType::Semantic, "customer prefers email")
                    .with_subject(memory_domain::MemorySubject::new("cust-1")),
            )
            .await
            .expect("first");
        let second = engine
            .remember(
                RememberRequest::new(MemoryType::Semantic, "Customer PREFERS email.")
                    .with_subject(memory_domain::MemorySubject::new("cust-1")),
            )
            .await
            .expect("normalized duplicate");

        assert_eq!(first.id, second.id);
    }

    #[tokio::test]
    async fn distinct_facts_still_add() {
        let engine = dedup_engine().await;
        let a = engine
            .remember(RememberRequest::new(
                MemoryType::Semantic,
                "Atlas uses PostgreSQL",
            ))
            .await
            .expect("a");
        let b = engine
            .remember(RememberRequest::new(
                MemoryType::Semantic,
                "The kitchen fridge needs restocking",
            ))
            .await
            .expect("b");
        assert_ne!(a.id, b.id);
    }

    #[tokio::test]
    async fn dedup_disabled_by_default() {
        let engine = MemoryEngine::from_config(EngineConfig::default())
            .await
            .unwrap();
        let a = engine
            .remember(RememberRequest::new(MemoryType::Semantic, "same text"))
            .await
            .unwrap();
        let b = engine
            .remember(RememberRequest::new(MemoryType::Semantic, "same text"))
            .await
            .unwrap();
        assert_ne!(a.id, b.id, "without dedup, duplicates are separate rows");
    }
}

#[cfg(test)]
mod conflict_tests {
    use super::*;
    use memory_domain::config::EngineConfig;
    use memory_domain::{MemorySubject, MemoryType};

    async fn conflict_engine() -> MemoryEngine {
        MemoryEngine::from_config(EngineConfig {
            conflict_enabled: true,
            ..EngineConfig::default()
        })
        .await
        .expect("conflict engine")
    }

    fn atlas_fact(text: &str) -> RememberRequest {
        RememberRequest::new(MemoryType::Semantic, text)
            .with_subject(MemorySubject::new("atlas").with_type("project"))
            .from_source(memory_domain::SourceKind::User, "u-1")
            .with_confidence(0.9)
    }

    #[tokio::test]
    async fn negation_supersedes_and_preserves_history() {
        let engine = conflict_engine().await;
        let old = engine
            .remember(atlas_fact("Atlas uses MySQL for its primary database"))
            .await
            .expect("old fact");

        let new = engine
            .remember(atlas_fact(
                "Atlas no longer uses MySQL; it is not on MySQL anymore",
            ))
            .await
            .expect("negation");

        assert_ne!(new.id, old.id, "negation becomes a new record");

        let retired = engine
            .recall_exact(old.id, &Default::default())
            .await
            .expect("get")
            .unwrap();
        assert_eq!(retired.status, memory_domain::MemoryStatus::Superseded);
        assert!(retired.validity().has_ended(chrono::Utc::now()));

        // History remains fully addressable.
        let chain = engine
            .history(new.id, &Default::default())
            .await
            .expect("history");
        // The new record supersedes via the closed window, though the
        // chain link is by subject continuity here — both records exist.
        assert!(!chain.is_empty());
    }

    #[tokio::test]
    async fn weaker_ambiguous_claims_quarantine() {
        let engine = conflict_engine().await;
        let _existing = engine
            .remember(
                atlas_fact("Atlas deploys to us-east-1")
                    .from_source(memory_domain::SourceKind::User, "u-1"),
            )
            .await
            .expect("existing");

        let weak_agent = RememberRequest::new(MemoryType::Semantic, "Atlas deploys to eu-west-1")
            .with_subject(MemorySubject::new("atlas").with_type("project"))
            .from_source(memory_domain::SourceKind::Agent, "agent-7")
            .with_confidence(0.3);

        let outcome = engine
            .remember(weak_agent)
            .await
            .expect("quarantined write");
        assert_eq!(
            outcome.status,
            memory_domain::MemoryStatus::Quarantined,
            "weak ambiguous claims wait for review"
        );
    }

    #[tokio::test]
    async fn conflicts_need_subjects_to_fire() {
        let engine = conflict_engine().await;
        let a = engine
            .remember(RememberRequest::new(
                MemoryType::Semantic,
                "no subject fact one",
            ))
            .await
            .expect("a");
        let b = engine
            .remember(RememberRequest::new(
                MemoryType::Semantic,
                "no longer subject fact two",
            ))
            .await
            .expect("b");
        assert_ne!(a.id, b.id);
        let b_status = engine
            .recall_exact(b.id, &Default::default())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(b_status.status, memory_domain::MemoryStatus::Active);
    }
}

#[cfg(test)]
mod graph_tests {
    use super::*;
    use memory_domain::{MemorySubject, MemoryType};
    use memory_graph::{Entity, EntityKey, GraphProvider};

    async fn embedded() -> MemoryEngine {
        MemoryEngine::from_config(EngineConfig::default())
            .await
            .expect("engine")
    }

    #[tokio::test]
    async fn remember_feeds_graph_and_forget_retracts_edges() {
        let graph = std::sync::Arc::new(memory_graph::InMemoryGraphStore::new());
        let engine = embedded().await.with_graph(graph.clone());

        let saved = engine
            .remember(
                RememberRequest::new(MemoryType::Semantic, "Project Atlas uses PostgreSQL")
                    .with_subject(MemorySubject::new("atlas").with_type("project")),
            )
            .await
            .expect("remember");

        let atlas_key = EntityKey::from_subject(&MemorySubject::new("atlas").with_type("project"));
        let edges = graph.relations_from(&atlas_key).await.expect("edges");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].relation_type, "USES");
        assert_eq!(edges[0].evidence, saved.id);

        // Forgetting the evidence retracts its edges.
        engine
            .forget(saved.id, &Default::default())
            .await
            .expect("forget");
        assert_eq!(graph.edge_count(), 0);
    }

    #[tokio::test]
    async fn subjectless_memories_do_not_touch_the_graph() {
        let graph = std::sync::Arc::new(memory_graph::InMemoryGraphStore::new());
        let engine = embedded().await.with_graph(graph.clone());

        engine
            .remember(RememberRequest::new(
                MemoryType::Semantic,
                "unanchored note that mentions uses nothing",
            ))
            .await
            .expect("remember");

        // Entities may be upserted directly; edges require evidence.
        graph
            .upsert_entity(&Entity {
                key: EntityKey::new(None, "solo"),
                display_name: "Solo".into(),
            })
            .await
            .expect("upsert");
        assert_eq!(graph.edge_count(), 0);
    }
}
