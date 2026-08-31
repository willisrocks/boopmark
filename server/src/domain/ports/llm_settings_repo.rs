use crate::domain::error::DomainError;
use crate::domain::llm_settings::LlmSettings;
use uuid::Uuid;

/// Encrypted settings update. The service owns key encryption and passes the
/// repository only ciphertext; `None` means keep the existing ciphertext and
/// the corresponding clear flag explicitly removes it.
#[derive(Debug, Default)]
pub struct LlmSettingsUpdate {
    pub enabled: bool,
    pub metadata_provider: String,
    pub anthropic_model: String,
    pub replace_anthropic_api_key_encrypted: Option<Vec<u8>>,
    pub clear_anthropic_api_key: bool,
    pub openai_model: String,
    pub replace_openai_api_key_encrypted: Option<Vec<u8>>,
    pub clear_openai_api_key: bool,
    pub image_generation_enabled: bool,
    pub image_generation_model: String,
}

#[trait_variant::make(Send)]
pub trait LlmSettingsRepository: Send + Sync {
    async fn get(&self, user_id: Uuid) -> Result<Option<LlmSettings>, DomainError>;
    async fn upsert(
        &self,
        user_id: Uuid,
        update: LlmSettingsUpdate,
    ) -> Result<LlmSettings, DomainError>;
}
