# Architecture — barq-agentic-memory

A standalone, high-performance, framework-neutral **memory engine** for AI
agents. It runs embedded (in-process) or as a server and is consumed by any
agent framework through native bindings or REST/gRPC.

## Boundary

This is a *memory engine*, not an agent framework. It does not plan tasks,
execute tools, orchestrate agents, or own the agent lifecycle.

Public surface:

```text
remember() recall() search() update() forget() history()
```

Internally it manages classification, routing, deduplication, conflict
resolution, indexing, retrieval, ranking, temporal validity, provenance,
retention, scoping, and provider selection.

## Memory model

Five functional types:

| Type | Meaning |
|---|---|
| Working | Active state now |
| Episodic | Past events, interactions, outcomes |
| Semantic | Durable facts, preferences, entities |
| Procedural | Instructions, skills, procedures |
| Prospective | Future goals, commitments, unfinished work |

Operational views (profile, conversation, entity, relational, temporal,
shared) are scopes/indexes over these types, never new cognitive types.

## Crate map

```text
crates/
├── memory-domain/         Canonical model: taxonomy, record, scope, errors,
│                          queries, configuration. No I/O dependencies.
├── memory-provider-api/   Provider traits: store, vector, working.
├── memory-core/           Registry + config-driven assembly.
└── ...                    Retrieval, lifecycle, classifier, conflict,
                           policy, server, bindings, providers (later phases)
```

Dependency rule: `memory-domain` depends on nothing but serde/chrono/uuid/
thiserror. Provider crates depend on domain. The core depends on provider
API only — never on concrete backends. Server/bindings depend on core.

## Data flow

```text
Agent / SDK / REST / gRPC
        ↓
   Memory Engine API  (remember / recall / search / update / forget / history)
        ↓
   Memory Core        (classify → dedup → resolve conflicts → route)
        ↓
Provider Registry    (store │ vector │ working │ graph │ object)
```

Embedded and server mode share the same core engine; deployment is a
configuration choice, not a fork in behavior.

## Invariants

1. **History is never destroyed** — updates create successors via
   `supersedes`; deletion tombstones until coordinated sweeps run.
2. **Scope isolation is enforced at every read** — a pinned scope dimension
   must match exactly; wildcards match anything.
3. **Zero-LLM operation** — callers may supply structured memories; the
   engine functions fully without any model dependency.
4. **Providers are replaceable** — the engine API never leaks backend types;
   vectors carry model/version stamps to prevent silent index corruption.
