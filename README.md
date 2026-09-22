<div align="center">

<img src="docs/assets/logo.png" alt="barq-agentic-memory" width="200" />

# barq-agentic-memory

**A portable memory engine for AI agents — written in Rust.**

Six concepts. Five memory types. Any backend. Zero LLM required.

[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-orange?logo=rust)](Cargo.toml)
[![Tests](https://img.shields.io/badge/tests-297%20passing-brightgreen)](docs/phase-log.md)
[![Blueprint](https://img.shields.io/badge/blueprint-24%2F24%20phases%20complete-gold)](docs/phase-log.md)
[![Built with opencode](https://img.shields.io/badge/built%20with-opencode%20%C2%B7%20ox%20alpha-8b949e)](https://opencode.ai)

[Architecture](docs/architecture.md) · [Changelog](CHANGELOG.md) · [Contributing](CONTRIBUTING.md) · [Phase Ledger](docs/phase-log.md)

</div>

---

## Why barq

Vector stores recall similarity. Databases recall rows. Neither handles
**truth changing over time**, two writers disagreeing, or a fact that must
be forgotten everywhere at once. barq is a typed, provider-independent
memory engine that treats those problems as first-class:

| | |
|---|---|
| **Temporal truth** | Facts carry validity windows — corrections supersede, history is retired, never silently destroyed |
| **Conflict handling** | Contradictions detected by rules; authority and confidence decide, ambiguity quarantines for review |
| **Hybrid retrieval** | Exact → keyword → semantic → rerank in one call, always scope-isolated |
| **Coordinated forgetting** | One `forget()` tombstones the canonical row, vector index, and graph edges together |
| **Governance inside** | Denied reads look like absence; every attempt lands on an audit trail; AES-256-GCM at-rest encryption |
| **Zero-LLM by default** | Classification, extraction, and embeddings run on deterministic rules — real models plug in behind the same traits |

## Quick start

**Rust** — embedded, zero infrastructure:

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

**Python**:

```python
from agent_memory import Memory

memory = Memory("./data")                # persists across restarts
memory.remember("Customer prefers email.", user_id="123")
memory.recall("How should I contact this customer?", user_id="123")
```

**REST** — same engine, server mode:

```bash
BARQ_STORE_PATH=/var/lib/barq/mem.redb BARQ_ADDR=0.0.0.0:8080 \
cargo run -p memory-server --release

curl -s localhost:8080/v1/recall -H 'content-type: application/json' \
     -d '{"query":"which database does atlas use","limit":5}'
```

Client SDKs with identical concepts ship for [Rust](sdks/rust),
[Python](sdks/python), [TypeScript](sdks/typescript), and [.NET](sdks/dotnet).

## The API

Six operations, identical across every language and transport:

| Operation | Meaning |
|---|---|
| `remember` | Store a typed memory (auto-classification optional) |
| `recall` | Hybrid retrieval — plan, fan out, rank |
| `search` | Keyword/filtered lookup |
| `update` | Supersede — new revision, history preserved |
| `forget` | Tombstone everywhere; `hard` for erasure |
| `history` | The full supersession chain, oldest first |

Five memory types behind them:

| Type | Holds | Example |
|---|---|---|
| **Semantic** | Durable facts, preferences, entities | "Customer prefers email" |
| **Episodic** | Actions, outcomes, trajectories with evidence links | "Migration failed; rolled back" |
| **Procedural** | Governed runbooks (DRAFT → REVIEW → APPROVED → ACTIVE) | "Staging deploy checklist" |
| **Prospective** | Goals, deadlines, dependencies | "Renew TLS before Friday" |
| **Working** | Live session state (TTL, never auto-promoted) | Current task scratchpad |

## Architecture

<p align="center">
  <img src="docs/assets/architecture.png" alt="barq-agentic-memory architecture" width="840" />
</p>

Every write flows through **classify → deduplicate → resolve conflicts →
route & rank**; every read hydrates through the canonical store, so scope
isolation and temporal validity are authoritative — never the index.
Embedded and server modes run the exact same core engine. Full details in
[`docs/architecture.md`](docs/architecture.md).

## Backends

Swap storage without touching application code — providers implement
traits, the engine API never changes:

| Capability | Embedded default | Production backend | Cargo feature |
|---|---|---|---|
| Canonical store | In-process · redb single file | PostgreSQL — optimistic concurrency, version ledger | `postgres` |
| Working memory | In-process TTL map | Redis — atomic Lua CAS | `redis` |
| Semantic index | In-memory cosine | pgvector — HNSW, model-stamp guards | `pgvector` |
| Entity graph | In-memory adjacency | `GraphProvider` trait | — |
| Episodes | In-memory | `EpisodeStore` trait | — |

## Performance

Release builds, Apple Silicon, in-memory engine ([`scripts/bench.sh`](scripts/bench.sh)):

| Operation | Latency |
|---|---:|
| Exact read | **~312 ns** |
| Engine startup | ~885 ns |
| Write — store + embed + index | ~4.7 µs |
| Semantic recall — 50 docs | ~25 µs |
| Hybrid recall — plan → fan-out → rerank | ~43 µs |

## Production readiness

- **Graceful degradation** — vector index down ⇒ exact retrieval keeps serving; per-backend health reporting
- **Resilience** — domain-aware retries, circuit breakers with half-open probes, idempotent index self-repair
- **Scale-out** — background workers own indexing and sweeps off the synchronous write path
- **Observability** — tracing spans on every engine operation, structured HTTP logs (`RUST_LOG`-driven, OTLP-ready layer swap)
- **Tested** — 297 hermetic tests, a mock-driven [testkit](crates/memory-testkit) with failure injection, plus live suites for PostgreSQL, Redis, pgvector, both bindings, and all four SDKs

## Verification

Every claim above is enforced by tests:

```bash
./scripts/gate.sh                # fmt + clippy -D warnings + full test suite
./scripts/test_sdks.sh           # boots server; exercises all four SDK clients
./scripts/test_python_binding.sh # builds wheel via maturin, runs e2e smoke
./scripts/test_node_binding.sh   # builds .node addon, runs e2e smoke
./scripts/bench.sh               # Criterion benchmarks (release only)
```

## Project layout

```text
crates/
├── memory-domain          # canonical model — no I/O dependencies
├── memory-provider-api    # provider traits (store · vector · working · embedder)
├── memory-core            # engine facade: six concepts + governance + repair
├── memory-retrieval       # planner, hybrid executor, scoring
├── memory-testkit         # mocks with recording + failure injection
├── memory-classifier · memory-dedup · memory-conflict
├── memory-episodic · memory-graph · memory-procedural · memory-prospective
├── memory-lifecycle · memory-policy · memory-reliability · memory-workers
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

CI runs the gate plus live PostgreSQL/Redis suites and all binding/SDK
smoke tests on every push ([workflow](.github/workflows/gate.yml));
releases publish to crates.io in dependency order via
[`scripts/publish.sh`](scripts/publish.sh). See
[CONTRIBUTING.md](CONTRIBUTING.md) for the workflow, design invariants,
and the add-a-provider guide.

## Built with

This engine was developed end-to-end with **[opencode](https://opencode.ai)**
driven by the **ox alpha model** — all 24 blueprint phases, from engine
contract to SDKs, were planned, implemented, tested, and shipped through
that agent workflow. The full ledger lives in
[`docs/phase-log.md`](docs/phase-log.md).

## License

[Apache-2.0](LICENSE)
