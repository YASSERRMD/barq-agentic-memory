//! Single-file persistent canonical store backed by redb.
//!
//! Chosen over RocksDB for a dependency-light build and over SQLite to
//! avoid FFI: one pure-Rust file that survives restarts. The on-disk
//! format is opaque to callers; compaction and migration belong to
//! lifecycle phases.

use crate::RECORD_TABLE;
use crate::filter::matches_query;
use async_trait::async_trait;
use memory_domain::{MemoryError, MemoryId, MemoryQuery, MemoryRecord, MemoryResult, MemoryScope};
use memory_provider_api::MemoryStoreProvider;
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const TABLE_DEF: TableDefinition<'static, &'static str, &'static [u8]> =
    TableDefinition::new(RECORD_TABLE);

/// Process-wide cache of open databases keyed by canonical path.
///
/// redb takes an exclusive file lock per [`Database`]; without this
/// cache, two engines sharing one file (different namespaces) would
/// deadlock on open. Weak handles let closed stores release the file.
static OPEN_DATABASES: std::sync::LazyLock<
    std::sync::Mutex<HashMap<PathBuf, std::sync::Weak<Database>>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

fn acquire_database(path: &Path) -> MemoryResult<Arc<Database>> {
    // Canonicalize via the parent so lookups match whether or not the
    // file exists yet (macOS resolves /var -> /private/var only for
    // existing paths).
    let canonical = match path.parent() {
        Some(parent) => parent
            .canonicalize()
            .unwrap_or_else(|_| parent.to_path_buf())
            .join(path.file_name().unwrap_or_default()),
        None => path.to_path_buf(),
    };
    let mut guard = OPEN_DATABASES.lock().expect("poisoned");
    if let Some(weak) = guard.get(&canonical) {
        if let Some(db) = weak.upgrade() {
            return Ok(db);
        }
    }
    let database = Database::create(path)
        .map_err(|e| MemoryError::storage("local", format!("open {}: {e}", path.display())))?;
    let shared = Arc::new(database);
    guard.insert(canonical, Arc::downgrade(&shared));
    Ok(shared)
}

/// Durable embedded store; one instance per logical namespace.
pub struct LocalStore {
    namespace: String,
    database: Arc<Database>,
}

impl LocalStore {
    /// Opens (creating if needed) the database file for `namespace`.
    pub fn open(path: impl AsRef<Path>, namespace: impl Into<String>) -> MemoryResult<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                MemoryError::storage("local", format!("mkdir {}: {e}", parent.display()))
            })?;
        }
        let database = acquire_database(path)?;

        // Materialize the table eagerly so read paths never race creation.
        let txn = database
            .begin_write()
            .map_err(|e| MemoryError::storage("local", e.to_string()))?;
        let _ = txn
            .open_table(TABLE_DEF)
            .map_err(|e| MemoryError::storage("local", e.to_string()))?;
        txn.commit()
            .map_err(|e| MemoryError::storage("local", e.to_string()))?;

        Ok(Self {
            namespace: namespace.into(),
            database,
        })
    }

    /// File backing this store.
    pub fn path(&self) -> MemoryResult<PathBuf> {
        Err(MemoryError::Unsupported(
            "path introspection not part of provider contract".into(),
        ))
    }

    fn key(id: &MemoryId) -> String {
        id.hyphenated()
    }

    fn namespaced(&self) -> String {
        format!("{}:", self.namespace)
    }

    fn encode(record: &MemoryRecord) -> MemoryResult<Vec<u8>> {
        serde_json::to_vec(record).map_err(|e| MemoryError::storage("local", e.to_string()))
    }

    fn decode(bytes: &[u8]) -> MemoryResult<MemoryRecord> {
        serde_json::from_slice(bytes).map_err(|e| MemoryError::storage("local", e.to_string()))
    }
}

#[async_trait]
impl MemoryStoreProvider for LocalStore {
    fn name(&self) -> &str {
        "local"
    }

