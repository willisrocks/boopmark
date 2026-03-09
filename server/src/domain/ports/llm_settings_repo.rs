use crate::domain::error::DomainError;
use crate::domain::llm_settings::LlmSettings;
use uuid::Uuid;

#[trait_variant::make(Send)]
pub trait LlmSettingsRepository: Send + Sync {
    async fn get(&self, user_id: Uuid) -> Result<Option<LlmSettings>, DomainError>;
    async fn upsert(
        &self,
        user_id: Uuid,
        enabled: bool,
        replace_anthropic_api_key_encrypted: Option<&[u8]>,
        clear_anthropic_api_key: bool,
        anthropic_model: &str,
    ) -> Result<LlmSettings, DomainError>;
}
