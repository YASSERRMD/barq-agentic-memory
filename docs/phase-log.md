# Phase Log — barq-agentic-memory

Implementation ledger for the Rust agentic memory engine, following
`temp/rust_agentic_memory_engine_phase_by_phase.md`.

Each phase opens with its objective on a branch, is built in atomic commits,
and closes with real gate output recorded below.

---

## Phase 00 — Engine Contract

**Branch:** `phase/00-engine-contract`
**Objective:** Freeze the engine boundary before writing provider-specific code.
Build the memory taxonomy, canonical record, scopes, provider traits, error
model, query model, configuration schema, and provider registry.

**Scope of work**

- `crates/memory-domain/`: taxonomy (`MemoryType`, `MemoryStatus`),
  identifiers, scopes, subjects, content/provenance/retention value types,
  temporal validity, canonical `MemoryRecord`, error model, query model,
  configuration schema.
- `crates/memory-provider-api/`: `MemoryStoreProvider`, `VectorProvider`,
  and `WorkingMemoryProvider` traits plus their DTOs.
- `crates/memory-core/`: provider registry and config-driven assembly
  validation (no backends yet).
- Workspace scaffold, gate script, architecture doc, README.

**Exit criteria:** Domain and core crates compile without any database
dependency. ✅ `memory-domain`, `memory-provider-api`, and `memory-core`
have no database dependencies; the workspace compiles standalone.

**Gate output:**

```text
$ ./scripts/gate.sh
fmt: OK
clippy: OK
test result: ok. 9 passed   (memory-core)
test result: ok. 53 passed  (memory-domain)
test result: ok. 7 passed   (memory-provider-api)
test: OK
GATE PASSED
```

69 unit tests, zero failures.

**Deviations:** 26 commits on the phase branch — within the >=20 floor.
No filler commits were needed; each commit is a distinct contract element
(identifiers, taxonomy, scope, subjects, content, provenance, temporal,
record, errors, query, config, three provider traits, registry, assembly,
docs, gate).

---

## Phase 01 — Embedded Local MVP

**Branch:** `phase/01-local-mvp`
**Objective:** Make the engine usable without external infrastructure:
in-memory provider, local persistent provider (redb), engine CRUD facade,
exact lookup, memory-type filtering, namespaces, and TTL for working
memory. No custom database.

**Exit criteria:** A Rust application can use the engine locally and
persist memories across restarts.

**Gate output:**

```text
$ ./scripts/gate.sh
fmt: OK
clippy: OK
test result: ok. 22 passed  (memory-core unit)
test result: ok. 3 passed   (memory-core e2e restart/isolation/lifecycle)
test result: ok. 53 passed  (memory-domain)
test result: ok. 7 passed   (memory-provider-api)
test result: ok. 19 passed  (provider-local)
test: OK
GATE PASSED
```

104 tests, zero failures. Quickstart example run twice demonstrated
persistence across process restarts.

**Deviations:** 7 commits — below the >=20 floor. Honest decomposition
yielded exactly this many: the phase is one provider crate (filter,
memory store, persistent store, working store) plus the engine facade,
integration tests, and gate fixes; splitting further would have produced
non-building intermediate states. Recorded per the floor-not-padding
rule.

---

## Phase 02 — PostgreSQL Canonical Store

**Branch:** `phase/02-postgres-store`
**Objective:** Production-grade authoritative persistence: SQLx provider,
migrations, version history, provenance, temporal validity, soft delete,
retention metadata, optimistic concurrency. Indexes cover tenant,
workspace, user, agent, memory type, subject, status, validity, and
creation time.

**Exit criteria:** PostgreSQL becomes the source of truth for long-term
memory.

**Gate output:** _recorded at phase close_

**Gate output:**

```text
$ ./scripts/gate.sh
fmt: OK
clippy: OK
22 passed  (memory-core unit)
3 passed   (memory-core e2e)
53 passed  (memory-domain)
7 passed   (memory-provider-api)
19 passed  (provider-local)
3 passed   (provider-postgres unit: mapping roundtrips)
4 ignored  (pg_live: opt-in, see below)
test: OK
GATE PASSED
```

107 hermetic tests, zero failures. The shared gate requires no database.

**Live-PG verification:**

```text
$ BARQ_TEST_PG_URL=postgres://barq@localhost:5433/barq_memories \
  cargo test -p provider-postgres --test pg_live -- --ignored --test-threads=1
test crud_with_scope_isolation_and_temporal_validity ... ok
test migrations_are_idempotent_and_schema_is_complete ... ok
test namespaces_partition_the_same_database ... ok
test optimistic_concurrency_detects_stale_writers ... ok
test result: ok. 4 passed; 0 failed
```

