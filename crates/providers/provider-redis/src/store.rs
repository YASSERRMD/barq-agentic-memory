//! [`WorkingMemoryProvider`] backed by Redis.
//!
//! Key layout: `{namespace}:session:{session_id}` → hash with `data`
//! (JSON payload) and `revision` (monotonic counter). Expiry is set on
//! every write, so Redis owns eviction; nothing here graduates to
//! long-term storage.

use async_trait::async_trait;
use chrono::Utc;
use memory_domain::{MemoryError, MemoryResult};
use memory_provider_api::WorkingMemoryProvider;
use redis::{AsyncCommands, Script, aio::ConnectionManager};
use serde_json::Value as Json;
use std::collections::HashMap;
use std::time::Duration;

/// Contract state type.
use memory_provider_api::WorkingMemoryState;

/// Atomic revision-guarded update.
///
/// Returns "0" on success, "-1" when the key is missing, "-2" on
/// revision mismatch — checked by callers into typed errors.
const CAS_SCRIPT: &str = r#"
local current = redis.call('HGET', KEYS[1], 'revision')
if not current then
    return -1
end
if tonumber(current) ~= tonumber(ARGV[1]) then
    return -2
end
redis.call('HSET', KEYS[1], 'data', ARGV[2], 'revision', ARGV[3])
redis.call('EXPIRE', KEYS[1], ARGV[4])
return 0
"#;

/// Create-once initialization; returns the existing state if present.
const INIT_SCRIPT: &str = r#"
if redis.call('EXISTS', KEYS[1]) == 1 then
    return 0
end
redis.call('HSET', KEYS[1], 'data', ARGV[1], 'revision', ARGV[2])
redis.call('EXPIRE', KEYS[1], ARGV[3])
return 1
"#;

/// Redis-backed session state store for embedded or server mode.
#[derive(Clone)]
pub struct RedisWorkingStore {
    connection: ConnectionManager,
    namespace: String,
}

impl RedisWorkingStore {
    /// Connects a self-healing connection manager.
    ///
    /// The manager reconnects automatically after Redis restarts, which
    /// is exactly what volatile session storage wants: sessions expire,
    /// the client does not.
    pub async fn connect(url: &str, namespace: impl Into<String>) -> MemoryResult<Self> {
        let client = redis::Client::open(url)
            .map_err(|e| MemoryError::unavailable("redis", e.to_string()))?;
        let connection = ConnectionManager::new(client)
            .await
            .map_err(|e| MemoryError::unavailable("redis", e.to_string()))?;
        Ok(Self {
            connection,
            namespace: namespace.into(),
        })
    }

    /// Logical namespace of this instance.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    fn key(&self, session_id: &str) -> String {
        format!("{}:session:{session_id}", self.namespace)
    }

    fn ttl_secs(ttl: Duration) -> i64 {
        ttl.as_secs().max(1) as i64
    }

    async fn fetch(&self, key: &str) -> MemoryResult<Option<(String, u64)>> {
        let mut conn = self.connection.clone();
        let fields: HashMap<String, String> = conn
            .hgetall(key)
            .await
            .map_err(|e| MemoryError::storage("redis", e.to_string()))?;
        if fields.is_empty() {
            return Ok(None);
        }
        let data = fields
            .get("data")
            .cloned()
            .ok_or_else(|| MemoryError::storage("redis", "session hash missing 'data' field"))?;
        let revision = fields
            .get("revision")
            .and_then(|r| r.parse::<u64>().ok())
            .ok_or_else(|| {
                MemoryError::storage("redis", "session hash missing valid 'revision'")
            })?;
        Ok(Some((data, revision)))
    }
}

#[async_trait]
impl WorkingMemoryProvider for RedisWorkingStore {
    fn name(&self) -> &str {
        "redis"
    }

    async fn set(&self, state: &WorkingMemoryState, ttl: Duration) -> MemoryResult<()> {
        let mut conn = self.connection.clone();
        let key = self.key(&state.session_id);
        let payload = serde_json::to_string(&state.data)
            .map_err(|e| MemoryError::storage("redis", e.to_string()))?;

        let revision_str = state.revision.to_string();
        let _: () = redis::pipe()
            .atomic()
            .hset_multiple(
                &key,
                &[
                    ("data", payload.as_str()),
                    ("revision", revision_str.as_str()),
                ],
            )
            .expire(&key, Self::ttl_secs(ttl))
            .query_async(&mut conn)
            .await
            .map_err(|e| MemoryError::storage("redis", e.to_string()))?;
        Ok(())
    }

