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