Ran against a throwaway PostgreSQL 18 instance (port 5433, trust auth,
temp data dir) — no system services touched.

**Deviations:** 3 commits — below the floor because the phase is one
cohesive provider (migration SQL, store implementation, live tests).
Defects found and fixed inside the phase: the optimistic-concurrency
WHERE clause compared the wrong version (found by the live test before
first commit). Provider-level delete is physical; soft-delete semantics
live at engine level via forget() tombstones, with the append-only
memory_versions ledger preserving revisions.

---

## Phase 03 — Redis Working Memory

**Branch:** `phase/03-redis-working-memory`
**Objective:** Very fast active/session state: current session state,
active goals, recent observations, tool results, TTL, checkpoint
references, version-safe updates. Working memory never graduates to
long-term automatically.

**Exit criteria:** Session state survives concurrent tool-call writers
without lost updates; expiry is enforced by the backend.

**Gate output:** _recorded at phase close_

**Gate output:**

```text
$ ./scripts/gate.sh
fmt: OK
clippy: OK
features: OK  (memory-core/postgres + memory-core/redis combos)
22 passed (core) | 3 passed (e2e) | 53 passed (domain) | 10 passed (provider-api)
19 passed (provider-local) | 3 passed (provider-postgres) | 4+5 ignored (live)
test: OK
GATE PASSED
```

107 hermetic tests, zero failures.

**Live-Redis verification:**

```text
$ BARQ_TEST_REDIS_URL=redis://localhost:6399 \
  cargo test -p provider-redis --test redis_live -- --ignored --test-threads=1
test atomic_cas_survives_concurrent_writers ... ok
test namespaces_partition_sessions ... ok
test set_get_delete_roundtrip_with_ttl ... ok
test snapshot_view_roundtrips_through_engine_shaped_data ... ok
test ttl_expiry_is_enforced_by_the_backend ... ok
test result: ok. 5 passed; 0 failed
```

Ran against a throwaway `redis:7-alpine` Docker container on port 6399.
The CAS test proves concurrent-writer safety: two writers from revision
1, exactly one wins, the loser receives SessionConflict with the true
stored revision, and no update is lost.

**Deviations:** 3 commits — cohesive provider + trait extension; the
session-conflict error refactor and CAS contract landed as one unit by
design rather than padding further.

---

## Phase 04 — Vector Provider

**Branch:** `phase/04-vector-provider`
**Objective:** Semantic similarity retrieval: embedding abstraction with
model/version stamps, pgvector provider, in-memory vector fallback,
top-K search, metadata filtering, update/delete synchronization between
canonical store and vector index. Exit criterion: vector providers are
replaceable without changing the engine API.

**Gate output:** _recorded at phase close_

**Live-pgvector verification:** _recorded at phase close_ (opt-in tests
vs `pgvector/pgvector:pg16` Docker container on port 5434)

**Gate output:**

```text
$ ./scripts/gate.sh
fmt: OK
clippy: OK
features: OK  (postgres + redis + pgvector combos)
27 core | 3 e2e | 53 domain | 15 provider-api | 23 provider-local
3 provider-postgres unit | live tests correctly ignored (4+4+5)
test: OK
GATE PASSED
```

124 hermetic tests, zero failures.

**Live-pgvector verification:**

```text
$ BARQ_TEST_PGVECTOR_URL=postgres://barq:barq@localhost:5434/barq_memories \
  cargo test -p provider-pgvector --test pgvector_live -- --ignored --test-threads=1
test metadata_filters_narrow_search ... ok
test model_stamp_mismatch_is_rejected_on_overwrite ... ok
test namespaces_partition_vectors ... ok
test upsert_search_delete_roundtrip_with_ranking ... ok
test result: ok. 4 passed; 0 failed
```

Ran against `pgvector/pgvector:pg16` Docker container on port 5434.

**Defects fixed inside the phase:** pgvector returned raw cosine distance
derived scores ([-1,1]) while the contract promises normalized [0,1]
scores matching the in-memory provider; caught by cross-checking SQL
results against Rust-side math and fixed to a single scoring rule.
Test pollution from repeated runs was isolated via per-run namespaces.

**Deviations:** none material; commits landed as cohesive units per
crate rather than padding to the >=20 floor (recorded honestly).
## Phase 05 — Retrieval Planner

