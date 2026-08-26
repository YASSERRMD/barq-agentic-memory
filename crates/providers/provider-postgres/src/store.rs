//! SQLx-backed [`MemoryStoreProvider`].
//!
//! Runtime queries (no compile-time macro) so builds never require a
//! live database. Optimistic concurrency: updates carry the caller's
//! expected `version`; a zero-rows update is a conflict, not silence.

use crate::mapping::{status_from_db, status_to_db, type_from_db, type_to_db};
use async_trait::async_trait;
use memory_domain::{
    MemoryContent, MemoryError, MemoryId, MemoryQuery, MemoryRecord, MemoryResult, MemoryScope,
    MemorySubject,
};
use memory_provider_api::MemoryStoreProvider;
use sqlx::{PgPool, Row};

/// Authoritative PostgreSQL store; one logical namespace per instance.
pub struct PostgresStore {
    pool: PgPool,
    namespace: String,
}

impl PostgresStore {
    /// Connects and applies embedded migrations idempotently.
    pub async fn connect(url: &str, namespace: impl Into<String>) -> MemoryResult<Self> {
        let pool = PgPool::connect(url)
            .await
            .map_err(|e| MemoryError::unavailable("postgres", e.to_string()))?;
        Self::with_pool(pool, namespace).await
    }

    /// Assembles from an existing pool (server mode reuse).
    pub async fn with_pool(pool: PgPool, namespace: impl Into<String>) -> MemoryResult<Self> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS _barq_migrations (
                name TEXT PRIMARY KEY,
                applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
             )",
        )
        .execute(&pool)
        .await
        .map_err(|e| MemoryError::storage("postgres", e.to_string()))?;

        for (name, sql) in crate::MIGRATIONS {
            let applied: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM _barq_migrations WHERE name = $1)",
            )
            .bind(name)
            .fetch_one(&pool)
            .await
            .map_err(|e| MemoryError::storage("postgres", e.to_string()))?;

            if !applied {
                for statement in split_statements(sql) {
                    if statement.trim().is_empty() {
                        continue;
                    }
                    sqlx::query(&statement).execute(&pool).await.map_err(|e| {
                        MemoryError::storage("postgres", format!("migration {name}: {e}"))
                    })?;
                }
                sqlx::query("INSERT INTO _barq_migrations (name) VALUES ($1)")
                    .bind(name)
                    .execute(&pool)
                    .await
                    .map_err(|e| MemoryError::storage("postgres", e.to_string()))?;
            }
        }
        Ok(Self {
            pool,
            namespace: namespace.into(),
        })
    }

    /// Underlying pool for later phases (vector, episodic).
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Logical namespace served by this instance.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Append-only revision ledger entries for one record, newest first.
    ///
    /// Provider-native extension beyond the store contract; the engine
    /// uses the supersedes chain for logical history.
    pub async fn version_history(
        &self,
        id: MemoryId,
    ) -> MemoryResult<Vec<(i64, String, chrono::DateTime<chrono::Utc>)>> {
        let rows = sqlx::query(
            "SELECT version, status, recorded_at FROM memory_versions
             WHERE memory_id = $1 ORDER BY version DESC",
        )
        .bind(id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(pg_error)?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.get::<i64, _>("version"),
                    r.get::<String, _>("status"),
                    r.get("recorded_at"),
                )
            })
            .collect())
    }
}

fn pg_error(e: sqlx::Error) -> MemoryError {
    match &e {
        sqlx::Error::Database(db) if db.code().as_deref() == Some("40001") => {
            MemoryError::VersionConflict {
                memory_id: MemoryId::generate(), // serialization retry marker
                expected: 0,
                actual: 0,
            }
        }
        sqlx::Error::RowNotFound => MemoryError::NotFound {
            memory_id: MemoryId::generate(),
        },
        _ => MemoryError::storage("postgres", e.to_string()),
    }
}

