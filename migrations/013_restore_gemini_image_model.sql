-- Migration 012 shipped on an earlier production branch and must remain
-- byte-for-byte intact so SQLx can validate databases that applied it. The
-- current application uses Gemini for image generation, so restore that
-- model as the canonical value and default without dropping the preserved
-- OpenAI settings data.
UPDATE user_llm_settings
SET image_generation_model = 'gemini-3.1-flash-lite-image'
WHERE image_generation_model = 'gpt-image-2';

ALTER TABLE user_llm_settings
    ALTER COLUMN image_generation_model
    SET DEFAULT 'gemini-3.1-flash-lite-image';
