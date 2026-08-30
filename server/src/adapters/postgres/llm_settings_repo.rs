use super::PostgresPool;
use crate::domain::error::DomainError;
use crate::domain::llm_settings::LlmSettings;
use crate::domain::ports::llm_settings_repo::{LlmSettingsRepository, LlmSettingsUpdate};
use uuid::Uuid;

impl LlmSettingsRepository for PostgresPool {
    async fn get(&self, user_id: Uuid) -> Result<Option<LlmSettings>, DomainError> {
        sqlx::query_as::<_, LlmSettings>(
            "SELECT user_id, enabled, metadata_provider, anthropic_api_key_encrypted,
                    anthropic_model, openai_api_key_encrypted, openai_model,
                    image_generation_enabled, image_generation_model,
                    created_at, updated_at
             FROM user_llm_settings
             WHERE user_id = $1",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))
    }

    async fn upsert(
        &self,
        user_id: Uuid,
        update: LlmSettingsUpdate,
    ) -> Result<LlmSettings, DomainError> {
        sqlx::query_as::<_, LlmSettings>(
            "INSERT INTO user_llm_settings (
                 user_id, enabled, metadata_provider,
                 anthropic_api_key_encrypted, anthropic_model,
                 openai_api_key_encrypted, openai_model,
                 image_generation_enabled, image_generation_model
             )
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (user_id) DO UPDATE
             SET enabled = EXCLUDED.enabled,
                 metadata_provider = EXCLUDED.metadata_provider,
                 anthropic_api_key_encrypted = CASE
                     WHEN $10 THEN NULL
                     WHEN $4 IS NOT NULL THEN $4
                     ELSE user_llm_settings.anthropic_api_key_encrypted
                 END,
                 anthropic_model = EXCLUDED.anthropic_model,
                 openai_api_key_encrypted = CASE
                     WHEN $11 THEN NULL
                     WHEN $6 IS NOT NULL THEN $6
                     ELSE user_llm_settings.openai_api_key_encrypted
                 END,
                 openai_model = EXCLUDED.openai_model,
                 image_generation_enabled = EXCLUDED.image_generation_enabled,
                 image_generation_model = EXCLUDED.image_generation_model,
                 updated_at = now()
             RETURNING user_id, enabled, metadata_provider,
                       anthropic_api_key_encrypted, anthropic_model,
                       openai_api_key_encrypted, openai_model,
                       image_generation_enabled, image_generation_model,
                       created_at, updated_at",
        )
        .bind(user_id)
        .bind(update.enabled)
        .bind(update.metadata_provider)
        .bind(update.replace_anthropic_api_key_encrypted)
        .bind(update.anthropic_model)
        .bind(update.replace_openai_api_key_encrypted)
        .bind(update.openai_model)
        .bind(update.image_generation_enabled)
        .bind(update.image_generation_model)
        .bind(update.clear_anthropic_api_key)
        .bind(update.clear_openai_api_key)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))
    }
}