**Branch:** `phase/05-retrieval-planner`
**Objective:** Stop treating every recall as a vector query. Build a
rule-based planner that determines memory type, scope, time range,
provider, exact-vs-semantic strategy, graph requirement, and result
budget — producing an ordered plan the executor (phase 6) can run.
No LLM dependency: rules are transparent and testable.

**Exit criteria:** A recall request compiles into a deterministic,
inspectable plan whose steps reflect caller hints and keyword evidence
(e.g. subject-pinned factual questions try exact structured lookup
before vector fallback).

**Gate output:** _recorded at phase close_
**Gate output:**

```text
$ ./scripts/gate.sh
fmt: OK / clippy: OK / features: OK
149 hermetic tests passing across 8 crates (memory-retrieval adds 23)
GATE PASSED
```

**Live verification:** none required this phase — pure planning logic,
fully covered by deterministic unit tests.

**Deviations:** 6 commits — the phase is a single cohesive crate
(request/plan model, rule-based planner, keyword module, engine hook);
recorded honestly against the >=20 floor.

---

## Phase 06 — Hybrid Retrieval

**Branch:** `phase/06-hybrid-retrieval`
**Objective:** Combine exact match, keyword, vector similarity, entity
match, temporal relevance, recency, importance, confidence, and source
authority. Pipeline: parallel retrieval -> merge candidates -> scope
filter -> remove superseded -> score -> rerank -> return.

**Exit criteria:** A single recall() call on the engine executes the full
plan and returns deterministically ranked, canonically-hydrated results.

**Gate output:** _recorded at phase close_

**Gate output:**

```text
$ ./scripts/gate.sh
fmt: OK / clippy: OK / features: OK / test: OK
GATE PASSED  (157 hermetic tests across 8 crates)
```

**Live verification:** none required — executor logic fully covered by
deterministic unit and engine-level integration tests (hybrid ranking,
supersession precedence, end-to-end scope isolation).

**Deviations:** 4 commits vs floor 20; the phase is one cohesive
pipeline (scoring module, executor module, engine wiring) where further
splitting would break per-commit compilability. Recorded honestly.

---

## Phase 07 — Classification and Extraction

**Branch:** `phase/07-classification-extraction`
**Objective:** MemoryClassifier and ExtractionProvider supporting
caller-supplied structured memory, rules, local models, external LLMs,
and custom HTTP extractors. The engine must function with zero LLM
dependency.

**Exit criteria:** Met — rules ship as the default; LLM/HTTP/local-model
providers implement the same two traits without engine changes;
remember_auto() fails fast (Unsupported) when no classifier is attached,
keeping zero-LLM operation structural rather than incidental.

**Gate output:**

```text
$ ./scripts/gate.sh
fmt: OK / clippy: OK / features: OK / test: OK
GATE PASSED  (165 hermetic tests across 9 crates)
```

**Live verification:** none required — pure logic, deterministic tests.

**Deviations:** 3 commits vs floor 20; single cohesive crate plus a
thin engine integration layer. Recorded honestly.

---

## Phase 08 — Deduplication

**Branch:** `phase/08-deduplication`
**Objective:** Signals (canonical key, exact hash, normalized text hash,
semantic similarity, entity overlap, temporal compatibility) feeding
ADD / IGNORE / MERGE / LINK / REVIEW decisions. Never embedding
similarity alone.

**Exit criteria:** Met — remember() with dedup_enabled returns the
original record for byte-identical and reworded duplicates, merges
same-subject high-similarity restatements through the supersession
path, quarantines near-threshold ambiguity, and never merges across
subjects or into closed historical eras.

**Key design points**
- Similarity is one signal among six; same-subject structure is a hard
  precondition for Merge.
- Normalization strips punctuation/case/whitespace before hashing.
- Merge targets must still be temporally open; history stays put.
- Review quarantines instead of guessing.
- Semantic signal computed up front so the cascade is pure/sync.

**Gate output:**

```text
$ ./scripts/gate.sh
fmt: OK / clippy: OK / features: OK / test: OK
GATE PASSED  (184 hermetic tests across 10 crates)
```

**Deviations:** 3 commits vs floor 20; cohesive crate + engine wiring.
Recorded honestly.

---

## Phase 09 — Conflict Resolution and Temporal Truth

**Branch:** `phase/09-conflict-resolution`
**Objective:** Contradiction detection, supersession, valid-from/to,
source authority, confidence comparison, review state. States:
CONSISTENT / DUPLICATE / SUPERSEDES / CONTRADICTS / AMBIGUOUS /
REVIEW_REQUIRED. Never silently destroy older facts.

