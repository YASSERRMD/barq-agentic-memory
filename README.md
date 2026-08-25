# barq-agentic-memory

A standalone, high-performance, **framework-neutral memory engine** for AI
agents, written in Rust. Runs embedded in your process or as a server, and is
consumed by any agent or framework through native bindings, REST, or gRPC.

```text
remember() recall() search() update() forget() history()
```

Six public concepts. Under the hood: classification, routing, deduplication,
conflict resolution, indexing, hybrid retrieval, temporal validity,
provenance, retention, scoping.

- Five memory types: **working**, **episodic**, **semantic**, **procedural**,
  **prospective**.
- Provider-based storage: local embedded file → PostgreSQL → Redis → vector
  and graph stores. Swap backends without changing the API.
- Bitemporal facts with supersession history — older truths are retired,
  never silently destroyed.
- Works with zero LLM dependency when callers supply structured memories.

## Status

Under active development, phase by phase (see
[`docs/phase-log.md`](docs/phase-log.md) and
[`docs/architecture.md`](docs/architecture.md)).

| Release | Scope |
|---|---|
| v0.1 | domain + local embedded CRUD |
| v0.2 | PostgreSQL + Redis |
| v0.3 | pgvector semantic recall |
| ... | full plan in the phase log |

## Development

```bash
cargo build --workspace   # compile everything
cargo test --workspace    # run all tests
./scripts/gate.sh         # fmt + clippy -D warnings + tests
```

Licensed under Apache-2.0.
