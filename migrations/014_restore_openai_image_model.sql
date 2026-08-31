-- Migration 013 was accidentally applied during an emergency deployment.
-- Preserve its immutable history, then restore the OpenAI image model used
-- by the current application and future settings rows.
UPDATE user_llm_settings
SET image_generation_model = 'gpt-image-2'
WHERE image_generation_model = 'gemini-3.1-flash-lite-image';

ALTER TABLE user_llm_settings
    ALTER COLUMN image_generation_model
    SET DEFAULT 'gpt-image-2';