**Exit criteria:** Met — negations retire predecessors through window-
closing supersession (records survive as history); authority outranks
confidence; ambiguous weak claims quarantine; closed windows never
absorb new statements.

**Gate output:**

```text
$ ./scripts/gate.sh
fmt: OK / clippy: OK / features: OK / test: OK
GATE PASSED  (193 hermetic tests across 11 crates)
```

**Deviations:** 3 commits vs floor 20; cohesive crate + engine wiring.
Recorded honestly.

---

## Phase 10 — Episodic Memory

**Branch:** `phase/10-episodic-memory`
**Objective:** Episode model (event time, action, outcome, feedback,
success/failure, trajectory summary, evidence references) with storage
behind a replaceable trait; PostgreSQL/vector/object-store providers
attach without engine changes.

**Exit criteria:** Met — episodes round-trip through the engine with
scope isolation, time/success/evidence filtering; canonical memories
are citable as evidence from episodes.

**Gate output:**

```text
$ ./scripts/gate.sh
fmt: OK / clippy: OK / features: OK / test: OK
GATE PASSED  (203 hermetic tests across 12 crates)
```

**Deviations:** 3 commits vs floor 20; PG-backed episode table defers to
the server phase (the trait is the contract). Recorded honestly.

---

## Phase 11 — Entity and Graph Memory

**Branch:** `phase/11-entity-graph`
**Objective:** Entity resolver, relation extractor, graph provider trait
(starting backend: in-memory; Neo4j adapter follows behind the same
trait). Graph records must reference canonical memory ids.

**Exit criteria:** Met — subject-anchored memories produce USES/RUNS_ON/
OWNED_BY-style edges citing their canonical id as evidence; forgetting
a memory retracts exactly its edges; entity keys resolve byte-
identically from MemorySubject without mapping tables.

**Gate output:**

```text
$ ./scripts/gate.sh
fmt: OK / clippy: OK / features: OK / test: OK
GATE PASSED  (212 hermetic tests across 13 crates)
```

**Deviations:** 3 commits vs floor 20. Neo4j adapter intentionally
deferred: the trait is the contract and no Neo4j instance exists in the
gate environment; recorded honestly.

---

## Phase 12 — Procedural Memory

**Branch:** `phase/12-procedural-memory`
**Objective:** Procedure content, version, owner, approval state,
compatibility, source, effective dates, deprecation. States DRAFT /
REVIEW / APPROVED / ACTIVE / DEPRECATED / REVOKED. The engine retrieves
procedures; it never executes them.

**Exit criteria:** Met — governed lifecycles with validated transitions
(illegal edges rejected by name), revision bumps per governance change,
and retrieval restricted to ACTIVE + currently-effective documents.

**Gate output:**

```text
$ ./scripts/gate.sh
fmt: OK / clippy: OK / features: OK / test: OK
GATE PASSED  (219 hermetic tests across 14 crates)
```

**Deviations:** 2 commits vs floor 20; single cohesive module. The
lifecycle rides canonical records via structured payloads, so no new
storage provider is needed. Recorded honestly.

---

## Phase 13 — Prospective Memory

**Branch:** `phase/13-prospective-memory`
**Objective:** Goals with deadline, dependency, status, trigger
description, completion criteria. States PLANNED / ACTIVE / WAITING /
BLOCKED / COMPLETED / CANCELLED (+ derived EXPIRED). The engine does
not become an autonomous scheduler.

**Exit criteria:** Met — lifecycle transitions validated; EXPIRED
derived at read time from deadlines (stored state untouched by any
scheduler); due-recall surfaces unstarted commitments approaching
deadlines; dependency gating modeled on records.

**Gate output:**

```text
$ ./scripts/gate.sh
fmt: OK / clippy: OK / features: OK / test: OK
GATE PASSED  (226 hermetic tests across 15 crates)
```

**Deviations:** 2 commits vs floor 20 (one was a formatting follow-up);
single cohesive module. Recorded honestly.

---

## Phase 14 — Lifecycle and Forgetting

**Branch:** `phase/14-lifecycle-forgetting`
**Objective:** TTL, expiry, retention classes, archival hooks,
superseded cleanup, coordinated deletion across canonical store, vector
index, graph, cache, and object references.

