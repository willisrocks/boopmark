-- Canonical tags and aliases (tag-management spec, "Canonical tag model").
-- Additive only: bookmarks.tags keeps storing canonical names, and no
-- existing tag is rewritten here. Normalizing an existing library happens
-- only when the user applies a merge proposal.

-- One row per rename/merge/delete the user applies; the unit of undo.
CREATE TABLE tag_change_batches (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Commit order within a user: assigned while the per-user tag lock is
    -- held, unlike created_at, which is the transaction start time.
    seq BIGSERIAL NOT NULL UNIQUE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('normalize', 'llm_merge', 'rename', 'merge', 'delete')),
    -- The merges applied, for the history list.
    summary JSONB NOT NULL,
    undone_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_tag_change_batches_user_seq ON tag_change_batches(user_id, seq DESC);

-- One row per known spelling, keyed by its compact key.
CREATE TABLE tag_aliases (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    alias_key TEXT NOT NULL,
    -- A spelling seen, for display ("also: GitHub").
    alias TEXT NOT NULL,
    -- The tag stored on bookmarks.
    canonical TEXT NOT NULL,
    source TEXT NOT NULL CHECK (source IN ('normalize', 'llm', 'manual')),
    batch_id UUID REFERENCES tag_change_batches(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, alias_key)
);

CREATE INDEX idx_tag_aliases_user_canonical ON tag_aliases(user_id, canonical);

-- Before-images of the bookmarks a batch touched.
CREATE TABLE tag_change_snapshots (
    batch_id UUID NOT NULL REFERENCES tag_change_batches(id) ON DELETE CASCADE,
    bookmark_id UUID NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE,
    tags_before TEXT[] NOT NULL,
    tags_after TEXT[] NOT NULL,
    PRIMARY KEY (batch_id, bookmark_id)
);

CREATE INDEX idx_tag_change_snapshots_bookmark ON tag_change_snapshots(bookmark_id);

-- Before-images of the alias rows a batch inserted or repointed, so undo
-- can restore an alias that pointed somewhere else. A NULL canonical means
-- the row did not exist before the batch.
CREATE TABLE tag_change_alias_snapshots (
    batch_id UUID NOT NULL REFERENCES tag_change_batches(id) ON DELETE CASCADE,
    alias_key TEXT NOT NULL,
    alias_before TEXT,
    canonical_before TEXT,
    source_before TEXT,
    batch_id_before UUID,
    created_at_before TIMESTAMPTZ,
    PRIMARY KEY (batch_id, alias_key)
);

-- Merges suggested by the normalization job or the LLM review step.
-- Nothing changes until the user applies one.
CREATE TABLE tag_merge_proposals (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    canonical TEXT NOT NULL,
    members TEXT[] NOT NULL,
    source TEXT NOT NULL CHECK (source IN ('deterministic', 'llm')),
    confidence TEXT NOT NULL CHECK (confidence IN ('high', 'medium', 'low')),
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'applied', 'dismissed')),
    batch_id UUID REFERENCES tag_change_batches(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    decided_at TIMESTAMPTZ
);

CREATE INDEX idx_tag_merge_proposals_user_status ON tag_merge_proposals(user_id, status);
