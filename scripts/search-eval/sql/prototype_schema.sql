-- Prototype of the recommended hybrid-search schema (spec:
-- docs/superpowers/specs/2026-10-31-hybrid-article-search-design.md).
-- NOT a real migration: it lives here so CI (plain postgres, no pgvector)
-- keeps passing. The real migration would be 015_bookmark_content_search.sql.
--
-- Applied on top of migrations 001-014. Idempotent: drops and recreates.

CREATE EXTENSION IF NOT EXISTS vector;

DROP TABLE IF EXISTS bookmark_chunks;
DROP TABLE IF EXISTS bookmark_contents;

-- One row per bookmark whose article text we tried to fetch.
CREATE TABLE bookmark_contents (
    bookmark_id UUID PRIMARY KEY REFERENCES bookmarks(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    status TEXT NOT NULL,              -- pending | fetched | failed | skipped
    error TEXT,
    text TEXT,                         -- extracted article text, capped
    text_chars INT NOT NULL DEFAULT 0,
    content_hash TEXT,
    -- Weighted lexical document: title A, description B, url C, body D.
    -- Maintained by the app (title/description live on bookmarks), not a
    -- generated column, because a generated column cannot read another table.
    search tsvector,
    fetched_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_bookmark_contents_user ON bookmark_contents(user_id);
CREATE INDEX idx_bookmark_contents_search ON bookmark_contents USING GIN(search);

-- Embedded passages. user_id is denormalised so the ANN query can filter on it.
CREATE TABLE bookmark_chunks (
    bookmark_id UUID NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    ord INT NOT NULL,
    text TEXT NOT NULL,
    model TEXT NOT NULL,               -- e.g. 'bge-small-en-v1.5'
    embedding vector(384) NOT NULL,
    PRIMARY KEY (bookmark_id, model, ord)
);
CREATE INDEX idx_bookmark_chunks_user ON bookmark_chunks(user_id);
CREATE INDEX idx_bookmark_chunks_hnsw ON bookmark_chunks
    USING hnsw (embedding vector_cosine_ops);
