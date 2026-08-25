//! Runnable proof of the Phase 1 exit criteria:
//! `cargo run -p memory-core --example quickstart`

use memory_core::{MemoryEngine, RememberRequest, UpdateRequest};
use memory_domain::{EngineConfig, MemoryQuery, MemoryType};

#[tokio::main]
async fn main() {
    let data_dir = std::env::temp_dir().join("barq-quickstart");
    let config = EngineConfig {
        namespace: "quickstart".into(),
        store: memory_domain::config::StoreConfig::Local {
            path: data_dir.join("memory.redb"),
        },
        ..EngineConfig::default()
    };

    let engine = MemoryEngine::from_config(config).expect("engine");

    // remember()
    let fact = engine
        .remember(
            RememberRequest::new(MemoryType::Semantic, "Customer prefers email contact")
                .with_subtype("preference"),
        )
        .await
        .expect("remember");
    println!("remembered: {}", fact.id);

    // search()
    let hits = engine
        .search(MemoryQuery::default().with_text("email"))
        .await
        .expect("search");
    println!("search('email') -> {} hit(s)", hits.len());

    // update() — creates a successor, history preserved
    let updated = engine
        .update(UpdateRequest::content(
            fact.id,
            Default::default(),
            "Customer prefers SMS contact",
        ))
        .await
        .expect("update");
    println!("updated -> {}", updated.id);

    // history()
    let chain = engine
        .history(updated.id, &Default::default())
        .await
        .unwrap();
    println!("history chain: {} generation(s)", chain.len());

    // forget() + working state
    engine.forget(fact.id, &Default::default()).await.unwrap();
    engine
        .set_working_state(
            "demo-session",
            serde_json::json!({ "goal": "finish phase 1" }),
        )
        .await
        .unwrap();
    let working = engine.working_state("demo-session").await.unwrap();
    println!("working state alive: {}", working.is_some());

    println!("quickstart OK — rerun to see persistence across restarts");
}
