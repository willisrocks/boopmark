-- Reviewed LLM consolidation job (tag-management spec, "3. Reviewed LLM
-- consolidation"). One row per user for the latest run, so the review page
-- can poll it. The run only writes merge proposals (tag_merge_proposals);
-- tags change when the user applies them.
CREATE TABLE tag_consolidation_jobs (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    -- A new run replaces the row; a finishing run only updates its own.
    run_id UUID NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed')),
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    -- A user-facing message; never a key, prompt or model answer.
    error TEXT,
    proposals INTEGER NOT NULL DEFAULT 0,
    pairs_checked INTEGER NOT NULL DEFAULT 0,
    input_tokens BIGINT NOT NULL DEFAULT 0,
    output_tokens BIGINT NOT NULL DEFAULT 0
);
