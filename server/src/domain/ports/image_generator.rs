use crate::domain::error::DomainError;
use std::future::Future;
use std::pin::Pin;
use uuid::Uuid;

/// The small, provider-neutral context used to turn a bookmark into an image
/// prompt.  Values originate in a bookmark and are therefore always bounded
/// and treated as source material by adapters.
#[derive(Debug, Clone)]
pub struct ImageGenerationContext {
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
}

/// Decrypted image settings.  Keeping this type at the port boundary means
/// image adapters never need to know how settings are stored or encrypted.
#[derive(Debug, Clone)]
pub struct ImageGenerationConfig {
    pub api_key: String,
    pub model: String,
    pub art_style: Option<String>,
}

#[derive(Debug)]
pub struct GeneratedImage {
    pub bytes: Vec<u8>,
    pub mime_type: String,
}

/// Narrow capability used by image adapters. Implementations normally wrap
/// the application settings service and may route prompt assistance through
/// either the Anthropic or OpenAI text provider selected by the user.
pub trait ImageAiSettingsProvider: Send + Sync {
    fn image_config(
        &self,
        user_id: Uuid,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ImageGenerationConfig>, DomainError>> + Send + '_>>;

    fn assist_image_prompt(
        &self,
        user_id: Uuid,
        context: ImageGenerationContext,
        instruction: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, DomainError>> + Send + '_>>;
}

/// Provider-neutral image generation/editing boundary. OpenAI's GPT Image 2
/// is the current pixel provider; the optional text assistant is deliberately
/// kept behind `ImageAiSettingsProvider` so account settings remain swappable.
pub trait ImageGenerator: Send + Sync {
    fn is_configured(
        &self,
        user_id: Uuid,
    ) -> Pin<Box<dyn Future<Output = Result<bool, DomainError>> + Send + '_>>;

    fn generate(
        &self,
        user_id: Uuid,
        context: ImageGenerationContext,
        instruction: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Result<GeneratedImage, DomainError>> + Send + '_>>;

    /// Edit an existing image when the caller supplies an instruction. A
    /// generator may fall back to generation if no source image is available.
    fn edit(
        &self,
        user_id: Uuid,
        source: Vec<u8>,
        context: ImageGenerationContext,
        instruction: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Result<GeneratedImage, DomainError>> + Send + '_>>;
}
