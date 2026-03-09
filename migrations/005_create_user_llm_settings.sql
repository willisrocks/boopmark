CREATE TABLE user_llm_settings (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    anthropic_api_key_encrypted BYTEA,
    anthropic_model TEXT NOT NULL DEFAULT 'claude-haiku-4-5',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
