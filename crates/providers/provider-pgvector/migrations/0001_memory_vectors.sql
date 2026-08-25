-- One vector table per engine namespace set; rows carry the namespace
-- so multiple engines can share a database safely.
CREATE TABLE IF NOT EXISTS memory_vectors (
    memory_id     UUID PRIMARY KEY,
    namespace     TEXT NOT NULL,
    model         TEXT NOT NULL,
    model_version TEXT NOT NULL,
    dimensions    INT  NOT NULL,
    embedding     vector(384) NOT NULL,
    metadata      JSONB NOT NULL DEFAULT '{}'::jsonb
);

-- Cosine-distance index (pgvector HNSW). Created only when the column
-- dimensionality matches; dimension changes require reindexing.
CREATE INDEX IF NOT EXISTS idx_memory_vectors_hnsw
    ON memory_vectors USING hnsw (embedding vector_cosine_ops);

CREATE INDEX IF NOT EXISTS idx_memory_vectors_namespace
    ON memory_vectors (namespace);
