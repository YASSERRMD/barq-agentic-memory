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
