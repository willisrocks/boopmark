use super::PostgresPool;
use crate::domain::error::DomainError;
use crate::domain::llm_settings::LlmSettings;
use crate::domain::ports::llm_settings_repo::LlmSettingsRepository;
use uuid::Uuid;

impl LlmSettingsRepository for PostgresPool {
    async fn get(&self, user_id: Uuid) -> Result<Option<LlmSettings>, DomainError> {
        sqlx::query_as::<_, LlmSettings>(
            "SELECT user_id, enabled, anthropic_api_key_encrypted, anthropic_model, created_at, updated_at
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
        enabled: bool,
        replace_anthropic_api_key_encrypted: Option<&[u8]>,
        clear_anthropic_api_key: bool,
        anthropic_model: &str,
    ) -> Result<LlmSettings, DomainError> {
        sqlx::query_as::<_, LlmSettings>(
            "INSERT INTO user_llm_settings (user_id, enabled, anthropic_api_key_encrypted, anthropic_model)
             VALUES ($1, $2, $3, $5)
             ON CONFLICT (user_id) DO UPDATE
             SET enabled = EXCLUDED.enabled,
                 anthropic_api_key_encrypted = CASE
                     WHEN $4 THEN NULL
                     WHEN $3 IS NOT NULL THEN $3
                     ELSE user_llm_settings.anthropic_api_key_encrypted
                 END,
                 anthropic_model = EXCLUDED.anthropic_model,
                 updated_at = now()
             RETURNING user_id, enabled, anthropic_api_key_encrypted, anthropic_model, created_at, updated_at",
        )
        .bind(user_id)
        .bind(enabled)
        .bind(replace_anthropic_api_key_encrypted)
        .bind(clear_anthropic_api_key)
        .bind(anthropic_model)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))
    }
}
