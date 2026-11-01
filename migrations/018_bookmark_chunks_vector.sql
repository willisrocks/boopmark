-- Full-article search, vector leg (spec:
-- docs/superpowers/specs/2026-10-31-hybrid-article-search-design.md).
-- Needs pgvector >= 0.8 (halfvec, hnsw.iterative_scan). Fly MPG: enable
-- `vector` in the dashboard first.

CREATE EXTENSION IF NOT EXISTS vector;

-- The embedded passages of each bookmark. An article of at least 300
-- characters is split into windows; anything else (a failed or skipped
-- fetch, a short page) has one chunk, ord 0, made of its metadata.
CREATE TABLE bookmark_chunks (
    bookmark_id UUID NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE,
    user_id     UUID NOT NULL,
    ord         INT  NOT NULL,
    text        TEXT NOT NULL,      -- the passage, shown as a snippet
    embedding   halfvec NOT NULL,   -- no fixed dimension: one schema for every backend
    model       TEXT NOT NULL,      -- Embedder::model_id() that produced this embedding
    PRIMARY KEY (bookmark_id, ord)
);
CREATE INDEX idx_bookmark_chunks_user ON bookmark_chunks(user_id);
-- One partial expression index per supported size: text-embedding-3-large
-- at 1,024 (production, the openai default) and bge-small at 384 (local).
-- The vector leg casts to the configured size and filters on it, so it uses
-- the matching index.
CREATE INDEX idx_bookmark_chunks_hnsw_1024 ON bookmark_chunks
    USING hnsw ((embedding::halfvec(1024)) halfvec_cosine_ops)
    WHERE vector_dims(embedding) = 1024;
CREATE INDEX idx_bookmark_chunks_hnsw_384 ON bookmark_chunks
    USING hnsw ((embedding::halfvec(384)) halfvec_cosine_ops)
    WHERE vector_dims(embedding) = 384;

-- The embed queue: a row needs (re-)embedding when it is not pending and its
-- `embedded_model` differs from the configured embedder. Every existing row
-- has `embedded_model = NULL`, so this is also the backfill.
ALTER TABLE bookmark_contents
    ADD COLUMN embed_attempts        INT NOT NULL DEFAULT 0,
    ADD COLUMN embed_next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    ADD COLUMN embed_error           TEXT;
CREATE INDEX idx_bookmark_contents_embed_queue ON bookmark_contents(embed_next_attempt_at)
    WHERE status <> 'pending' AND embedded_model IS NULL;

-- As in 016, plus the chunks: a new URL drops them (the old page's passages
-- must not match), a title or description edit re-embeds (both are in the
-- chunk headers), and a tag edit re-embeds a metadata chunk (300 =
-- domain::chunking::MIN_CHUNKED_TEXT_CHARS; only that chunk holds tags).
CREATE OR REPLACE FUNCTION bookmarks_sync_content() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' OR NEW.url IS DISTINCT FROM OLD.url THEN
        INSERT INTO bookmark_contents (bookmark_id, user_id, search, meta_text)
        VALUES (NEW.id, NEW.user_id,
                bookmark_search_document(NEW.title, NEW.description, NEW.url, NEW.tags, NULL),
                concat_ws(' ', NEW.title, NEW.description))
        ON CONFLICT (bookmark_id) DO UPDATE SET
            status = 'pending', attempts = 0, next_attempt_at = now(), claim_token = NULL, error = NULL,
            text = NULL, text_chars = 0, content_hash = NULL, fetched_at = NULL,
            embedded_model = NULL, embed_attempts = 0, embed_next_attempt_at = now(), embed_error = NULL,
            search = EXCLUDED.search, meta_text = EXCLUDED.meta_text, updated_at = now();
        IF TG_OP = 'UPDATE' THEN
            DELETE FROM bookmark_chunks WHERE bookmark_id = NEW.id;
        END IF;
        PERFORM pg_notify('bookmark_ingest', '');
    ELSE
        UPDATE bookmark_contents SET
            search = bookmark_search_document(NEW.title, NEW.description, NEW.url, NEW.tags, text),
            meta_text = concat_ws(' ', NEW.title, NEW.description),
            embedded_model = CASE
                WHEN (OLD.title, OLD.description) IS DISTINCT FROM (NEW.title, NEW.description)
                  OR (OLD.tags IS DISTINCT FROM NEW.tags AND text_chars < 300)
                THEN NULL ELSE embedded_model END,
            embed_attempts = 0, embed_next_attempt_at = now(),
            updated_at = now()
        WHERE bookmark_id = NEW.id;
    END IF;
    RETURN NULL;
END
$$;