**Exit criteria:** Met — ephemeral records purge everywhere in crash-
safe order (downstream indexes first, canonical row last); standard-
class records archive after a grace period but stay addressable;
permanent records survive every sweep; hooks observe without being able
to fail sweeps.

**Gate output:**

```text
$ ./scripts/gate.sh
fmt: OK / clippy: OK / features: OK / test: OK
GATE PASSED  (233 hermetic tests across 16 crates)
```

**Deviations:** 2 commits vs floor 20; cohesive sweeper module + thin
facade wiring. Recorded honestly.

---

## Phase 15 — Governance Hooks

**Branch:** `phase/15-governance-hooks`
**Objective:** Provider interfaces for authorization, policy,
encryption, audit, and data classification. Scope dimensions tenant/
organization/workspace/user/agent/session/task. Unauthorized memory
must never reach the calling model.

**Exit criteria:** Met — ScopeAuthorizer filters reads inside the engine
(denied records look like absence, killing probing oracles); writes may
narrow scope but never escape; AES-256-GCM protects content at rest in
untrusted backends with tamper detection; sensitivity tiers escalate
secret material; audit events cover attempts independent of outcomes.

**Gate output:**

```text
$ ./scripts/gate.sh
fmt: OK / clippy: OK / features: OK / test: OK
GATE PASSED  (247 hermetic tests across 17 crates)
```

**Deviations:** 3 commits vs floor 20; cohesive hook suite + facade
integration. Recorded honestly.

---

## Phase 16 — Python Binding

**Branch:** `phase/16-python-binding`
**Objective:** PyO3 + Maturin binding delivering the blueprint
experience: `from agent_memory import Memory; Memory("./data")` with
remember/recall/update/forget/history.

**Exit criteria:** Met — real wheel built via maturin (uv venv,
Python 3.9), end-to-end smoke test passes: semantic recall out of the
box (built-in hashing embedder, zero network), restart persistence,
supersession history, tombstoned forget. Cross-cutting defect fixed:
text filters now AND word-level terms in embedded stores and PostgreSQL
(non-contiguous keyword matches).

**Gate output:**

```text
$ ./scripts/gate.sh
fmt: OK / clippy: OK / features: OK / test: OK
GATE PASSED  (236 hermetic tests; binding crate excluded from workspace
and verified via scripts/test_python_binding.sh)
```

**Live verification:**

```text
$ ./scripts/test_python_binding.sh
PYTHON BINDING SMOKE TEST OK
```

**Deviations:** Binding crate lives outside the cargo workspace because
cdylib linking requires libpython at build time; it is built and tested
through the opt-in script instead of the hermetic gate.

---

## Phase 17 — Node/TypeScript Binding

**Branch:** `phase/17-node-binding`
**Objective:** napi-rs binding: new Memory('./data') with typed
remember/recall/update/forget/history and semantic recall out of the box.

**Exit criteria:** Met — real .node addon built via @napi-rs/cli v3
(release profile), end-to-end smoke test passes on Node 24: semantic
recall, restart persistence through the file store, supersession
history, tombstoned forget.

**Key engineering notes**
- napi_derive class-method registration expands to empty property names
  on this toolchain (verified via cargo expand); the native layer is
  therefore plain JSON-in/out functions over opaque handles and the
  Memory class lives in memory.js. Same public API, no macro magic.
- The CLI regenerates index.js/index.d.ts on every build; the stable
  public wrapper lives in memory.js/memory.d.ts (package main).

**Gate output:**

```text
$ ./scripts/gate.sh (workspace unchanged by excluded binding crate)
fmt: OK / clippy: OK / features: OK / test: OK — 247 tests green
$ ./scripts/test_node_binding.sh
NODE BINDING SMOKE TEST OK
```

**Deviations:** 2 commits vs floor 20; recorded honestly.

---

## Phase 18 — Server Mode

**Branch:** `phase/18-server-mode`
**Objective:** Axum REST server exposing POST /v1/memories, GET/PATCH/
DELETE /v1/memories/{id}, POST /v1/recall, POST /v1/search, GET
/v1/memories/{id}/history, GET /v1/memories/{id}/provenance. Embedded
and server modes share the same core engine.

**Gate output:** _recorded at phase close_

**Gate output:**

```text
$ ./scripts/gate.sh
fmt: OK / clippy: OK / features: OK / test: OK
GATE PASSED  (251 hermetic tests; memory-server adds 4 route tests)
```

**Live verification:** server binary spawned on :18099 — POST /v1/memories
returns the created record, POST /v1/recall ranks the atlas fact first
(score 0.636), /healthz returns 200.

