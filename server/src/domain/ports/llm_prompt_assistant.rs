use crate::domain::error::DomainError;
use crate::domain::llm_settings::TextProvider;
use crate::domain::ports::image_generator::ImageGenerationContext;
use std::future::Future;
use std::pin::Pin;

/// Maximum size accepted for a provider-written image prompt. Keeping this
/// limit at the port boundary prevents an unexpectedly verbose model response
/// from becoming an unbounded image request.
pub const MAX_ASSISTED_IMAGE_PROMPT_CHARS: usize = 3_000;

/// Provider-neutral text capability used to turn bookmark context into a
/// concise image-generation direction. The image renderer remains a separate
/// capability: this port only writes the prompt.
pub trait LlmPromptAssistant: Send + Sync {
    fn assist_image_prompt(
        &self,
        api_key: &str,
        model: &str,
        context: ImageGenerationContext,
        instruction: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Result<String, DomainError>> + Send + '_>>;

    /// Provider-aware entry point used by the configured image settings
    /// bridge. Concrete adapters can use the compatibility default; routers
    /// override it to dispatch explicitly instead of guessing from model IDs.
    fn assist_image_prompt_with_provider(
        &self,
        _provider: TextProvider,
        api_key: &str,
        model: &str,
        context: ImageGenerationContext,
        instruction: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Result<String, DomainError>> + Send + '_>> {
        self.assist_image_prompt(api_key, model, context, instruction)
    }
}

/// Normalize and bound a provider-written prompt before handing it to an
/// image renderer. Newlines are useful in prompts, but other control
/// characters are rejected as malformed output.
pub fn validate_assisted_image_prompt(value: String) -> Result<String, DomainError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(DomainError::Internal(
            "AI image prompt assistant returned an empty prompt".to_string(),
        ));
    }
    if value.chars().count() > MAX_ASSISTED_IMAGE_PROMPT_CHARS {
        return Err(DomainError::Internal(
            "AI image prompt assistant returned an oversized prompt".to_string(),
        ));
    }
    if value
        .chars()
        .any(|character| character.is_control() && character != '\n' && character != '\t')
    {
        return Err(DomainError::Internal(
            "AI image prompt assistant returned malformed text".to_string(),
        ));
    }
    Ok(value.to_string())
}
