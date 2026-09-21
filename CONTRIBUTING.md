# Contributing to barq-agentic-memory

Thanks for helping build a portable memory engine for AI agents.

## Development environment

**Required**
- Rust 1.85+ (`rustup`)
- That's it — the default build is fully embedded (in-memory backends)

**Optional, per surface**

| Surface | Requirement | Verification |
|---|---|---|
| PostgreSQL provider | any Postgres with the `vector` extension (e.g. `pgvector/pgvector:pg16` docker image) | `scripts` below |
| Redis provider | Redis 7 | `scripts` below |
| Node binding | Node 20+, npm | `scripts/test_node_binding.sh` |
| Python binding | Python 3.8+, `pip install maturin` | `scripts/test_python_binding.sh` |
| All four SDKs | Node, Python, .NET 10 | `scripts/test_sdks.sh` |

Live integration tests are **opt-in** via environment variables so the
default gate needs no infrastructure:

```bash
export BARQ_TEST_PG_URL="postgres://barq:barq@localhost:5432/barq_memories"
export BARQ_TEST_PGVECTOR_URL="$BARQ_TEST_PG_URL"
export BARQ_TEST_REDIS_URL="redis://localhost:6379"

cargo test -p provider-postgres  --test pg_live      -- --ignored
cargo test -p provider-pgvector  --test pgvector_live -- --ignored
cargo test -p provider-redis     --test redis_live   -- --ignored
```

## The gate

Every change passes the gate before merge:

```bash
./scripts/gate.sh
```

It runs `cargo fmt --check`, `cargo clippy --workspace --all-targets
-- -D warnings`, the full test suite, and compile checks for the
`postgres` + `redis` + `pgvector` feature combinations. CI runs the
same gate plus live suites; per repo policy **merges never wait on CI**.

## Workflow

1. Branch from `main` (`feat/...`, `fix/...`, `docs/...`, `phase/...`).
2. Work in atomic commits — one logical change per commit, every commit
   builds and passes existing tests.
3. Conventional Commits, scope = crate or package:
   `feat(memory-core): ...`, `fix(provider-postgres): ...`,
   `docs(readme): ...`. Commit bodies explain **why**, not what.
4. Open a PR; merge with a merge commit (never squash — per-phase
   history is part of the ledger).
5. Tag releases `vX.Y.Z`; see `scripts/publish.sh` for the crates.io
   dependency order.

## Design rules (the invariants)

Changes that break these will not merge:

1. **History is never destroyed.** Updates supersede; deletes tombstone
   until coordinated sweeps run.
2. **Scope isolation is enforced at every read.**
3. **The engine works with zero LLM dependency.** Intelligence plugs in
   behind traits (`MemoryClassifier`, `ExtractionProvider`,
   `EmbeddingProvider`), never into the core.
4. **Providers are replaceable.** The engine API never leaks backend
   types.
5. **Denied reads look like absence** (governance), and `similarity
   alone never merges` (dedup).
6. **Benchmarks precede native indexes.** See
   `docs/native-index-roadmap.md` before proposing index work.

## Adding a provider

Implement the relevant trait from `memory-provider-api`
(`MemoryStoreProvider`, `VectorProvider`, `WorkingMemoryProvider`),
place the crate under `crates/providers/`, wire it into
`MemoryEngine::from_config` behind a cargo feature, add hermetic tests
plus an opt-in live suite, and register the feature combo in
`scripts/gate.sh` if needed.

## License

Apache-2.0. By contributing you agree your contributions are licensed
under the same terms.
