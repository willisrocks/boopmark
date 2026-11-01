-- Full-article search, lexical legs (spec:
-- docs/superpowers/specs/2026-10-31-hybrid-article-search-design.md).
-- The vector leg and its chunks table come in a later migration.

CREATE EXTENSION IF NOT EXISTS pg_trgm;

-- The weighted lexical document: title A, description B, URL and tags C,
-- article body D. Every writer goes through this function so the weights
-- cannot drift between the trigger, the backfill and the ingest worker.
CREATE FUNCTION bookmark_search_document(
    title TEXT, description TEXT, url TEXT, tags TEXT[], body TEXT
) RETURNS tsvector
LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT setweight(to_tsvector('english', coalesce(title, '')), 'A') ||
           setweight(to_tsvector('english', coalesce(description, '')), 'B') ||
           setweight(to_tsvector('english', coalesce(url, '') || ' ' ||
                                 coalesce(array_to_string(tags, ' '), '')), 'C') ||
           setweight(to_tsvector('english', coalesce(body, '')), 'D')
$$;

-- One row per bookmark: the extracted article and the ingest queue state.
CREATE TABLE bookmark_contents (
    bookmark_id     UUID PRIMARY KEY REFERENCES bookmarks(id) ON DELETE CASCADE,
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    status          TEXT NOT NULL DEFAULT 'pending'
                    CHECK (status IN ('pending', 'fetched', 'failed', 'skipped')),
    attempts        INT NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    error           TEXT,
    text            TEXT,                       -- extracted article text, capped by the worker
    text_chars      INT NOT NULL DEFAULT 0,
    content_hash    TEXT,                       -- sha256(text)
    search          tsvector NOT NULL,          -- bookmark_search_document(...)
    meta_text       TEXT NOT NULL DEFAULT '',   -- title || ' ' || description, for the trigram leg
    embedded_model  TEXT,                       -- Embedder::model_id() of the chunks (vector leg)
    fetched_at      TIMESTAMPTZ,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_bookmark_contents_user ON bookmark_contents(user_id);
CREATE INDEX idx_bookmark_contents_search ON bookmark_contents USING GIN(search);
CREATE INDEX idx_bookmark_contents_meta_trgm ON bookmark_contents USING GIN(meta_text gin_trgm_ops);
CREATE INDEX idx_bookmark_contents_queue ON bookmark_contents(next_attempt_at)
    WHERE status = 'pending';

-- Keep bookmark_contents in step with every write to bookmarks, in the same
-- transaction: a new bookmark or a changed URL is (re)queued for ingest,
-- and a title, description or tag edit rebuilds the lexical document around
-- the stored article text.
CREATE FUNCTION bookmarks_sync_content() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' OR NEW.url IS DISTINCT FROM OLD.url THEN
        INSERT INTO bookmark_contents (bookmark_id, user_id, search, meta_text)
        VALUES (NEW.id, NEW.user_id,
                bookmark_search_document(NEW.title, NEW.description, NEW.url, NEW.tags, NULL),
                concat_ws(' ', NEW.title, NEW.description))
        ON CONFLICT (bookmark_id) DO UPDATE SET
            status = 'pending', attempts = 0, next_attempt_at = now(), error = NULL,
            text = NULL, text_chars = 0, content_hash = NULL, fetched_at = NULL,
            search = EXCLUDED.search, meta_text = EXCLUDED.meta_text, updated_at = now();
        PERFORM pg_notify('bookmark_ingest', '');
    ELSE
        UPDATE bookmark_contents SET
            search = bookmark_search_document(NEW.title, NEW.description, NEW.url, NEW.tags, text),
            meta_text = concat_ws(' ', NEW.title, NEW.description),
            updated_at = now()
        WHERE bookmark_id = NEW.id;
    END IF;
    RETURN NULL;
END
$$;

CREATE TRIGGER bookmarks_sync_content
    AFTER INSERT OR UPDATE OF title, description, url, tags ON bookmarks
    FOR EACH ROW EXECUTE FUNCTION bookmarks_sync_content();

-- Backfill: every existing bookmark starts pending, already searchable by
-- its metadata. The ingest worker drains the queue with per-host throttling.
INSERT INTO bookmark_contents (bookmark_id, user_id, search, meta_text)
SELECT id, user_id,
       bookmark_search_document(title, description, url, tags, NULL),
       concat_ws(' ', title, description)
FROM bookmarks;
