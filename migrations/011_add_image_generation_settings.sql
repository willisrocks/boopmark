ALTER TABLE user_llm_settings
    ADD COLUMN image_generation_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN gemini_api_key_encrypted BYTEA,
    ADD COLUMN image_generation_model TEXT NOT NULL DEFAULT 'gemini-3.1-flash-lite-image',
    ADD COLUMN image_generation_art_style TEXT NOT NULL DEFAULT 'Bold editorial illustration with clean geometric shapes, a limited vibrant color palette, subtle depth, and one clear visual focal point. Modern, simple, playful, and polished.';