    async fn get(&self, session_id: &str) -> MemoryResult<Option<WorkingMemoryState>> {
        match self.fetch(&self.key(session_id)).await? {
            None => Ok(None),
            Some((payload, revision)) => {
                let data: Json = serde_json::from_str(&payload)
                    .map_err(|e| MemoryError::storage("redis", e.to_string()))?;
                Ok(Some(WorkingMemoryState {
                    session_id: session_id.to_string(),
                    data,
                    revision,
                    updated_at: Utc::now(),
                }))
            }
        }
    }

    async fn delete(&self, session_id: &str) -> MemoryResult<()> {
        let mut conn = self.connection.clone();
        let deleted: i64 = conn
            .del(self.key(session_id))
            .await
            .map_err(|e| MemoryError::storage("redis", e.to_string()))?;
        let _ = deleted; // idempotent either way
        Ok(())
    }

    async fn compare_and_set(
        &self,
        session_id: &str,
        expected_revision: u64,
        data: Json,
        ttl: Duration,
    ) -> MemoryResult<WorkingMemoryState> {
        let mut conn = self.connection.clone();
        let key = self.key(session_id);
        let payload = serde_json::to_string(&data)
            .map_err(|e| MemoryError::storage("redis", e.to_string()))?;

        let script = Script::new(CAS_SCRIPT);
        let code: i32 = script
            .key(&key)
            .arg(expected_revision.to_string())
            .arg(payload.as_str())
            .arg((expected_revision + 1).to_string())
            .arg(Self::ttl_secs(ttl).to_string())
            .invoke_async(&mut conn)
            .await
            .map_err(|e| MemoryError::storage("redis", e.to_string()))?;

        match code {
            0 => Ok(WorkingMemoryState {
                session_id: session_id.to_string(),
                data,
                revision: expected_revision + 1,
                updated_at: Utc::now(),
            }),
            -1 => Err(MemoryError::SessionNotFound {
                session_id: session_id.to_string(),
            }),
            -2 => {
                // Re-read so the error carries the true stored revision.
                let actual = self.fetch(&key).await?.map(|(_, rev)| rev).unwrap_or(0);
                Err(MemoryError::SessionConflict {
                    session_id: session_id.to_string(),
                    expected: expected_revision,
                    actual,
                })
            }
            other => Err(MemoryError::storage(
                "redis",
                format!("CAS returned {other}"),
            )),
        }
    }

    async fn initialize(
        &self,
        session_id: &str,
        data: Json,
        ttl: Duration,
    ) -> MemoryResult<WorkingMemoryState> {
        // Initialize-once: create only when absent, else read what is
        // there. A concurrent creator wins; we adopt its state.
        let created = self
            .compare_and_set_missing(session_id, data.clone(), ttl)
            .await?;
        match created {
            Some(state) => Ok(state),
            None => self
                .get(session_id)
                .await?
                .ok_or(MemoryError::SessionNotFound {
                    session_id: session_id.to_string(),
                }),
        }
    }
}

impl RedisWorkingStore {
    /// CAS against "missing" sentinel: creates the entry only if absent.
    async fn compare_and_set_missing(
        &self,
        session_id: &str,
        data: Json,
        ttl: Duration,
    ) -> MemoryResult<Option<WorkingMemoryState>> {
        let mut conn = self.connection.clone();
        let key = self.key(session_id);
        let payload = serde_json::to_string(&data)
            .map_err(|e| MemoryError::storage("redis", e.to_string()))?;

        let script = Script::new(INIT_SCRIPT);
        let code: i32 = script
            .key(&key)
            .arg(payload.as_str())
            .arg("1") // first revision
            .arg(Self::ttl_secs(ttl).to_string())
            .invoke_async(&mut conn)
            .await
            .map_err(|e| MemoryError::storage("redis", e.to_string()))?;

        if code == 1 {
            Ok(Some(WorkingMemoryState {
                session_id: session_id.to_string(),
                data,
                revision: 1,
                updated_at: Utc::now(),
            }))
        } else {
            Ok(None)
        }
    }
}
