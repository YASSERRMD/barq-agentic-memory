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
dependency.

**Gate output:** _recorded at phase close_

**Deviations:** _none yet_
