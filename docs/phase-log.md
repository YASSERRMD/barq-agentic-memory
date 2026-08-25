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
