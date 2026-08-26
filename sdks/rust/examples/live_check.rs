//! Live SDK check against a running server (scripts/test_sdks.sh).
//! Usage: BARQ_BASE=http://127.0.0.1:8080 cargo run -p memory-client --example live_check

use memory_client::{MemoryClient, RememberOptions, ReqwestTransport};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base = std::env::var("BARQ_BASE").unwrap_or_else(|_| "http://127.0.0.1:8080".into());
    let client = MemoryClient::new(ReqwestTransport::new(base));

    let saved = client
        .remember(
            "Rust SDK smoke fact",
            RememberOptions {
                tenant_id: Some("acme".into()),
                ..Default::default()
            },
        )
        .await?;
    assert!(saved.id.len() > 10);

    let hits = client.recall("sdk smoke fact", Some("acme"), 5).await?;
    assert!(
        hits.iter().any(|h| h.memory.id == saved.id),
        "recall failed"
    );

    let successor = client.update(&saved.id, "Rust SDK smoke fact v2").await?;
    let chain = client.history(&successor.id).await?;
    assert_eq!(chain.len(), 2, "history must show both generations");

    client.forget(&successor.id, false).await?;
    println!("RUST SDK SMOKE TEST OK");
    Ok(())
}
