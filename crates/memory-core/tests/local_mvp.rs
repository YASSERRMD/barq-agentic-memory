//! End-to-end proof of the Phase 1 exit criteria: a Rust application
//! uses the engine locally and memories persist across restarts.

use memory_core::{MemoryEngine, RememberRequest, UpdateRequest};
use memory_domain::{
    config::StoreConfig, EngineConfig, MemoryQuery, MemoryScope, MemoryScopeBuilder, MemoryType,
};
use std::path::PathBuf;

fn temp_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "barq-e2e-{tag}-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ))
}

#[tokio::test]
async fn memories_survive_engine_restart() {
    let dir = temp_dir("restart");
    let path = dir.join("memories.redb");

    // First run: remember a durable fact.
    let id = {
        let config = EngineConfig {
            namespace: "demo".into(),
            store: StoreConfig::Local { path: path.clone() },
            ..EngineConfig::default()
        };
        let engine = MemoryEngine::from_config(config).expect("engine");

        let saved = engine
            .remember(
                RememberRequest::new(MemoryType::Semantic, "Project Atlas runs on PostgreSQL")
                    .with_scope(MemoryScopeBuilder::new().tenant("acme").user("u-1").build()),
            )
            .await
            .expect("remember");
        saved.id
    };
    // Engine dropped: process "exits".

    // Second run: the fact is still there.
    {
        let config = EngineConfig {
            namespace: "demo".into(),
            store: StoreConfig::Local { path },
            ..EngineConfig::default()
        };
        let engine = MemoryEngine::from_config(config).expect("engine restart");

        let hits = engine
            .search(MemoryQuery::default().with_text("postgresql"))
            .await
            .expect("search after restart");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, id);
        assert_eq!(hits[0].content.text, "Project Atlas runs on PostgreSQL");
    }

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn namespaces_partition_one_file_between_tenants() {
    let dir = temp_dir("tenants");

    fn config_for(ns: &str, path: PathBuf) -> EngineConfig {
        EngineConfig {
            namespace: ns.into(),
            store: StoreConfig::Local { path },
            ..EngineConfig::default()
        }
    }

    let path_a = dir.join("shared.redb");
    let tenant_a = MemoryEngine::from_config(config_for("tenant-a", path_a.clone()))
        .expect("engine a");
    let tenant_b = MemoryEngine::from_config(config_for("tenant-b", path_a))
        .expect("engine b (same file)");

    tenant_a
        .remember(RememberRequest::new(MemoryType::Semantic, "a-only fact"))
        .await
        .expect("remember a");

    assert!(
        tenant_b
            .search(MemoryQuery::default().with_text("a-only"))
            .await
            .expect("search b")
            .is_empty(),
        "tenant-b must not see tenant-a memories"
    );
    assert_eq!(
        tenant_a
            .search(MemoryQuery::default().with_text("a-only"))
            .await
            .expect("search a")
            .len(),
        1
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn full_lifecycle_flow_over_local_store() {
    let dir = temp_dir("lifecycle");
    let config = EngineConfig {
        store: StoreConfig::Local {
            path: dir.join("lifecycle.redb"),
        },
        ..EngineConfig::default()
    };
    let engine = MemoryEngine::from_config(config).expect("engine");

    let v1 = engine
        .remember(RememberRequest::new(MemoryType::Semantic, "Atlas uses MySQL"))
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

    let chain = engine.history(v2.id, &MemoryScope::default()).await.expect("history");
    assert_eq!(chain.len(), 2);

    assert!(engine.forget(v2.id, &MemoryScope::default()).await.expect("forget"));
    assert!(
        engine
            .search(MemoryQuery::default())
            .await
            .expect("search")
            .is_empty()
    );

    std::fs::remove_dir_all(&dir).ok();
}
