//! Live PostgreSQL integration tests.
//!
//! Opt-in so the shared gate stays hermetic: set `BARQ_TEST_PG_URL`
//! (e.g. postgres://barq@localhost:5433/barq_memories) and run with
//! `cargo test -p provider-postgres --test pg_live -- --ignored`.

use memory_domain::{
    MemoryContent, MemoryId, MemoryQuery, MemoryRecord, MemoryScope, MemoryScopeBuilder,
    MemoryType, RetentionPolicy,
};
use memory_provider_api::MemoryStoreProvider;
use provider_postgres::PostgresStore;

fn test_url() -> Option<String> {
    std::env::var("BARQ_TEST_PG_URL").ok()
}

#[tokio::test]
#[ignore = "requires BARQ_TEST_PG_URL"]
async fn migrations_are_idempotent_and_schema_is_complete() {
    let Some(url) = test_url() else {
        eprintln!("BARQ_TEST_PG_URL not set; skipping");
        return;
    };
    let store = PostgresStore::connect(&url, "migrate-test")
        .await
        .expect("connect");
    // Second connect exercises the already-applied path.
    let _ = PostgresStore::connect(&url, "migrate-test")
        .await
        .expect("reconnect");

    let indexes: i64 = sqlx_index_count(&store).await;
    assert!(indexes >= 10, "expected blueprint indexes, found {indexes}");
}

async fn sqlx_index_count(store: &PostgresStore) -> i64 {
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_indexes WHERE tablename = 'memories'")
        .fetch_one(store.pool())
        .await
        .expect("index count");
    n
}

#[tokio::test]
#[ignore = "requires BARQ_TEST_PG_URL"]
async fn crud_with_scope_isolation_and_temporal_validity() {
    let Some(url) = test_url() else {
        eprintln!("BARQ_TEST_PG_URL not set; skipping");
        return;
    };
    let store = PostgresStore::connect(&url, "crud-test")
        .await
        .expect("connect");

    // Unique tenant per run keeps repeat invocations independent.
    let acme = MemoryScopeBuilder::new()
        .tenant(format!("acme-{}", uuid::Uuid::now_v7().simple()))
        .user("u-1")
        .build();
    let mut record = MemoryRecord::new(
        MemoryType::Semantic,
        MemoryContent::from_text("Atlas uses PostgreSQL").with_structured(serde_json::json!({
            "project": "atlas",
            "database": "postgres"
        })),
    )
    .with_subject(memory_domain::MemorySubject::new("atlas").with_type("project"))
    .with_retention(RetentionPolicy::permanent());
    record.scope = acme.clone();

    let saved = store.put(&record).await.expect("put");
    assert_eq!(saved.id, record.id);

    // Scope isolation on read.
    let foreign = MemoryScopeBuilder::new().tenant("globex").build();
    assert!(store.get(&saved.id, &foreign).await.expect("get").is_none());
    assert!(store.get(&saved.id, &acme).await.expect("get").is_some());

    // Structured payload round-trips through JSONB.
    let got = store.get(&saved.id, &acme).await.expect("get").unwrap();
    assert_eq!(
        got.content.structured.as_ref().unwrap()["database"],
        "postgres"
    );

    // Query by subject + type + keyword within the run's tenant.
    let hits = store
        .query(
            &MemoryQuery::default()
                .of_type(MemoryType::Semantic)
                .with_text("atlas")
                .with_scope(acme.clone()),
        )
        .await
        .expect("query");
    assert_eq!(hits.len(), 1);

    // Temporal snapshot in the past hides a future-only fact.
    let future_only = MemoryRecord {
        valid_from: Some(chrono::Utc::now() + chrono::Duration::days(30)),
        ..MemoryRecord::new(
            MemoryType::Prospective,
            MemoryContent::from_text("future plan"),
        )
        .with_scope(acme.clone())
    };
    store.put(&future_only).await.expect("put future");
    let now_hits = store
        .query(&MemoryQuery::default().with_scope(acme.clone()))
        .await
        .expect("query now");
    assert!(now_hits.iter().all(|h| h.id != future_only.id));
    let future_q = MemoryQuery::default()
        .valid_at(chrono::Utc::now() + chrono::Duration::days(60))
        .with_text("future plan")
        .with_scope(acme.clone());
    assert_eq!(store.query(&future_q).await.expect("query later").len(), 1);

    // Provider delete is the physical path (engine-level forget()
    // tombstones first); absent-after-delete means idempotent success.
    store.delete(&saved.id, &acme).await.expect("delete");
    assert!(store.get(&saved.id, &acme).await.expect("get").is_none());
    store.delete(&saved.id, &acme).await.expect("delete again");
    let gone = !row_exists(&store, saved.id).await;
    assert!(gone, "physical deletion must remove the canonical row");
}

async fn row_exists(store: &PostgresStore, id: MemoryId) -> bool {
    sqlx::query("SELECT 1 AS x FROM memories WHERE id = $1")
        .bind(id.as_uuid())
        .fetch_optional(store.pool())
        .await
        .expect("exists query")
        .is_some()
}

#[tokio::test]
#[ignore = "requires BARQ_TEST_PG_URL"]
async fn optimistic_concurrency_detects_stale_writers() {
    let Some(url) = test_url() else {
        eprintln!("BARQ_TEST_PG_URL not set; skipping");
        return;
    };
    let store_a = PostgresStore::connect(&url, "concurrency")
        .await
        .expect("connect a");
    let store_b = PostgresStore::connect(&url, "concurrency")
        .await
        .expect("connect b");

    let base = MemoryRecord::new(MemoryType::Working, MemoryContent::from_text("v1"));
    store_a.put(&base).await.expect("put");

    // Two writers read version 1...
    let mut w1 = base.clone();
    let mut w2 = base.clone();
    w1.content = MemoryContent::from_text("writer one wins");
    w2.content = MemoryContent::from_text("writer two stale");

    store_b.update(&w1).await.expect("first writer succeeds");

    let err = store_b.update(&w2).await.unwrap_err();
    match err {
        memory_domain::MemoryError::VersionConflict {
            expected, actual, ..
        } => {
            assert_eq!((expected, actual), (1, 2));
        }
        other => panic!("expected VersionConflict, got {other}"),
    }

    // The ledger records both attempts' accepted versions only.
    let history = store_a.version_history(base.id).await.expect("history");
    assert_eq!(history.len(), 2, "initial + winning update");
}

#[tokio::test]
#[ignore = "requires BARQ_TEST_PG_URL"]
async fn namespaces_partition_the_same_database() {
    let Some(url) = test_url() else {
        eprintln!("BARQ_TEST_PG_URL not set; skipping");
        return;
    };
    let a = PostgresStore::connect(&url, "tenant-a").await.expect("a");
    let b = PostgresStore::connect(&url, "tenant-b").await.expect("b");

    let record = MemoryRecord::new(
        MemoryType::Semantic,
        MemoryContent::from_text("namespace secret"),
    );
    a.put(&record).await.expect("put");

    assert!(
        b.get(&record.id, &MemoryScope::default())
            .await
            .expect("get")
            .is_none()
    );
    assert!(
        a.get(&record.id, &MemoryScope::default())
            .await
            .expect("get")
            .is_some()
    );

    let b_hits = b
        .query(&MemoryQuery::default().with_text("namespace secret"))
        .await
        .expect("q");
    assert!(b_hits.is_empty(), "cross-namespace leakage is forbidden");
}
