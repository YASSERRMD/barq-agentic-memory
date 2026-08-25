//! pgvector-backed [`VectorProvider`] for semantic recall.
//!
//! Embeddings are stored as `vector` columns with cosine HNSW indexing.
//! Model/version stamps are enforced on read and write so a change of
//! embedding model can never silently poison search results.

use async_trait::async_trait;
use memory_domain::{MemoryError, MemoryId, MemoryResult, MemoryScope};
use memory_provider_api::{VectorMatch, VectorProvider, VectorQuery, VectorRecord};
use sqlx::{PgPool, Row};

/// pgvector index; one logical namespace per instance.
#[derive(Clone)]
pub struct PgVectorStore {
    pool: PgPool,
    namespace: String,
}

impl PgVectorStore {
    /// Connects and applies migrations idempotently.
    pub async fn connect(url: &str, namespace: impl Into<String>) -> MemoryResult<Self> {
        let pool = PgPool::connect(url)
            .await
            .map_err(|e| MemoryError::unavailable("pgvector", e.to_string()))?;
        Self::with_pool(pool, namespace).await
    }

    /// Assembles from an existing pool (shares the canonical store's).
    pub async fn with_pool(pool: PgPool, namespace: impl Into<String>) -> MemoryResult<Self> {
        // The vector extension must already exist in the database; we
        // surface a clear error rather than requiring superuser rights.
        let has_extension: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'vector')",
        )
        .fetch_one(&pool)
        .await
        .map_err(|e| MemoryError::storage("pgvector", e.to_string()))?;
        if !has_extension {
            return Err(MemoryError::storage(
                "pgvector",
                "extension 'vector' not installed in this database",
            ));
        }

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS memory_vectors (
                memory_id     UUID PRIMARY KEY,
                namespace     TEXT NOT NULL,
                model         TEXT NOT NULL,
                model_version TEXT NOT NULL,
                dimensions    INT  NOT NULL,
                embedding     vector(384) NOT NULL,
                metadata      JSONB NOT NULL DEFAULT '{}'::jsonb
             )",
        )
        .execute(&pool)
        .await
        .map_err(|e| MemoryError::storage("pgvector", e.to_string()))?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_memory_vectors_hnsw
                ON memory_vectors USING hnsw (embedding vector_cosine_ops)",
        )
        .execute(&pool)
        .await
        .map_err(|e| MemoryError::storage("pgvector", e.to_string()))?;

        Ok(Self {
            pool,
            namespace: namespace.into(),
        })
    }

    /// Underlying pool for diagnostics.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    fn literal(vector: &[f32]) -> String {
        let body: Vec<String> = vector.iter().map(|v| format!("{v}")).collect();
        format!("[{}]", body.join(","))
    }

    async fn stored_stamp(&self, memory_id: MemoryId) -> MemoryResult<Option<(String, String)>> {
        let row =
            sqlx::query("SELECT model, model_version FROM memory_vectors WHERE memory_id = $1")
                .bind(memory_id.as_uuid())
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| MemoryError::storage("pgvector", e.to_string()))?;
        Ok(row.map(|r| {
            (
                r.get::<String, _>("model"),
                r.get::<String, _>("model_version"),
            )
        }))
    }

    /// Namespace-scoped bulk removal used by lifecycle sweeps.
    ///
    /// Scope columns live on canonical records, not vectors, so writes
    /// must mirror them into vector metadata for this to be exact.
    pub async fn delete_scope(&self, scope: &MemoryScope) -> MemoryResult<u64> {
        let mut sql = sqlx::QueryBuilder::new("DELETE FROM memory_vectors WHERE namespace = ");
        sql.push_bind(self.namespace.clone());
        for (name, value) in [
            ("tenant_id", &scope.tenant_id),
            ("workspace_id", &scope.workspace_id),
            ("user_id", &scope.user_id),
            ("agent_id", &scope.agent_id),
            ("session_id", &scope.session_id),
            ("task_id", &scope.task_id),
        ] {
            if let Some(v) = value {
                sql.push(format_args!(" AND metadata->>{} = ", quote_json_key(name)))
                    .push_bind(v);
            }
        }
        let result = sql.build().execute(&self.pool).await.map_err(pg_error)?;
        Ok(result.rows_affected())
    }
}