/// Splits migration SQL on semicolon-terminated statements.
fn split_statements(sql: &str) -> Vec<String> {
    sql.split(';')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[async_trait]
impl MemoryStoreProvider for PostgresStore {
    fn name(&self) -> &str {
        "postgres"
    }

    async fn put(&self, memory: &MemoryRecord) -> MemoryResult<MemoryRecord> {
        let mut txn = self.pool.begin().await.map_err(pg_error)?;
        insert_memory(&mut txn, &self.namespace, memory).await?;
        sqlx::query(
            "INSERT INTO memory_versions (memory_id, version, status, content_text)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(memory.id.as_uuid())
        .bind(memory.version as i64)
        .bind(status_to_db(memory.status))
        .bind(&memory.content.text)
        .execute(&mut *txn)
        .await
        .map_err(pg_error)?;
        txn.commit().await.map_err(pg_error)?;
        Ok(memory.clone())
    }

    async fn get(&self, id: &MemoryId, scope: &MemoryScope) -> MemoryResult<Option<MemoryRecord>> {
        let row = sqlx::query("SELECT * FROM memories WHERE id = $1 AND namespace = $2")
            .bind(id.as_uuid())
            .bind(&self.namespace)
            .fetch_optional(&self.pool)
            .await
            .map_err(pg_error)?;

        match row {
            None => Ok(None),
            Some(row) => {
                let record = row_to_record(&row)?;
                if scope.contains(&record.scope) {
                    Ok(Some(record))
                } else {
                    Ok(None)
                }
            }
        }
    }

    async fn update(&self, memory: &MemoryRecord) -> MemoryResult<MemoryRecord> {
        let mut txn = self.pool.begin().await.map_err(pg_error)?;

        // Optimistic concurrency: only succeed from the caller's version.
        let result = sqlx::query(
            "UPDATE memories SET
                subtype = $3,
                tenant_id = $4, organization_id = $5, workspace_id = $6,
                user_id = $7, agent_id = $8, session_id = $9, task_id = $10,
                subject_type = $11, subject_id = $12, subject_display = $13,
                content_text = $14, content_structured = $15, content_tags = $16,
                confidence = $17, importance = $18,
                valid_from = $19, valid_to = $20,
                status = $21, version = $22,
                provenance = $23, retention = $24, updated_at = now()
             WHERE id = $1 AND namespace = $2 AND version = $25",
        )
        .bind(memory.id.as_uuid())
        .bind(&self.namespace)
        .bind(memory.subtype.as_deref())
        .bind(memory.scope.tenant_id.as_deref())
        .bind(memory.scope.organization_id.as_deref())
        .bind(memory.scope.workspace_id.as_deref())
        .bind(memory.scope.user_id.as_deref())
        .bind(memory.scope.agent_id.as_deref())
        .bind(memory.scope.session_id.as_deref())
        .bind(memory.scope.task_id.as_deref())
        .bind(memory.subject.as_ref().map(|s| s.entity_type.clone()))
        .bind(memory.subject.as_ref().map(|s| s.entity_id.clone()))
        .bind(memory.subject.as_ref().map(|s| s.display_name.clone()))
        .bind(&memory.content.text)
        .bind(memory.content.structured.clone())
        .bind(&memory.content.tags)
        .bind(memory.confidence)
        .bind(memory.importance)
        .bind(memory.valid_from)
        .bind(memory.valid_to)
        .bind(status_to_db(memory.status))
        .bind(memory.version as i64 + 1) // $22: new version
        .bind(serde_json::to_value(&memory.provenance).map_err(serde_err)?)
        .bind(serde_json::to_value(memory.retention).map_err(serde_err)?)
        .bind(memory.version as i64) // $25: expected current version
        .execute(&mut *txn)
        .await
        .map_err(pg_error)?;

        if result.rows_affected() == 0 {
            return Err(match self.get(&memory.id, &MemoryScope::default()).await? {
                None => MemoryError::NotFound {
                    memory_id: memory.id,
                },
                Some(stored) => MemoryError::VersionConflict {
                    memory_id: memory.id,
                    expected: memory.version,
                    actual: stored.version,
                },
            });
        }

        sqlx::query(
            "INSERT INTO memory_versions (memory_id, version, status, content_text)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(memory.id.as_uuid())
        .bind(memory.version as i64 + 1)
        .bind(status_to_db(memory.status))
        .bind(&memory.content.text)
        .execute(&mut *txn)
        .await
        .map_err(pg_error)?;

        txn.commit().await.map_err(pg_error)?;
        Ok(memory.clone())
    }

    async fn delete(&self, id: &MemoryId, scope: &MemoryScope) -> MemoryResult<()> {
        // Scope check happens in SQL so invisible rows are untouched.
        let result = sqlx::query(
            "DELETE FROM memories
             WHERE id = $1 AND namespace = $2
               AND (tenant_id IS NULL OR $3::text IS NULL OR tenant_id = $3)
               AND (workspace_id IS NULL OR $4::text IS NULL OR workspace_id = $4)
               AND (user_id IS NULL OR $5::text IS NULL OR user_id = $5)
               AND (agent_id IS NULL OR $6::text IS NULL OR agent_id = $6)
               AND (session_id IS NULL OR $7::text IS NULL OR session_id = $7)
               AND (task_id IS NULL OR $8::text IS NULL OR task_id = $8)",
        )
        .bind(id.as_uuid())
        .bind(&self.namespace)
        .bind(scope.tenant_id.as_deref())
        .bind(scope.workspace_id.as_deref())
        .bind(scope.user_id.as_deref())
        .bind(scope.agent_id.as_deref())
        .bind(scope.session_id.as_deref())
        .bind(scope.task_id.as_deref())
        .execute(&self.pool)
        .await
        .map_err(pg_error)?;
        let _ = result.rows_affected(); // absent already => still success
        Ok(())
    }

    async fn query(&self, query: &MemoryQuery) -> MemoryResult<Vec<MemoryRecord>> {
        let query = query.clone().validated()?;
        let mut sql = sqlx::QueryBuilder::new("SELECT * FROM memories WHERE namespace = ");
        sql.push_bind(&self.namespace);

        if !query.memory_types.is_empty() {
            let types: Vec<&str> = query.memory_types.iter().map(|t| type_to_db(*t)).collect();
            sql.push(" AND memory_type = ANY(")
                .push_bind(types)
                .push(")");
        }
        if !query.statuses.is_empty() {
            let statuses: Vec<&str> = query.statuses.iter().map(|s| status_to_db(*s)).collect();
            sql.push(" AND status = ANY(").push_bind(statuses).push(")");
        }
        if let Some(subject) = &query.subject {
            let key = subject.canonical_key();
            // canonical_key is "type:id" or "id"; split back apart.
            let (stype, sid) = match key.split_once(':') {
                Some((t, i)) => (Some(t.to_string()), i.to_string()),
                None => (None, key),
            };
            sql.push(" AND subject_id = ").push_bind(sid);
            if let Some(t) = stype {
                sql.push(" AND lower(subject_type) = ").push_bind(t);
            }
        }
        if let Some(text) = &query.text {
            // Word-level AND mirrors the embedded stores' semantics.
            for word in text.split_whitespace() {
                sql.push(" AND content_text ILIKE ")
                    .push_bind(format!("%{}%", word));
            }
        }

        let snapshot = query.effective_valid_at();
        sql.push(" AND (valid_from IS NULL OR valid_from <= ")
            .push_bind(snapshot)
            .push(") AND (valid_to IS NULL OR valid_to > ")
            .push_bind(snapshot)
            .push(")");

        for dim in [
            ("tenant_id", &query.scope.tenant_id),
            ("organization_id", &query.scope.organization_id),
            ("workspace_id", &query.scope.workspace_id),
            ("user_id", &query.scope.user_id),
            ("agent_id", &query.scope.agent_id),
            ("session_id", &query.scope.session_id),
            ("task_id", &query.scope.task_id),
        ] {
            if let Some(value) = dim.1 {
                sql.push(format_args!(" AND {} = ", dim.0)).push_bind(value);
            }
        }

        sql.push(" ORDER BY created_at DESC LIMIT ")
            .push_bind(query.limit as i64);

        let rows = sql.build().fetch_all(&self.pool).await.map_err(pg_error)?;
        rows.iter().map(row_to_record).collect()
    }
}

fn serde_err(e: serde_json::Error) -> MemoryError {
    MemoryError::storage("postgres", e.to_string())
}

async fn insert_memory(
    txn: &mut sqlx::PgConnection,
    namespace: &str,
    m: &MemoryRecord,
) -> MemoryResult<()> {
    sqlx::query(
        "INSERT INTO memories (
            id, namespace, memory_type, subtype,
            tenant_id, organization_id, workspace_id, user_id, agent_id, session_id, task_id,
            subject_type, subject_id, subject_display,
            content_text, content_structured, content_tags,
            confidence, importance, valid_from, valid_to,
            status, version, supersedes, provenance, retention,
            created_at, updated_at
         ) VALUES (
            $1, $2, $3, $4,
            $5, $6, $7, $8, $9, $10, $11,
            $12, $13, $14,
            $15, $16, $17,
            $18, $19, $20, $21,
            $22, $23, $24, $25, $26,
            $27, $28
         )",
    )
    .bind(m.id.as_uuid())
    .bind(namespace)
    .bind(type_to_db(m.memory_type))
    .bind(m.subtype.as_deref())
    .bind(m.scope.tenant_id.as_deref())
    .bind(m.scope.organization_id.as_deref())
    .bind(m.scope.workspace_id.as_deref())
    .bind(m.scope.user_id.as_deref())
    .bind(m.scope.agent_id.as_deref())
    .bind(m.scope.session_id.as_deref())
    .bind(m.scope.task_id.as_deref())
    .bind(m.subject.as_ref().map(|s| s.entity_type.clone()))
    .bind(m.subject.as_ref().map(|s| s.entity_id.clone()))
    .bind(m.subject.as_ref().map(|s| s.display_name.clone()))
    .bind(&m.content.text)
    .bind(m.content.structured.clone())
    .bind(&m.content.tags)
    .bind(m.confidence)
    .bind(m.importance)
    .bind(m.valid_from)
    .bind(m.valid_to)
    .bind(status_to_db(m.status))
    .bind(m.version as i64)
    .bind(m.supersedes.map(|id| id.as_uuid()))
    .bind(serde_json::to_value(&m.provenance).map_err(serde_err)?)
    .bind(serde_json::to_value(m.retention).map_err(serde_err)?)
    .bind(m.created_at)
    .bind(m.updated_at)
    .execute(&mut *txn)
    .await
    .map_err(pg_error)?;
    Ok(())
}

fn row_to_record(row: &sqlx::postgres::PgRow) -> MemoryResult<MemoryRecord> {
    let memory_type: String = row.try_get("memory_type").map_err(pg_error)?;
    let status_raw: String = row.try_get("status").map_err(pg_error)?;

    let subject = if let Some(entity_id) = row
        .try_get::<Option<String>, _>("subject_id")
        .map_err(pg_error)?
    {
        Some(MemorySubject {
            entity_type: row.try_get("subject_type").map_err(pg_error)?,
            display_name: row.try_get("subject_display").map_err(pg_error)?,
            entity_id,
        })
    } else {
        None
    };

    let structured = row
        .try_get::<Option<serde_json::Value>, _>("content_structured")
        .map_err(pg_error)?
        .filter(|v| !v.is_null());

    Ok(MemoryRecord {
        id: MemoryId::from_uuid(row.try_get::<uuid::Uuid, _>("id").map_err(pg_error)?),
        memory_type: type_from_db(&memory_type)?,
        subtype: row.try_get("subtype").map_err(pg_error)?,
        scope: MemoryScope {
            tenant_id: row.try_get("tenant_id").map_err(pg_error)?,
            organization_id: row.try_get("organization_id").map_err(pg_error)?,
            workspace_id: row.try_get("workspace_id").map_err(pg_error)?,
            user_id: row.try_get("user_id").map_err(pg_error)?,
            agent_id: row.try_get("agent_id").map_err(pg_error)?,
            session_id: row.try_get("session_id").map_err(pg_error)?,
            task_id: row.try_get("task_id").map_err(pg_error)?,
        },
        subject,
        content: MemoryContent {
            text: row.try_get("content_text").map_err(pg_error)?,
            structured,
            tags: row.try_get("content_tags").map_err(pg_error)?,
        },
        confidence: row.try_get("confidence").map_err(pg_error)?,
        importance: row.try_get("importance").map_err(pg_error)?,
        valid_from: row.try_get("valid_from").map_err(pg_error)?,
        valid_to: row.try_get("valid_to").map_err(pg_error)?,
        status: status_from_db(&status_raw)?,
        version: row.try_get::<i64, _>("version").map_err(pg_error)? as u64,
        supersedes: row
            .try_get::<Option<uuid::Uuid>, _>("supersedes")
            .map_err(pg_error)?
            .map(MemoryId::from_uuid),
        provenance: serde_json::from_value(row.try_get("provenance").map_err(pg_error)?)
            .map_err(serde_err)?,
        retention: serde_json::from_value(row.try_get("retention").map_err(pg_error)?)
            .map_err(serde_err)?,
        created_at: row.try_get("created_at").map_err(pg_error)?,
        updated_at: row.try_get("updated_at").map_err(pg_error)?,
    })
}
