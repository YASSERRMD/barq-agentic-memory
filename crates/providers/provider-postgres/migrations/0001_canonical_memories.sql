-- Canonical memory table: current state of every record.
CREATE TABLE IF NOT EXISTS memories (
    id                  UUID PRIMARY KEY,
    namespace           TEXT NOT NULL DEFAULT 'barq',
    memory_type         TEXT NOT NULL,
    subtype             TEXT,

    tenant_id           TEXT,
    organization_id     TEXT,
    workspace_id        TEXT,
    user_id             TEXT,
    agent_id            TEXT,
    session_id          TEXT,
    task_id             TEXT,

    subject_type        TEXT,
    subject_id          TEXT,
    subject_display     TEXT,

    content_text        TEXT NOT NULL,
    content_structured  JSONB,
    content_tags        TEXT[] NOT NULL DEFAULT '{}',

    confidence          REAL NOT NULL DEFAULT 0.5,
    importance          REAL NOT NULL DEFAULT 0.5,

    valid_from          TIMESTAMPTZ,
    valid_to            TIMESTAMPTZ,

    status              TEXT NOT NULL DEFAULT 'active',
    version             BIGINT NOT NULL DEFAULT 1,
    supersedes          UUID,

    provenance          JSONB NOT NULL,
    retention           JSONB NOT NULL,

    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Append-only revision ledger: supersession history is never destroyed.
CREATE TABLE IF NOT EXISTS memory_versions (
    id           BIGSERIAL PRIMARY KEY,
    memory_id    UUID NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    version      BIGINT NOT NULL,
    status       TEXT NOT NULL,
    content_text TEXT NOT NULL,
    recorded_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (memory_id, version)
);

-- Scope indexes (blueprint §Phase 2).
CREATE INDEX IF NOT EXISTS idx_memories_namespace   ON memories (namespace);
CREATE INDEX IF NOT EXISTS idx_memories_tenant      ON memories (tenant_id)      WHERE tenant_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_memories_workspace   ON memories (workspace_id)   WHERE workspace_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_memories_user        ON memories (user_id)        WHERE user_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_memories_agent       ON memories (agent_id)       WHERE agent_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_memories_session     ON memories (session_id)     WHERE session_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_memories_task        ON memories (task_id)        WHERE task_id IS NOT NULL;

-- Type, subject, status, validity, creation time.
CREATE INDEX IF NOT EXISTS idx_memories_type        ON memories (memory_type);
CREATE INDEX IF NOT EXISTS idx_memories_subject     ON memories (subject_type, subject_id);
CREATE INDEX IF NOT EXISTS idx_memories_status      ON memories (status);
CREATE INDEX IF NOT EXISTS idx_memories_validity    ON memories (valid_from, valid_to);
CREATE INDEX IF NOT EXISTS idx_memories_created_at  ON memories (created_at DESC);
CREATE INDEX IF NOT EXISTS idx_memories_supersedes  ON memories (supersedes) WHERE supersedes IS NOT NULL;