    async fn put(&self, memory: &MemoryRecord) -> MemoryResult<MemoryRecord> {
        let key = format!("{}{}", self.namespaced(), Self::key(&memory.id));
        let value = Self::encode(memory)?;
        let db = self.database.clone();
        tokio::task::spawn_blocking(move || -> MemoryResult<()> {
            let txn = db
                .begin_write()
                .map_err(|e| MemoryError::storage("local", e.to_string()))?;
            {
                let mut table = txn
                    .open_table(TABLE_DEF)
                    .map_err(|e| MemoryError::storage("local", e.to_string()))?;
                table
                    .insert(key.as_str(), value.as_slice())
                    .map_err(|e| MemoryError::storage("local", e.to_string()))?;
            }
            txn.commit()
                .map_err(|e| MemoryError::storage("local", e.to_string()))
        })
        .await
        .map_err(|e| MemoryError::storage("local", e.to_string()))??;
        Ok(memory.clone())
    }

    async fn get(&self, id: &MemoryId, scope: &MemoryScope) -> MemoryResult<Option<MemoryRecord>> {
        let key = format!("{}{}", self.namespaced(), Self::key(id));
        let db = self.database.clone();
        let raw = tokio::task::spawn_blocking(move || -> MemoryResult<Option<Vec<u8>>> {
            let txn = db
                .begin_read()
                .map_err(|e| MemoryError::storage("local", e.to_string()))?;
            let table = txn
                .open_table(TABLE_DEF)
                .map_err(|e| MemoryError::storage("local", e.to_string()))?;
            match table
                .get(key.as_str())
                .map_err(|e| MemoryError::storage("local", e.to_string()))?
            {
                Some(cell) => Ok(Some(cell.value().to_vec())),
                None => Ok(None),
            }
        })
        .await
        .map_err(|e| MemoryError::storage("local", e.to_string()))??;

        match raw {
            None => Ok(None),
            Some(bytes) => {
                let record = Self::decode(&bytes)?;
                if scope.contains(&record.scope) {
                    Ok(Some(record))
                } else {
                    Ok(None)
                }
            }
        }
    }

    async fn update(&self, memory: &MemoryRecord) -> MemoryResult<MemoryRecord> {
        let key = format!("{}{}", self.namespaced(), Self::key(&memory.id));
        let exists = {
            let db = self.database.clone();
            let k = key.clone();
            tokio::task::spawn_blocking(move || -> MemoryResult<bool> {
                let txn = db
                    .begin_read()
                    .map_err(|e| MemoryError::storage("local", e.to_string()))?;
                let table = txn
                    .open_table(TABLE_DEF)
                    .map_err(|e| MemoryError::storage("local", e.to_string()))?;
                Ok(table
                    .get(k.as_str())
                    .map_err(|e| MemoryError::storage("local", e.to_string()))?
                    .is_some())
            })
            .await
            .map_err(|e| MemoryError::storage("local", e.to_string()))??
        };
        if !exists {
            return Err(MemoryError::NotFound {
                memory_id: memory.id,
            });
        }
        self.put(memory).await
    }

    async fn delete(&self, id: &MemoryId, scope: &MemoryScope) -> MemoryResult<()> {
        match self.get(id, scope).await? {
            None => return Ok(()), // invisible or absent => already gone
            Some(_) => {}
        }
        let key = format!("{}{}", self.namespaced(), Self::key(id));
        let db = self.database.clone();
        tokio::task::spawn_blocking(move || -> MemoryResult<()> {
            let txn = db
                .begin_write()
                .map_err(|e| MemoryError::storage("local", e.to_string()))?;
            {
                let mut table = txn
                    .open_table(TABLE_DEF)
                    .map_err(|e| MemoryError::storage("local", e.to_string()))?;
                table
                    .remove(key.as_str())
                    .map_err(|e| MemoryError::storage("local", e.to_string()))?;
            }
            txn.commit()
                .map_err(|e| MemoryError::storage("local", e.to_string()))
        })
        .await
        .map_err(|e| MemoryError::storage("local", e.to_string()))??;
        Ok(())
    }