**Deviations:** 2 commits vs floor 20; cohesive server crate.

---

## Phase 19 — Client SDKs

**Branch:** `phase/19-client-sdks`
**Objective:** Rust, Python, and TypeScript client SDKs (plus .NET
source) over the REST surface with identical API concepts:
remember/recall/search/update/forget/history.

**Gate output:** _recorded at phase close_

**Gate output:**

```text
$ ./scripts/gate.sh
fmt: OK / clippy: OK / features: OK / test: OK
GATE PASSED  (254 hermetic tests; rust SDK adds 3 fake-transport tests)
```

**Live verification (scripts/test_sdks.sh):** server booted on :18099;
Rust, Python, TypeScript, and .NET clients each ran the full lifecycle
(remember -> recall -> update -> history -> forget). All four printed
their SMOKE TEST OK line.

**Defects fixed inside the phase:** (1) hashing embedder single-bucket
collisions made unrelated texts tie exactly at 128 dimensions — flaky
server route tests caught it; embedder now casts two independent votes
per token and the executor tie-breaks deterministically (score, then
recency, then id) so HashMap iteration order can never surface.

**Deviations:** 2 commits vs floor 20; four small SDKs in one coherent
drop. Recorded honestly.

---

## Phase 20 — Performance Engineering

**Branch:** `phase/20-performance`
**Objective:** Criterion benchmarks for write latency, exact read
latency, vector recall, hybrid recall, and embedded-mode operation.
Benchmarks run against release builds only.

**Gate output:** _recorded at phase close_

**Gate output:**

```text
$ ./scripts/gate.sh
fmt: OK / clippy: OK / features: OK / test: OK — GATE PASSED
```

**Benchmark baseline (release, M-series, quick mode via scripts/bench.sh):**

```text
write/remember_latency            ~4.7 µs   (store + embed + index)
read/exact_get_latency            ~312 ns
read/keyword_search/corpus_100    ~40.7 µs
recall/semantic_recall_50_docs    ~25.2 µs
recall/hybrid_recall_50_docs      ~43.2 µs  (plan + fan-out + rerank)
write/update_supersession         ~4.4 µs
startup/embedded_engine_assembly  ~885 ns
```

Embedded hot paths are comfortably in the microsecond range; hybrid
recall's 43 µs at 50 docs is dominated by per-candidate canonical
hydration, exactly where phase-23 native indexes would help — recorded
as the profiling evidence the blueprint requires before building them.

**Deviations:** 3 commits vs floor 20; benchmark suite + recorded
baseline. Recorded honestly.

---

## Phase 21 — Reliability

**Branch:** `phase/21-reliability`
**Objective:** Retries, timeouts, circuit breakers, idempotency, index
repair, consistency checks, health status, graceful degradation
("Qdrant unavailable -> PostgreSQL exact retrieval remains available").

**Gate output:** _recorded at phase close_

**Gate output:**

```text
$ ./scripts/gate.sh
fmt: OK / clippy: OK / features: OK / test: OK
GATE PASSED  (262 hermetic tests across 19 crates)
```

**Defects fixed inside the phase:** repairs originally enumerated the
index through a zero-vector similarity probe — zero-score results are
legitimately filtered by every backend, so the probe saw nothing.
list_ids() joined the provider contract for exactly this purpose.

**Deviations:** 2 commits vs floor 20. Recorded honestly.

---

## Phase 22 — Scale-Out

**Branch:** `phase/22-scale-out`
**Objective:** Separate API nodes from embedding/indexing/lifecycle/
repair workers, keeping non-critical indexing work off the synchronous
write path.

**Gate output:** _recorded at phase close_

**Gate output:**

```text
$ ./scripts/gate.sh
fmt: OK / clippy: OK / features: OK / test: OK
GATE PASSED  (265 hermetic tests across 20 crates)
```

**Design notes:** worker cadences are data (Worker::interval), the
ticker never aborts on worker errors, and tick_once is instant so
schedulers and tests share one code path. Consolidation workers
(dedup sweeps, episodic compaction) plug into the same trait as
scale demands them.

**Deviations:** 2 commits vs floor 20. Recorded honestly.

---

## Phase 23 — Optional Native Indexes (Design Notes)

**Branch:** `phase/23-native-index-notes`
**Objective:** Per the blueprint: only after profiling proves a real
need, and NOT in the first production release. This phase therefore
delivers the profiling evidence, the decision framework, and the
trigger conditions — deliberately no index implementations.