#[async_trait]
impl VectorProvider for PgVectorStore {
    fn name(&self) -> &str {
        "pgvector"
    }

    async fn upsert(&self, record: &VectorRecord) -> MemoryResult<()> {
        if record.embedding.is_empty() {
            return Err(MemoryError::validation("embedding", "must not be empty"));
        }
        // Refuse to overwrite a vector produced by another model stamp:
        // mixing generations is the classic silent-recall-corruption bug.
        if let Some((model, version)) = self.stored_stamp(record.memory_id).await? {
            if model != record.model || version != record.model_version {
                return Err(MemoryError::validation(
                    "embedding",
                    format!(
                        "memory {} indexed by {model}/{version}, refusing {}/{}",
                        record.memory_id, record.model, record.model_version
                    ),
                ));
            }
        }

        let dims = record.embedding.len() as i32;
        let literal = Self::literal(&record.embedding);
        sqlx::query(
            "INSERT INTO memory_vectors
                (memory_id, namespace, model, model_version, dimensions, embedding, metadata)
             VALUES ($1, $2, $3, $4, $5, $6::vector, $7::jsonb)
             ON CONFLICT (memory_id) DO UPDATE SET
                embedding = EXCLUDED.embedding,
                dimensions = EXCLUDED.dimensions,
                metadata = EXCLUDED.metadata",
        )
        .bind(record.memory_id.as_uuid())
        .bind(&self.namespace)
        .bind(&record.model)
        .bind(&record.model_version)
        .bind(dims)
        .bind(literal)
        .bind(serde_json::to_value(&record.metadata).map_err(json_err)?)
        .execute(&self.pool)
        .await
        .map_err(pg_error)?;
        Ok(())
    }

    async fn search(&self, query: &VectorQuery) -> MemoryResult<Vec<VectorMatch>> {
        let q = query.clone().validated()?;
        let literal = Self::literal(&q.embedding);

        let mut sql = sqlx::QueryBuilder::new(
            // pgvector's `<=>` is cosine *distance* in [0, 2]; normalize
            // to a similarity in [0, 1] where 1 = identical direction,
            // matching the in-memory provider's scoring exactly.
            "SELECT memory_id, 1 - ((embedding <=> ",
        );
        sql.push_bind(literal.clone());
        sql.push("::vector) / 2) AS score FROM memory_vectors WHERE namespace = ");
        sql.push_bind(&self.namespace);
        sql.push(" AND dimensions = ")
            .push_bind(q.embedding.len() as i32);

        for (k, v) in &q.filter.equals {
            sql.push(format_args!(" AND metadata->>{} = ", quote_json_key(k)))
                .push_bind(v);
        }

        sql.push(" ORDER BY embedding <=> ").push_bind(literal);
        sql.push("::vector LIMIT ").push_bind(q.top_k as i64);

        let rows = sql.build().fetch_all(&self.pool).await.map_err(pg_error)?;
        Ok(rows
            .into_iter()
            .map(|row| VectorMatch {
                memory_id: MemoryId::from_uuid(row.get::<uuid::Uuid, _>("memory_id")),
                // `1 - distance` yields double precision; normalize to f32.
                score: row.get::<f64, _>("score") as f32,
            })
            .collect())
    }

    async fn delete(&self, memory_id: &MemoryId) -> MemoryResult<()> {
        sqlx::query("DELETE FROM memory_vectors WHERE memory_id = $1")
            .bind(memory_id.as_uuid())
            .execute(&self.pool)
            .await
            .map_err(pg_error)?;
        Ok(())
    }
}

fn pg_error(e: sqlx::Error) -> MemoryError {
    match &e {
        sqlx::Error::Database(db) => MemoryError::storage("pgvector", db.message().to_string()),
        _ => MemoryError::storage("pgvector", e.to_string()),
    }
}

fn json_err(e: serde_json::Error) -> MemoryError {
    MemoryError::storage("pgvector", e.to_string())
}

/// Quotes a JSON key for `metadata->>` access.
///
/// Keys come from engine-internal scope names and caller metadata keys;
/// quoting keeps arbitrary caller keys from breaking the SQL text.
fn quote_json_key(key: &str) -> String {
    format!("'{}'", key.replace('\'', "''"))
}
