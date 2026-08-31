ALTER TABLE user_llm_settings
    ADD COLUMN metadata_provider TEXT NOT NULL DEFAULT 'anthropic',
    ADD COLUMN openai_api_key_encrypted BYTEA,
    ADD COLUMN openai_model TEXT NOT NULL DEFAULT 'gpt-5.6-luna';

-- Existing production databases may have the Gemini image model from
-- migration 011. This release uses the OpenAI image adapter, so migrate the
-- legacy default to the only supported image model. Keep the existing
-- enablement flag and encrypted provider material untouched.
UPDATE user_llm_settings
SET image_generation_model = 'gpt-image-2'
WHERE image_generation_model IS NULL
   OR image_generation_model LIKE 'gemini-%';

-- The original migration used a shorthand Haiku default. Normalize only that
-- old default; preserve deliberately selected legacy/custom model identifiers.
UPDATE user_llm_settings
SET anthropic_model = 'claude-haiku-4-5-20251001'
WHERE anthropic_model = 'claude-haiku-4-5';

ALTER TABLE user_llm_settings
    ALTER COLUMN anthropic_model SET DEFAULT 'claude-haiku-4-5-20251001';