    async fn query(&self, query: &MemoryQuery) -> MemoryResult<Vec<MemoryRecord>> {
        let query = query.clone().validated()?;
        let prefix = self.namespaced();
        let db = self.database.clone();
        let rows = tokio::task::spawn_blocking(move || -> MemoryResult<Vec<Vec<u8>>> {
            let txn = db
                .begin_read()
                .map_err(|e| MemoryError::storage("local", e.to_string()))?;
            let table = txn
                .open_table(TABLE_DEF)
                .map_err(|e| MemoryError::storage("local", e.to_string()))?;
            let mut out = Vec::new();
            for row in table
                .iter()
                .map_err(|e| MemoryError::storage("local", e.to_string()))?
            {
                let (key, cell) = row.map_err(|e| MemoryError::storage("local", e.to_string()))?;
                if !key.value().starts_with(&prefix) {
                    continue;
                }
                out.push(cell.value().to_vec());
            }
            Ok(out)
        })
        .await
        .map_err(|e| MemoryError::storage("local", e.to_string()))??;

        let mut hits: Vec<MemoryRecord> = Vec::with_capacity(rows.len());
        for bytes in &rows {
            hits.push(Self::decode(bytes)?);
        }
        hits.retain(|r| matches_query(r, &query));
        hits.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        hits.truncate(query.limit as usize);
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_domain::{MemoryContent, MemoryType};

    fn temp_path(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("barq-test-{tag}-{}", uuid::Uuid::now_v7().simple()));
        dir.join("memories.redb")
    }

    #[tokio::test]
    async fn persists_across_reopen() {
        let path = temp_path("persist");
        let id = {
            let store = LocalStore::open(&path, "acme").expect("open");
            let r = MemoryRecord::new(
                MemoryType::Semantic,
                MemoryContent::from_text("durable fact"),
            );
            store.put(&r).await.expect("put");
            r.id
        }; // store dropped here

        let reopened = LocalStore::open(&path, "acme").expect("reopen");
        let got = reopened
            .get(&id, &MemoryScope::default())
            .await
            .expect("get")
            .expect("record survives restart");
        assert_eq!(got.content.text, "durable fact");

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[tokio::test]
    async fn namespaces_isolate_within_one_file() {
        let path = temp_path("ns");
        let shared = LocalStore::open(&path, "tenant-a").expect("open");
        let r = MemoryRecord::new(MemoryType::Semantic, MemoryContent::from_text("only for a"));
        shared.put(&r).await.expect("put");

        let other = LocalStore::open(&path, "tenant-b").expect("open same file");
        let invisible = other
            .get(&r.id, &MemoryScope::default())
            .await
            .expect("get");
        assert!(invisible.is_none(), "namespace must isolate records");

        let visible = shared
            .get(&r.id, &MemoryScope::default())
            .await
            .expect("get");
        assert!(visible.is_some());

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[tokio::test]
    async fn update_requires_existing_record() {
        let path = temp_path("update");
        let store = LocalStore::open(&path, "ns").expect("open");
        let r = MemoryRecord::new(MemoryType::Working, MemoryContent::from_text("v1"));
        let err = store.update(&r).await.unwrap_err();
        assert!(matches!(err, MemoryError::NotFound { .. }));

        store.put(&r).await.expect("put");
        let mut r2 = r.clone();
        r2.version += 1;
        store.update(&r2).await.expect("update");

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[tokio::test]
    async fn query_applies_filters_and_limit() {
        let path = temp_path("query");
        let store = LocalStore::open(&path, "q").expect("open");
        for i in 0..4 {
            store
                .put(&MemoryRecord::new(
                    MemoryType::Episodic,
                    MemoryContent::from_text(format!("episode {i}")),
                ))
                .await
                .expect("put");
        }
        store
            .put(&MemoryRecord::new(
                MemoryType::Semantic,
                MemoryContent::from_text("episode-like fact"),
            ))
            .await
            .expect("put");

        let q = MemoryQuery::default()
            .of_type(MemoryType::Episodic)
            .with_text("episode")
            .with_limit(3);
        let hits = store.query(&q).await.expect("query");
        assert_eq!(hits.len(), 3);
        assert!(hits.iter().all(|h| h.memory_type == MemoryType::Episodic));

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
