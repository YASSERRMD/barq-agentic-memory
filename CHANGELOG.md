# Changelog

All notable changes to barq-agentic-memory. Dates use DD Month YYYY.
The full phase-by-phase build ledger lives in
[docs/phase-log.md](docs/phase-log.md).

## v0.9.0 — 20 September 2026

First release covering the complete 24-phase blueprint.

### Engine
- **Canonical model** — five memory types (working / episodic / semantic /
  procedural / prospective), scopes with wildcard isolation, validity
  windows, supersession history, provenance, retention classes.
- **Six-concept API** — remember, recall, search, update, forget, history;
  updates never rewrite records, they supersede.
- **Hybrid retrieval** — rule-based planner (exact → keyword → semantic
  fan-out), composite scoring (similarity, recency, importance,
  confidence, authority), deterministic reranking.
- **Intelligence** — rule-based classifier/extractor, six-signal dedup
  cascade (never embedding similarity alone), contradiction resolution
  with window-closing supersession.
- **Governance** — authorization filtering inside the engine (denials read
  as absence), AES-256-GCM content encryption, audit events, sensitivity
  tiers.
- **Lifecycle** — retention sweeps with coordinated deletion across
  canonical store, vector index, and graph; archival grace periods.

### Providers
- Embedded: in-memory, redb single-file persistence, in-memory vector
  index, in-memory graph, in-memory episodes.
- Server-grade: PostgreSQL (optimistic concurrency, append-only version
  ledger), Redis working memory (atomic Lua CAS), pgvector (HNSW,
  model-stamp guards).

### Interfaces
- Python binding (`agent_memory`, PyO3/Maturin), Node.js binding
  (napi-rs), Axum REST server (full blueprint endpoint set), client
  SDKs for Rust, Python, TypeScript, and .NET.

### Operations
- Reliability: domain-aware retries, circuit breakers, health with
  graceful degradation, self-repairing vector index.
- Scale-out: worker/ticker separation keeping indexing off the
  synchronous write path.
- Observability: tracing spans on engine operations and structured HTTP
  request logs (RUST_LOG-driven, OTLP-ready layer swap).

### Verification
- 266 hermetic tests across 20 workspace crates; live integration
  suites for PostgreSQL, Redis, pgvector, all four SDKs, and both
  bindings; Criterion benchmark baseline recorded in the phase ledger.

## v0.1.0 — 25 August 2026

Initial public commit: engine contract blueprint and repository scaffold.
