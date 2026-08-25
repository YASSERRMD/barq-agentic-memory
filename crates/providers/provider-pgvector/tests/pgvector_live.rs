//! Live pgvector integration tests.
//!
//! Opt-in so the shared gate stays hermetic: set `BARQ_TEST_PGVECTOR_URL`
//! (e.g. postgres://barq:barq@localhost:5434/barq_memories) and run with
//! `cargo test -p provider-pgvector --test pgvector_live -- --ignored`.

use memory_domain::MemoryId;
use memory_provider_api::{
    EmbeddingProvider, HashingEmbedder, MetadataFilter, VectorProvider, VectorQuery, VectorRecord,
};
use provider_pgvector::PgVectorStore;
use std::collections::HashMap;

fn test_url() -> Option<String> {
    std::env::var("BARQ_TEST_PGVECTOR_URL").ok()
}

async fn embed(texts: &[&str]) -> Vec<Vec<f32>> {
    let owned: Vec<String> = texts.iter().map(|t| t.to_string()).collect();
    HashingEmbedder::new(384)
        .embed(&owned)
        .await
        .expect("embed")
}

fn record(id: MemoryId, embedding: Vec<f32>, tenant: Option<&str>) -> VectorRecord {
    let mut r = VectorRecord::new(id, embedding, "barq-hashing", "1");
    if let Some(t) = tenant {
        r = r.with_metadata("tenant_id", t);
    }
    r
}

#[tokio::test]
#[ignore = "requires BARQ_TEST_PGVECTOR_URL"]
async fn upsert_search_delete_roundtrip_with_ranking() {
    let Some(url) = test_url() else {
        eprintln!("BARQ_TEST_PGVECTOR_URL not set; skipping");
        return;
    };
    // Unique namespace per run keeps repeat invocations independent.
    let namespace = format!("vec-{}", uuid::Uuid::now_v7().simple());
    let index = PgVectorStore::connect(&url, &namespace)
        .await
        .expect("connect");

    let atlas = MemoryId::generate();
    let kitchen = MemoryId::generate();

    let embeddings = embed(&[
        "project atlas uses postgresql database",
        "office kitchen needs restocking",
    ])
    .await;
    index
        .upsert(&record(atlas, embeddings[0].clone(), None))
        .await
        .expect("upsert atlas");
    index
        .upsert(&record(kitchen, embeddings[1].clone(), None))
        .await
        .expect("upsert kitchen");

    // The atlas-related query must rank the atlas vector first.
    let query_embedding = embed(&["postgres database atlas"]).await.remove(0);
    let hits = index
        .search(&VectorQuery {
            embedding: query_embedding,
            top_k: 2,
            ..Default::default()
        })
        .await
        .expect("search");
    assert_eq!(hits.len(), 2, "both vectors share dimensions 384");
    assert_eq!(
        hits[0].memory_id, atlas,
        "atlas text must outrank the kitchen text"
    );
    assert!(hits[0].score > hits[1].score);
    // Normalized scoring: shared-vocabulary pair sits in (0.5, 1].
    assert!(hits[0].score > 0.5 && hits[0].score <= 1.0);
    assert!(
        (hits[1].score - 0.5).abs() < 1e-3,
        "orthogonal texts score ~0.5"
    );

    // Delete is idempotent and removes recallability.
    index.delete(&atlas).await.expect("delete");
    index.delete(&atlas).await.expect("delete again");
    let after = index
        .search(&VectorQuery {
            embedding: embed(&["postgres database"]).await.remove(0),
            top_k: 5,
            ..Default::default()
        })
        .await
        .expect("search after delete");
    assert!(after.iter().all(|h| h.memory_id != atlas));
}

#[tokio::test]
#[ignore = "requires BARQ_TEST_PGVECTOR_URL"]
async fn metadata_filters_narrow_search() {
    let Some(url) = test_url() else {
        eprintln!("BARQ_TEST_PGVECTOR_URL not set; skipping");
        return;
    };
    // Unique namespace per run keeps repeat invocations independent.
    let namespace = format!("vec-f-{}", uuid::Uuid::now_v7().simple());
    let index = PgVectorStore::connect(&url, &namespace)
        .await
        .expect("connect");

    let mine = MemoryId::generate();
    let theirs = MemoryId::generate();
    let shared = embed(&["identical payload text for both rows"])
        .await
        .remove(0);

    index
        .upsert(&record(mine, shared.clone(), Some("acme")))
        .await
        .expect("upsert mine");
    index
        .upsert(&record(theirs, shared, Some("globex")))
        .await
        .expect("upsert theirs");

    let mut filter = MetadataFilter::default();
    filter.equals.insert("tenant_id".into(), "acme".into());

    let hits = index
        .search(&VectorQuery {
            embedding: embed(&["identical payload text for both rows"])
                .await
                .remove(0),
            top_k: 10,
            filter,
            ..Default::default()
        })
        .await
        .expect("filtered search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].memory_id, mine);
}

#[tokio::test]
#[ignore = "requires BARQ_TEST_PGVECTOR_URL"]
async fn model_stamp_mismatch_is_rejected_on_overwrite() {
    let Some(url) = test_url() else {
        eprintln!("BARQ_TEST_PGVECTOR_URL not set; skipping");
        return;
    };
    let index = PgVectorStore::connect(&url, "vec-stamp")
        .await
        .expect("connect");

    let id = MemoryId::generate();
    let vec_a = embed(&["stable text"]).await.remove(0);
    index
        .upsert(&VectorRecord::new(id, vec_a.clone(), "barq-hashing", "1"))
        .await
        .expect("initial stamp");

    // Same stamp: overwrite allowed.
    index
        .upsert(&VectorRecord::new(id, vec_a.clone(), "barq-hashing", "1"))
        .await
        .expect("same-model overwrite ok");

    // Different model: refused.
    let err = index
        .upsert(&VectorRecord::new(id, vec_a, "text-embedding-3-small", "2"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, memory_domain::MemoryError::Validation { .. }),
        "mixed-generation overwrite must fail validation"
    );

    let _ = HashMap::<String, String>::new(); // keep import parity with filters
}

#[tokio::test]
#[ignore = "requires BARQ_TEST_PGVECTOR_URL"]
async fn namespaces_partition_vectors() {
    let Some(url) = test_url() else {
        eprintln!("BARQ_TEST_PGVECTOR_URL not set; skipping");
        return;
    };
    let a = PgVectorStore::connect(&url, "tenant-a").await.expect("a");
    let b = PgVectorStore::connect(&url, "tenant-b").await.expect("b");

    let id = MemoryId::generate();
    let v = embed(&["shared across namespaces"]).await.remove(0);
    a.upsert(&record(id, v.clone(), None))
        .await
        .expect("upsert a");

    let b_hits = b
        .search(&VectorQuery {
            embedding: v,
            top_k: 10,
            ..Default::default()
        })
        .await
        .expect("b search");
    assert!(
        b_hits.iter().all(|h| h.memory_id != id),
        "namespace isolation required"
    );

    let a_hits = a
        .search(&VectorQuery {
            embedding: embed(&["shared across namespaces"]).await.remove(0),
            top_k: 10,
            ..Default::default()
        })
        .await
        .expect("a search");
    assert!(a_hits.iter().any(|h| h.memory_id == id));
}
