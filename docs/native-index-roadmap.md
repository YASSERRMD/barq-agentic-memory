# Native Index Roadmap (Deferred by Design)

Status: **not built — on purpose.** The blueprint is explicit: optional
native indexes (HNSW, BM25, bitmap metadata, temporal, entity, WAL,
compaction, snapshots) come *only after profiling proves a real need*
and are excluded from the first production release.

This document records the profiling evidence we already have, the
trigger conditions that would justify each index, and the design
constraints any future implementation must respect.

## Current profiling evidence (phase 20 baseline, release build)

| Path | Latency | Corpus |
|---|---|---|
| Exact get (`recall_exact`) | ~312 ns | n/a |
| Keyword search (embedded scan) | ~40.7 µs | 100 docs |
| Semantic recall (flat cosine) | ~25.2 µs | 50 docs / 256-dim |
| Hybrid recall (plan+fan-out+rerank) | ~43.2 µs | 50 docs |
| Write with inline indexing | ~4.7 µs | per record |
| Engine assembly (startup) | ~885 ns | embedded |

## Trigger conditions

Each index is justified only when its trigger fires in production
telemetry, not in microbenchmarks:

| Index | Trigger | Why deferred |
|---|---|---|
| HNSW (in-store) | Semantic recall p99 > 50 ms at >100k vectors | pgvector already ships HNSW; embedded scale rarely justifies a second implementation |
| BM25 | Keyword recall quality complaints AND corpus > 1M docs | Word-AND substring filtering serves current corpora adequately |
| Bitmap metadata | Metadata-filtered queries > 10 ms at >5M rows | Postgres partial indexes cover server mode today |
| Temporal index | Validity-window queries dominating planner time in flamegraphs | B-tree on (valid_from, valid_to) suffices at current write rates |
| Entity index | Entity lookups > 5 ms p99 | Canonical subject indexes (PG) + graph store cover this |
| WAL / compaction / snapshots | redb file growth or recovery-time SLOs breached | redb's own WAL is correct; measured restarts are instant at MVP scale |

## Design constraints for any future implementation

1. **Behind provider traits only.** A native index must implement the
   existing `MemoryStoreProvider` / `VectorProvider` contracts — the
   engine API never changes for an index.
2. **Embedded optional.** Native indexes ship behind cargo features,
   off by default; the default build stays dependency-light.
3. **Benchmarks precede merges.** A PR adding a native index must
   include before/after Criterion runs at production-representative
   corpus sizes.
4. **Graceful degradation.** An index failure must degrade to exact
   retrieval (phase 21 contract), never fail the read path.
5. **No custom database.** The phase-1 rule survives: we adopt storage
   engines, we do not write them, unless a trigger above fires with
   flamegraph evidence in hand.

## Review cadence

Revisit this document when production telemetry shows any trigger
condition, or at the v1.1 planning checkpoint, whichever comes first.
