-- Full-article search, vector leg (spec:
-- docs/superpowers/specs/2026-10-31-hybrid-article-search-design.md).
-- Only the parts that work without pgvector. The `vector` extension and the
-- bookmark_chunks table are created at startup when available
-- (server/src/adapters/postgres/vector_schema.rs), so the app boots and
-- searches lexically on a database without the extension.

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
        -- bookmark_chunks exists only once the vector schema is ready.
        IF TG_OP = 'UPDATE' AND to_regclass('bookmark_chunks') IS NOT NULL THEN
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
