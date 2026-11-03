-- Page capture for unfetchable articles (spec:
-- docs/superpowers/specs/2026-11-03-page-capture-design.md).
-- The browser sends the HTML it rendered; the server parses it with the same
-- Readability pass as a fetched page and stores the text through the existing
-- chunk + embed pipeline. One migration carries every schema change.

-- 1. Where the stored text came from. NULL when no article text is stored,
--    and NULL again when the bookmark's URL changes (the trigger below).
ALTER TABLE bookmark_contents
    ADD COLUMN source TEXT CHECK (source IN ('fetched', 'captured', 'scraped'));
UPDATE bookmark_contents SET source = 'fetched'
    WHERE status = 'fetched' AND text IS NOT NULL;
CREATE INDEX idx_bookmark_contents_source ON bookmark_contents(user_id, source)
    WHERE status = 'fetched';

-- 2. Captured HTML accepted but not yet parsed (the rare `queued` answer).
--    One row per bookmark: a re-capture replaces the pending payload.
CREATE TABLE bookmark_page_captures (
    bookmark_id UUID PRIMARY KEY REFERENCES bookmarks(id) ON DELETE CASCADE,
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    page_url    TEXT NOT NULL,          -- X-Page-Url after validation
    html_gz     BYTEA NOT NULL,         -- gzip, <= PAGE_CAPTURE_MAX_BYTES
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_bookmark_page_captures_age ON bookmark_page_captures(created_at);

-- 3. Scraper spend, one row per calendar month, survives restarts.
CREATE TABLE scraper_usage (
    month      DATE PRIMARY KEY,        -- first day of the month, UTC
    pages      BIGINT NOT NULL DEFAULT 0,
    failures   BIGINT NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 4. The 018 trigger, plus `source = NULL` on a URL change: the old page's
--    captured text must not be attributed to the new URL.
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
            text = NULL, text_chars = 0, content_hash = NULL, fetched_at = NULL, source = NULL,
            embedded_model = NULL, embed_attempts = 0, embed_next_attempt_at = now(), embed_error = NULL,
            search = EXCLUDED.search, meta_text = EXCLUDED.meta_text, updated_at = now();
        -- bookmark_chunks exists only once the vector schema is ready.
        IF TG_OP = 'UPDATE' AND to_regclass('bookmark_chunks') IS NOT NULL THEN
            DELETE FROM bookmark_chunks WHERE bookmark_id = NEW.id;
        END IF;
        -- A capture queued for the old page would be indexed against the new
        -- URL; it is dropped with it.
        DELETE FROM bookmark_page_captures WHERE bookmark_id = NEW.id;
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
