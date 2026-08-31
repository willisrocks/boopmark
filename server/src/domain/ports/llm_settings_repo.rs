use crate::domain::error::DomainError;
use crate::domain::llm_settings::LlmSettings;
use uuid::Uuid;

#[trait_variant::make(Send)]
pub trait LlmSettingsRepository: Send + Sync {
    async fn get(&self, user_id: Uuid) -> Result<Option<LlmSettings>, DomainError>;
    // Settings are persisted atomically so the independently clearable encrypted keys
    // cannot drift from their enable flags and model selections.
    #[allow(clippy::too_many_arguments)]
    async fn upsert(
        &self,
        user_id: Uuid,
        enabled: bool,
        replace_anthropic_api_key_encrypted: Option<&[u8]>,
        clear_anthropic_api_key: bool,
        anthropic_model: &str,
        image_generation_enabled: bool,
        replace_gemini_api_key_encrypted: Option<&[u8]>,
        clear_gemini_api_key: bool,
        image_generation_model: &str,
        image_generation_art_style: &str,
    ) -> Result<LlmSettings, DomainError>;
}
