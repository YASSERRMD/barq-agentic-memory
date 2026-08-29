<div align="center">

<img src="docs/assets/logo.png" alt="barq-agentic-memory" width="200" />

# barq-agentic-memory

**A portable memory engine for AI agents — written in Rust.**

Six concepts. Five memory types. Any backend. Zero LLM required.

[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-orange?logo=rust)](Cargo.toml)
[![Tests](https://img.shields.io/badge/tests-265%20passing-brightgreen)](docs/phase-log.md)
[![Blueprint](https://img.shields.io/badge/blueprint-24%2F24%20phases%20complete-gold)](docs/phase-log.md)
[![Built with opencode](https://img.shields.io/badge/built%20with-opencode%20%C2%B7%20ox%20alpha-8b949e?logo=data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciLz4=)](https://opencode.ai)

[Architecture](docs/architecture.md) · [Phase Ledger](docs/phase-log.md) · [API Reference](#the-api) · [Benchmarks](#performance)

</div>

---

## Why

Agents keep forgetting. Vector stores only recall similarity; databases
only recall rows. Neither model handles *truth changing over time*, two
writers disagreeing, or a fact that must be forgotten everywhere at once.

**barq** is a typed, provider-independent memory engine that treats those
problems as first-class:

| | |
|---|---|
| **Temporal truth** | Facts carry validity windows. Corrections supersede — history is retired, never silently destroyed |
| **Conflict handling** | Contradictions detected by rules; authority and confidence decide, ambiguity quarantines for review |
| **Hybrid retrieval** | Exact → keyword → semantic → rerank in one call; every recall runs through scope isolation |
| **Coordinated forgetting** | One `forget()` tombstones canonical row, vector index, and graph edges together |
| **Governance built in** | Authorization filters *inside* the engine — denied memories look like absence, and every attempt is audited |
| **Zero-LLM operation** | Classification, extraction, and embeddings work out of the box with deterministic rules — attach real models behind the same traits when you want them |

## The API

Six operations across every language and transport:

```text
remember   recall      search
update     forget      history
```

Five memory types behind them:

| Type | Holds | Example |
|---|---|---|
| **Semantic** | Durable facts, preferences, entities | "Customer prefers email" |
| **Episodic** | Actions, outcomes, trajectories | "Migration failed; rolled back" |
| **Procedural** | Governed runbooks (DRAFT → ACTIVE → REVOKED) | "Staging deploy checklist" |
| **Prospective** | Goals, deadlines, dependencies | "Renew TLS before Friday" |
| **Working** | Live session state (TTL, never auto-promoted) | Current task scratchpad |

## Quick start

### Rust — embedded, zero infrastructure

```rust
use memory_core::{MemoryEngine, RememberRequest};
use memory_domain::{
    config::{EmbeddingConfig, StoreConfig, VectorStoreConfig},
    EngineConfig, MemoryType,
};
use memory_retrieval::RecallRequest;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = MemoryEngine::from_config(EngineConfig {
        store: StoreConfig::Local { path: "./memory.redb".into() },
        // Semantic recall with no model download:
        vector: Some(VectorStoreConfig::InMemory),
        embedding: Some(EmbeddingConfig::Hashing { dimensions: 256 }),
        ..Default::default()
    }).await?;

    engine.remember(RememberRequest::new(
        MemoryType::Semantic, "Project Atlas uses PostgreSQL",
    )).await?;

    let hits = engine
        .recall(&RecallRequest::new("which database does atlas use?"))
        .await?;

    println!("{}", hits[0].record.content.text); // Project Atlas uses PostgreSQL
    Ok(())
}
```

### Python — the two-line experience

```python
from agent_memory import Memory

memory = Memory("./data")                      # persists across restarts
memory.remember("Customer prefers email.", user_id="123")
memory.recall("How should I contact this customer?", user_id="123")
```

### Server mode

```bash
BARQ_STORE_PATH=/var/lib/barq/mem.redb \
BARQ_ADDR=0.0.0.0:8080 \
cargo run -p memory-server --release
```

```bash
curl -s localhost:8080/v1/recall -H 'content-type: application/json' \
     -d '{"query":"which database does atlas use","limit":5}'
```

## Architecture

<p align="center">
  <img src="docs/assets/architecture.png" alt="barq-agentic-memory architecture" width="840" />
</p>

Every write flows through **classify → deduplicate → resolve conflicts →
route & rank**; every read hydrates through the canonical store so scope
isolation and temporal validity are authoritative — never the index.
Details in [`docs/architecture.md`](docs/architecture.md).

## Backends

Swap storage without touching application code — providers implement
traits, the engine API never changes:

| Capability | Embedded default | Production backend | Feature flag |
|---|---|---|---|
| Canonical store | In-process / redb single file | PostgreSQL (optimistic concurrency, version ledger) | `postgres` |
| Working memory | In-process TTL map | Redis (atomic Lua CAS) | `redis` |
| Semantic index | In-memory cosine | pgvector (HNSW, model-stamp guards) | `pgvector` |
| Entity graph | In-memory adjacency | Neo4j-compatible trait | — |
| Episodes | In-memory | Trait-backed | — |

## Performance

Release builds, Apple Silicon, in-memory engine ([`scripts/bench.sh`](scripts/bench.sh)):

| Operation | Latency |
|---|---:|
| Exact read | **~312 ns** |
| Engine startup | ~885 ns |
| Write (store + embed + index) | ~4.7 µs |
| Semantic recall, 50 docs | ~25 µs |
| Hybrid recall (plan → fan-out → rerank) | ~43 µs |

## Reliability

- **Graceful degradation** — vector index down ⇒ exact retrieval keeps serving; health reports per-backend status
- **Circuit breakers + retries** — domain-aware retry classification, trip-after-N failure fast
- **Self-repair** — `repair_vector_index()` reconciles ghosts and missing embeddings idempotently
- **Scale-out ready** — background workers own indexing/sweeps off the synchronous write path

## Verification

Every claim above is enforced by tests — 265 hermetic tests run on every
change, plus live integration suites:

```bash
./scripts/gate.sh                # fmt + clippy -D warnings + full test suite
./scripts/test_sdks.sh           # boots server; exercises Rust/Python/TS/.NET clients
./scripts/test_python_binding.sh # builds wheel via maturin, runs e2e smoke
./scripts/test_node_binding.sh   # builds .node addon, runs e2e smoke
./scripts/bench.sh               # Criterion benchmarks (release only)
```

## Repository

```text
crates/
├── memory-domain          # canonical model — no I/O dependencies
├── memory-provider-api    # provider traits (store · vector · working · embedder)
├── memory-core            # the engine facade: six concepts + governance + repair
├── memory-retrieval       # planner, hybrid executor, scoring
├── memory-classifier / memory-dedup / memory-conflict
├── memory-episodic / memory-graph / memory-procedural / memory-prospective
├── memory-lifecycle / memory-policy / memory-reliability / memory-workers
├── memory-server          # Axum REST mode (same core engine)
├── providers/             # local · postgres · redis · pgvector
└── bindings/              # python (PyO3) · node (napi-rs)
sdks/                      # rust · python · typescript · dotnet
```

## Development

```bash
cargo build --workspace
cargo test --workspace
./scripts/gate.sh
```

The project was built phase-by-phase against a fixed blueprint; the full
ledger — objectives, gate output, and honest deviations for all 24 phases —
lives in [`docs/phase-log.md`](docs/phase-log.md).

## Built with

This engine was developed end-to-end with **[opencode](https://opencode.ai)**
driven by the **ox alpha model** — all 24 blueprint phases, from engine
contract to SDKs, were planned, implemented, tested, and shipped through
that agent workflow.

## License

[Apache-2.0](LICENSE)
