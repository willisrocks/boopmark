-- Durable operation claims are separate from bookmark rows so a duplicate
-- request is rejected before enrichment/metadata/image side effects.  The
-- account-scoped key also keeps tenants isolated.
CREATE TABLE bookmark_create_operations (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    idempotency_key UUID NOT NULL,
    fingerprint_version SMALLINT NOT NULL,
    fingerprint TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending', 'completed')),
    bookmark_id UUID REFERENCES bookmarks(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, idempotency_key)
);

CREATE INDEX idx_bookmark_create_operations_bookmark_id
    ON bookmark_create_operations(bookmark_id);
