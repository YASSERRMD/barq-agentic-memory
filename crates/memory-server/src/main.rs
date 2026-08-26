//! Server binary: runs the memory engine over REST.
//!
//! BARQ_STORE_PATH=/tmp/mem.redb BARQ_ADDR=127.0.0.1:8080 cargo run -p memory-server

use memory_core::MemoryEngine;
use memory_domain::config::{EmbeddingConfig, EngineConfig, StoreConfig, VectorStoreConfig};
use memory_server::serve;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr: std::net::SocketAddr = std::env::var("BARQ_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8080".into())
        .parse()?;

    let mut config = EngineConfig {
        // Server mode still gets semantic recall by default: hashing
        // embedder, no model download; production swaps pgvector in
        // via config without code changes.
        vector: Some(VectorStoreConfig::InMemory),
        embedding: Some(EmbeddingConfig::Hashing { dimensions: 256 }),
        ..EngineConfig::default()
    };
    if let Ok(path) = std::env::var("BARQ_STORE_PATH") {
        config.store = StoreConfig::Local { path: path.into() };
    }

    let engine = MemoryEngine::from_config(config).await?;
    println!("barq memory server listening on http://{addr}");
    serve(engine, addr).await?;
    Ok(())
}
