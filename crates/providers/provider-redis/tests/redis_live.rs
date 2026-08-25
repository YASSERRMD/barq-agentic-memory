//! Live Redis integration tests.
//!
//! Opt-in so the shared gate stays hermetic: set `BARQ_TEST_REDIS_URL`
//! (e.g. redis://localhost:6399) and run with
//! `cargo test -p provider-redis --test redis_live -- --ignored`.

use memory_domain::MemoryError;
use memory_provider_api::{SessionSnapshot, WorkingMemoryProvider};
use provider_redis::RedisWorkingStore;
use serde_json::json;
use std::time::Duration;

fn test_url() -> Option<String> {
    std::env::var("BARQ_TEST_REDIS_URL").ok()
}

fn unique_session(tag: &str) -> String {
    format!("{tag}-{}", uuid::Uuid::now_v7().simple())
}

#[tokio::test]
#[ignore = "requires BARQ_TEST_REDIS_URL"]
async fn set_get_delete_roundtrip_with_ttl() {
    let Some(url) = test_url() else {
        eprintln!("BARQ_TEST_REDIS_URL not set; skipping");
        return;
    };
    let store = RedisWorkingStore::connect(&url, "wm-test")
        .await
        .expect("connect");
    let session = unique_session("s");

    let state = memory_provider_api::WorkingMemoryState::initial(
        session.clone(),
        json!({"goal": "book flight", "observations": ["price $400"]}),
    );
    store
        .set(&state, Duration::from_secs(60))
        .await
        .expect("set");

    let got = store.get(&session).await.expect("get").expect("live");
    assert_eq!(got.data["goal"], "book flight");
    assert_eq!(got.revision, 1);

    store.delete(&session).await.expect("delete");
    assert!(store.get(&session).await.expect("get").is_none());
}

#[tokio::test]
#[ignore = "requires BARQ_TEST_REDIS_URL"]
async fn ttl_expiry_is_enforced_by_the_backend() {
    let url = test_url().expect("BARQ_TEST_REDIS_URL");
    let store = RedisWorkingStore::connect(&url, "wm-ttl")
        .await
        .expect("connect");
    let session = unique_session("ttl");

    // Provider floors TTLs to whole seconds (EXPIRE semantics).
    let state = memory_provider_api::WorkingMemoryState::initial(session.clone(), json!({}));
    store
        .set(&state, Duration::from_secs(1))
        .await
        .expect("set");
    assert!(store.get(&session).await.expect("get").is_some());

    tokio::time::sleep(Duration::from_millis(1300)).await;
    assert!(
        store.get(&session).await.expect("get").is_none(),
        "Redis must evict expired sessions"
    );
}

#[tokio::test]
#[ignore = "requires BARQ_TEST_REDIS_URL"]
async fn atomic_cas_survives_concurrent_writers() {
    let url = test_url().expect("BARQ_TEST_REDIS_URL");
    let store_a = RedisWorkingStore::connect(&url, "wm-cas").await.expect("a");
    let store_b = RedisWorkingStore::connect(&url, "wm-cas").await.expect("b");
    let session = unique_session("cas");

    // Initialize-once from two "processes".
    let first = store_a
        .initialize(&session, json!({"tool": null}), Duration::from_secs(60))
        .await
        .expect("init a");
    let second = store_b
        .initialize(&session, json!({"tool": null}), Duration::from_secs(60))
        .await
        .expect("init b");
    assert_eq!(first.revision, second.revision);

    // Two writers start from revision 1; only one CAS may win...
    let winner = store_a
        .compare_and_set(
            &session,
            1,
            json!({"tool": "writer-a"}),
            Duration::from_secs(60),
        )
        .await
        .expect("writer a wins");
    assert_eq!(winner.revision, 2);

    match store_b
        .compare_and_set(
            &session,
            1,
            json!({"tool": "stale-b"}),
            Duration::from_secs(60),
        )
        .await
    {
        Err(MemoryError::SessionConflict {
            expected, actual, ..
        }) => {
            assert_eq!(
                (expected, actual),
                (1, 2),
                "error must report true stored revision"
            )
        }
        other => panic!("expected SessionConflict, got {other:?}"),
    }

    // Winner's data intact — no lost update in either direction.
    let final_state = store_a.get(&session).await.expect("get").unwrap();
    assert_eq!(final_state.data["tool"], "writer-a");
}

#[tokio::test]
#[ignore = "requires BARQ_TEST_REDIS_URL"]
async fn namespaces_partition_sessions() {
    let url = test_url().expect("BARQ_TEST_REDIS_URL");
    let agent_a = RedisWorkingStore::connect(&url, "agent-a")
        .await
        .expect("a");
    let agent_b = RedisWorkingStore::connect(&url, "agent-b")
        .await
        .expect("b");
    let session = unique_session("shared");

    let state = memory_provider_api::WorkingMemoryState::initial(session.clone(), json!("a"));
    agent_a
        .set(&state, Duration::from_secs(60))
        .await
        .expect("set");

    assert!(agent_b.get(&session).await.expect("get").is_none());
    assert!(agent_a.get(&session).await.expect("get").is_some());
}

#[tokio::test]
#[ignore = "requires BARQ_TEST_REDIS_URL"]
async fn snapshot_view_roundtrips_through_engine_shaped_data() {
    let url = test_url().expect("BARQ_TEST_REDIS_URL");
    let store = RedisWorkingStore::connect(&url, "wm-snap")
        .await
        .expect("connect");
    let session = unique_session("snap");

    let mut snap = SessionSnapshot::default();
    snap.push_goal("deploy phase 3");
    snap.push_observation("tests green");
    snap.push_tool_result("cargo test: ok");
    snap.add_checkpoint_ref("01ABCHECKPOINT000000000000000");

    let mut data = json!({"custom": true});
    snap.apply_to(&mut data);

    let state = memory_provider_api::WorkingMemoryState::initial(session.clone(), data);
    store
        .set(&state, Duration::from_secs(60))
        .await
        .expect("set");

    let loaded = store.get(&session).await.expect("get").unwrap();
    let parsed = SessionSnapshot::from_state_data(&loaded.data);
    assert_eq!(parsed.active_goals, vec!["deploy phase 3".to_string()]);
    assert_eq!(parsed.checkpoint_refs.len(), 1);
}
