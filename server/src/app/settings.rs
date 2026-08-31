use crate::app::secrets::SecretBox;
use crate::domain::error::DomainError;
use crate::domain::llm_settings::{
    ANTHROPIC_MODEL_OPTIONS, DEFAULT_ANTHROPIC_MODEL, DEFAULT_IMAGE_GENERATION_MODEL,
    DEFAULT_OPENAI_MODEL, IMAGE_GENERATION_MODEL_OPTIONS, LlmSettings, OPENAI_MODEL_OPTIONS,
    TextProvider,
};
use crate::domain::ports::image_generator::{
    ImageAiSettingsProvider, ImageGenerationConfig, ImageGenerationContext,
};
use crate::domain::ports::llm_prompt_assistant::LlmPromptAssistant;
use crate::domain::ports::llm_settings_repo::{LlmSettingsRepository, LlmSettingsUpdate};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use uuid::Uuid;

pub struct SettingsService<R> {
    repo: Arc<R>,
    secret_box: Arc<SecretBox>,
}

pub struct SettingsView {
    pub enabled: bool,
    pub metadata_provider: String,
    pub has_anthropic_api_key: bool,
    pub anthropic_model: String,
    pub has_openai_api_key: bool,
    pub openai_model: String,
    pub image_generation_enabled: bool,
    pub image_generation_model: String,
}

#[derive(Debug, Default)]
pub struct SaveLlmSettingsInput {
    pub enabled: bool,
    pub metadata_provider: Option<String>,
    pub anthropic_api_key: Option<String>,
    pub clear_anthropic_api_key: bool,
    pub anthropic_model: Option<String>,
    pub openai_api_key: Option<String>,
    pub clear_openai_api_key: bool,
    pub openai_model: Option<String>,
    pub image_generation_enabled: bool,
    pub image_generation_model: Option<String>,
}

/// Decrypted credentials for the selected text provider. This is intentionally
/// returned only to the application layer that immediately invokes a provider;
/// settings views expose presence booleans, never key material.
#[derive(Debug, Clone)]
pub struct TextProviderCredentials {
    pub provider: TextProvider,
    pub api_key: String,
    pub model: String,
}

/// Decrypted image-generation settings consumed by the image adapter. OpenAI
/// is the only supported image provider today, so the provider is implicit and
/// the model is validated against the image allow-list.
#[derive(Debug, Clone)]
pub struct ImageGenerationSettings {
    pub api_key: String,
    pub model: String,
}

impl<R> SettingsService<R>
where
    R: LlmSettingsRepository + Send + Sync,
{
    pub fn new(repo: Arc<R>, secret_box: Arc<SecretBox>) -> Self {
        Self { repo, secret_box }
    }

    pub async fn load(&self, user_id: Uuid) -> Result<SettingsView, DomainError> {
        let settings = self.repo.get(user_id).await?;
        Ok(to_view(settings.as_ref()))
    }

    pub async fn get_decrypted_api_key(
        &self,
        user_id: Uuid,
    ) -> Result<Option<(String, String)>, DomainError> {
        Ok(self
            .get_text_provider_credentials(user_id)
            .await?
            .map(|credentials| (credentials.api_key, credentials.model)))
    }

    /// Load the selected metadata provider and decrypt its independent key.
    pub async fn get_text_provider_credentials(
        &self,
        user_id: Uuid,
    ) -> Result<Option<TextProviderCredentials>, DomainError> {
        let settings = self.repo.get(user_id).await?;
        match settings {
            Some(s) if s.enabled => {
                let provider = TextProvider::from_str(&s.metadata_provider).unwrap_or_default();
                let (encrypted, model) = match provider {
                    TextProvider::Anthropic => (&s.anthropic_api_key_encrypted, s.anthropic_model),
                    TextProvider::OpenAi => (&s.openai_api_key_encrypted, s.openai_model),
                };
                encrypted
                    .as_ref()
                    .map(|encrypted| {
                        self.secret_box
                            .decrypt(encrypted)
                            .map(|api_key| TextProviderCredentials {
                                provider,
                                api_key,
                                model: model.trim().to_string(),
                            })
                            .map_err(DomainError::Internal)
                    })
                    .transpose()
            }
            _ => Ok(None),
        }
    }

    /// Load image-generation settings, using the independently encrypted
    /// OpenAI provider key. Image generation is opt-in and disabled by default.
    pub async fn get_image_generation_settings(
        &self,
        user_id: Uuid,
    ) -> Result<Option<ImageGenerationSettings>, DomainError> {
        let settings = self.repo.get(user_id).await?;
        match settings {
            Some(s) if s.image_generation_enabled => {
                let Some(encrypted) = s.openai_api_key_encrypted.as_ref() else {
                    return Ok(None);
                };
                let api_key = self
                    .secret_box
                    .decrypt(encrypted)
                    .map_err(DomainError::Internal)?;
                Ok(Some(ImageGenerationSettings {
                    api_key,
                    model: normalize_image_model(Some(&s.image_generation_model)),
                }))
            }
            _ => Ok(None),
        }
    }

    pub async fn save(
        &self,
        user_id: Uuid,
        input: SaveLlmSettingsInput,
    ) -> Result<SettingsView, DomainError> {
        let existing = self.repo.get(user_id).await?;
        let provider = resolve_provider(existing.as_ref(), input.metadata_provider)?;
        let anthropic_model = resolve_model_for_provider(
            existing.as_ref(),
            TextProvider::Anthropic,
            input.anthropic_model,
        )?;
        let openai_model = resolve_model_for_provider(
            existing.as_ref(),
            TextProvider::OpenAi,
            input.openai_model,
        )?;
        let image_generation_model =
            resolve_image_model_for_save(existing.as_ref(), input.image_generation_model)?;

        let (replace_anthropic_key, clear_anthropic_key) = self.encrypt_key_change(
            resolve_api_key_change(input.anthropic_api_key, input.clear_anthropic_api_key),
        )?;
        let (replace_openai_key, clear_openai_key) = self.encrypt_key_change(
            resolve_api_key_change(input.openai_api_key, input.clear_openai_api_key),
        )?;

        let saved = self
            .repo
            .upsert(
                user_id,
                LlmSettingsUpdate {
                    enabled: input.enabled,
                    metadata_provider: provider.as_str().to_string(),
                    anthropic_model,
                    replace_anthropic_api_key_encrypted: replace_anthropic_key,
                    clear_anthropic_api_key: clear_anthropic_key,
                    openai_model,
                    replace_openai_api_key_encrypted: replace_openai_key,
                    clear_openai_api_key: clear_openai_key,
                    image_generation_enabled: input.image_generation_enabled,
                    image_generation_model,
                },
            )
            .await?;

        Ok(to_view(Some(&saved)))
    }

    fn encrypt_key_change(
        &self,
        change: ApiKeyChange,
    ) -> Result<(Option<Vec<u8>>, bool), DomainError> {
        match change {
            ApiKeyChange::KeepExisting => Ok((None, false)),
            ApiKeyChange::Clear => Ok((None, true)),
            ApiKeyChange::Replace(value) => Ok((
                Some(
                    self.secret_box
                        .encrypt(&value)
                        .map_err(DomainError::InvalidInput)?,
                ),
                false,
            )),
        }
    }
}

/// Bridge account settings and the configured text provider into the
/// provider-neutral image port. Image generation currently uses the OpenAI
/// key shared with the OpenAI text provider, while prompt assistance follows
/// the selected metadata provider.
pub struct ConfiguredImageAiSettingsProvider<R> {
    settings: Arc<SettingsService<R>>,
    prompt_assistant: Arc<dyn LlmPromptAssistant>,
}

impl<R> ConfiguredImageAiSettingsProvider<R> {
    pub fn new(
        settings: Arc<SettingsService<R>>,
        prompt_assistant: Arc<dyn LlmPromptAssistant>,
    ) -> Self {
        Self {
            settings,
            prompt_assistant,
        }
    }
}

impl<R> ImageAiSettingsProvider for ConfiguredImageAiSettingsProvider<R>
where
    R: LlmSettingsRepository + Send + Sync,
{
    fn image_config(
        &self,
        user_id: Uuid,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ImageGenerationConfig>, DomainError>> + Send + '_>>
    {
        let settings = self.settings.clone();
        Box::pin(async move {
            settings
                .get_image_generation_settings(user_id)
                .await
                .map(|settings| {
                    settings.map(|settings| ImageGenerationConfig {
                        api_key: settings.api_key,
                        model: settings.model,
                        art_style: None,
                    })
                })
        })
    }

    fn assist_image_prompt(
        &self,
        user_id: Uuid,
        context: ImageGenerationContext,
        instruction: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, DomainError>> + Send + '_>> {
        let settings = self.settings.clone();
        let prompt_assistant = self.prompt_assistant.clone();
        Box::pin(async move {
            let Some(credentials) = settings.get_text_provider_credentials(user_id).await? else {
                return Ok(None);
            };
            prompt_assistant
                .assist_image_prompt_with_provider(
                    credentials.provider,
                    &credentials.api_key,
                    &credentials.model,
                    context,
                    instruction,
                )
                .await
                .map(Some)
        })
    }
}

enum ApiKeyChange {
    KeepExisting,
    Clear,
    Replace(String),
}

fn normalize_model(model: Option<String>) -> String {
    match model {
        Some(value) if !value.trim().is_empty() => value.trim().to_string(),
        _ => DEFAULT_ANTHROPIC_MODEL.to_string(),
    }
}

fn normalize_openai_model(model: Option<String>) -> String {
    match model {
        Some(value) if !value.trim().is_empty() => value.trim().to_string(),
        _ => DEFAULT_OPENAI_MODEL.to_string(),
    }
}

fn normalize_image_model(model: Option<&str>) -> String {
    match model.map(str::trim) {
        Some(value) if !value.is_empty() => value.to_string(),
        _ => DEFAULT_IMAGE_GENERATION_MODEL.to_string(),
    }
}

fn resolve_provider(
    existing: Option<&LlmSettings>,
    submitted: Option<String>,
) -> Result<TextProvider, DomainError> {
    match submitted.as_deref().map(str::trim) {
        None | Some("") => Ok(existing
            .and_then(|settings| TextProvider::from_str(&settings.metadata_provider))
            .unwrap_or_default()),
        Some(value) => TextProvider::from_str(value).ok_or_else(|| {
            DomainError::InvalidInput("Unsupported metadata provider selection".into())
        }),
    }
}

fn resolve_model_for_provider(
    existing: Option<&LlmSettings>,
    provider: TextProvider,
    submitted: Option<String>,
) -> Result<String, DomainError> {
    let existing_model = existing.map(|settings| match provider {
        TextProvider::Anthropic => settings.anthropic_model.as_str(),
        TextProvider::OpenAi => settings.openai_model.as_str(),
    });
    let default_model = match provider {
        TextProvider::Anthropic => DEFAULT_ANTHROPIC_MODEL,
        TextProvider::OpenAi => DEFAULT_OPENAI_MODEL,
    };

    match submitted.as_deref().map(str::trim) {
        None | Some("") => {
            // Each provider's model is persisted independently. A settings
            // form submits both model selectors, so changing the active
            // provider must not reset the model saved for the other one.
            if let Some(model) = existing_model.filter(|model| !model.trim().is_empty()) {
                return Ok(model.trim().to_string());
            }
            Ok(default_model.to_string())
        }
        Some(value) => {
            let is_official = match provider {
                TextProvider::Anthropic => ANTHROPIC_MODEL_OPTIONS
                    .iter()
                    .any(|option| option.value == value),
                TextProvider::OpenAi => OPENAI_MODEL_OPTIONS
                    .iter()
                    .any(|option| option.value == value),
            };
            if is_official
                || existing_model
                    .map(|model| model.trim() == value)
                    .unwrap_or(false)
            {
                Ok(value.to_string())
            } else {
                Err(DomainError::InvalidInput(
                    match provider {
                        TextProvider::Anthropic => "Unsupported Anthropic model selection",
                        TextProvider::OpenAi => "Unsupported OpenAI model selection",
                    }
                    .into(),
                ))
            }
        }
    }
}

fn resolve_image_model_for_save(
    existing: Option<&LlmSettings>,
    submitted: Option<String>,
) -> Result<String, DomainError> {
    match submitted.as_deref().map(str::trim) {
        None | Some("") => Ok(existing
            .map(|settings| normalize_image_model(Some(&settings.image_generation_model)))
            .unwrap_or_else(|| DEFAULT_IMAGE_GENERATION_MODEL.to_string())),
        Some(value)
            if IMAGE_GENERATION_MODEL_OPTIONS
                .iter()
                .any(|option| option.value == value) =>
        {
            Ok(value.to_string())
        }
        Some(value)
            if existing
                .map(|settings| settings.image_generation_model.trim() == value)
                .unwrap_or(false) =>
        {
            Ok(value.to_string())
        }
        Some(_) => Err(DomainError::InvalidInput(
            "Unsupported image generation model selection".into(),
        )),
    }
}

fn resolve_api_key_change(api_key: Option<String>, clear: bool) -> ApiKeyChange {
    if clear {
        return ApiKeyChange::Clear;
    }

    match api_key {
        Some(value) if !value.trim().is_empty() => ApiKeyChange::Replace(value.trim().to_string()),
        _ => ApiKeyChange::KeepExisting,
    }
}

fn to_view(settings: Option<&LlmSettings>) -> SettingsView {
    match settings {
        Some(settings) => {
            let provider = TextProvider::from_str(&settings.metadata_provider).unwrap_or_default();
            SettingsView {
                enabled: settings.enabled,
                metadata_provider: provider.as_str().to_string(),
                has_anthropic_api_key: settings.anthropic_api_key_encrypted.is_some(),
                anthropic_model: normalize_model(Some(settings.anthropic_model.clone())),
                has_openai_api_key: settings.openai_api_key_encrypted.is_some(),
                openai_model: normalize_openai_model(Some(settings.openai_model.clone())),
                image_generation_enabled: settings.image_generation_enabled,
                image_generation_model: normalize_image_model(Some(
                    &settings.image_generation_model,
                )),
            }
        }
        None => SettingsView {
            enabled: false,
            metadata_provider: TextProvider::default().as_str().to_string(),
            has_anthropic_api_key: false,
            anthropic_model: DEFAULT_ANTHROPIC_MODEL.to_string(),
            has_openai_api_key: false,
            openai_model: DEFAULT_OPENAI_MODEL.to_string(),
            image_generation_enabled: false,
            image_generation_model: DEFAULT_IMAGE_GENERATION_MODEL.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error::DomainError;
    use crate::domain::llm_settings::LlmSettings;
    use chrono::Utc;
    use std::sync::Mutex;

    struct FakeLlmSettingsRepository {
        stored: Mutex<Option<LlmSettings>>,
        last_upsert: Mutex<Option<LastUpsert>>,
    }

    struct PromptAssistantStub {
        marker: &'static str,
    }

    impl LlmPromptAssistant for PromptAssistantStub {
        fn assist_image_prompt(
            &self,
            _api_key: &str,
            _model: &str,
            _context: ImageGenerationContext,
            _instruction: Option<String>,
        ) -> Pin<Box<dyn Future<Output = Result<String, DomainError>> + Send + '_>> {
            let marker = self.marker;
            Box::pin(async move { Ok(marker.to_string()) })
        }
    }

    #[allow(dead_code)]
    struct LastUpsert {
        enabled: bool,
        metadata_provider: String,
        replace_anthropic_api_key_encrypted: Option<Vec<u8>>,
        clear_anthropic_api_key: bool,
        anthropic_model: String,
        replace_openai_api_key_encrypted: Option<Vec<u8>>,
        clear_openai_api_key: bool,
        openai_model: String,
        image_generation_enabled: bool,
        image_generation_model: String,
    }

    impl FakeLlmSettingsRepository {
        fn new() -> Self {
            Self {
                stored: Mutex::new(None),
                last_upsert: Mutex::new(None),
            }
        }
    }

    impl LlmSettingsRepository for FakeLlmSettingsRepository {
        async fn get(&self, _user_id: Uuid) -> Result<Option<LlmSettings>, DomainError> {
            Ok(self.stored.lock().expect("stored lock").clone())
        }

        async fn upsert(
            &self,
            user_id: Uuid,
            update: LlmSettingsUpdate,
        ) -> Result<LlmSettings, DomainError> {
            let existing = self.stored.lock().expect("stored lock").clone();
            let anthropic_encrypted = if update.clear_anthropic_api_key {
                None
            } else {
                update
                    .replace_anthropic_api_key_encrypted
                    .clone()
                    .or_else(|| {
                        existing
                            .as_ref()
                            .and_then(|settings| settings.anthropic_api_key_encrypted.clone())
                    })
            };
            let openai_encrypted = if update.clear_openai_api_key {
                None
            } else {
                update.replace_openai_api_key_encrypted.clone().or_else(|| {
                    existing
                        .as_ref()
                        .and_then(|settings| settings.openai_api_key_encrypted.clone())
                })
            };

            *self.last_upsert.lock().expect("last_upsert lock") = Some(LastUpsert {
                enabled: update.enabled,
                metadata_provider: update.metadata_provider.clone(),
                replace_anthropic_api_key_encrypted: update
                    .replace_anthropic_api_key_encrypted
                    .clone(),
                clear_anthropic_api_key: update.clear_anthropic_api_key,
                anthropic_model: update.anthropic_model.clone(),
                replace_openai_api_key_encrypted: update.replace_openai_api_key_encrypted.clone(),
                clear_openai_api_key: update.clear_openai_api_key,
                openai_model: update.openai_model.clone(),
                image_generation_enabled: update.image_generation_enabled,
                image_generation_model: update.image_generation_model.clone(),
            });

            let saved = LlmSettings {
                user_id,
                enabled: update.enabled,
                metadata_provider: update.metadata_provider,
                anthropic_api_key_encrypted: anthropic_encrypted,
                anthropic_model: update.anthropic_model,
                openai_api_key_encrypted: openai_encrypted,
                openai_model: update.openai_model,
                image_generation_enabled: update.image_generation_enabled,
                image_generation_model: update.image_generation_model,
                created_at: existing
                    .as_ref()
                    .map(|settings| settings.created_at)
                    .unwrap_or_else(Utc::now),
                updated_at: Utc::now(),
            };
            *self.stored.lock().expect("stored lock") = Some(saved.clone());
            Ok(saved)
        }
    }

    #[test]
    fn normalize_model_defaults_to_latest_full_haiku_id() {
        assert_eq!(normalize_model(None), "claude-haiku-4-5-20251001");
        assert_eq!(
            normalize_model(Some("   ".into())),
            "claude-haiku-4-5-20251001"
        );
    }

    #[test]
    fn normalize_model_accepts_the_current_official_model_ids() {
        assert_eq!(
            normalize_model(Some("claude-opus-4-6".into())),
            "claude-opus-4-6"
        );
        assert_eq!(
            normalize_model(Some("claude-sonnet-4-6".into())),
            "claude-sonnet-4-6"
        );
        assert_eq!(
            normalize_model(Some("claude-haiku-4-5-20251001".into())),
            "claude-haiku-4-5-20251001"
        );
    }

    #[test]
    fn normalize_model_preserves_a_preexisting_custom_value() {
        assert_eq!(
            normalize_model(Some("claude-3-7-sonnet-latest".into())),
            "claude-3-7-sonnet-latest"
        );
    }

    #[test]
    fn blank_key_keeps_existing_key() {
        assert!(matches!(
            resolve_api_key_change(Some("   ".into()), false),
            ApiKeyChange::KeepExisting
        ));
    }

    #[test]
    fn clear_checkbox_removes_saved_key() {
        assert!(matches!(
            resolve_api_key_change(None, true),
            ApiKeyChange::Clear
        ));
    }

    #[test]
    fn non_blank_key_replaces_saved_key() {
        assert!(matches!(
            resolve_api_key_change(Some("sk-ant-new".into()), false),
            ApiKeyChange::Replace(_)
        ));
    }

    #[tokio::test]
    async fn save_encrypts_replacement_key_and_returns_presence_only_view() {
        let repo = Arc::new(FakeLlmSettingsRepository::new());
        let secret_box = Arc::new(SecretBox::new(
            "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
        ));
        let service = SettingsService::new(repo.clone(), secret_box.clone());
        let user_id = Uuid::new_v4();

        let view = service
            .save(
                user_id,
                SaveLlmSettingsInput {
                    enabled: true,
                    anthropic_api_key: Some("sk-ant-test".into()),
                    clear_anthropic_api_key: false,
                    anthropic_model: Some("claude-haiku-4-5-20251001".into()),
                    ..Default::default()
                },
            )
            .await
            .expect("save");

        let last_upsert = repo
            .last_upsert
            .lock()
            .expect("last_upsert lock")
            .take()
            .expect("last_upsert");
        let encrypted = last_upsert
            .replace_anthropic_api_key_encrypted
            .expect("encrypted key");

        assert!(last_upsert.enabled);
        assert!(!last_upsert.clear_anthropic_api_key);
        assert_eq!(last_upsert.anthropic_model, "claude-haiku-4-5-20251001");
        assert_ne!(encrypted, b"sk-ant-test");
        assert_eq!(
            secret_box.decrypt(&encrypted).expect("decrypt"),
            "sk-ant-test"
        );
        assert!(view.enabled);
        assert!(view.has_anthropic_api_key);
        assert_eq!(view.anthropic_model, "claude-haiku-4-5-20251001");
    }

    #[tokio::test]
    async fn load_preserves_a_stored_legacy_model_value() {
        let repo = Arc::new(FakeLlmSettingsRepository::new());
        let secret_box = Arc::new(SecretBox::new(
            "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
        ));
        let service = SettingsService::new(repo.clone(), secret_box);
        let user_id = Uuid::new_v4();

        repo.stored
            .lock()
            .expect("stored lock")
            .replace(LlmSettings {
                user_id,
                enabled: true,
                metadata_provider: "anthropic".into(),
                anthropic_api_key_encrypted: Some(vec![1, 2, 3]),
                anthropic_model: "claude-3-7-sonnet-latest".into(),
                openai_api_key_encrypted: None,
                openai_model: DEFAULT_OPENAI_MODEL.into(),
                image_generation_enabled: false,
                image_generation_model: DEFAULT_IMAGE_GENERATION_MODEL.into(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
            });

        let view = service.load(user_id).await.expect("load");

        assert!(view.enabled);
        assert!(view.has_anthropic_api_key);
        assert_eq!(view.anthropic_model, "claude-3-7-sonnet-latest");
    }

    #[tokio::test]
    async fn save_preserves_a_re_submitted_legacy_model_value() {
        let repo = Arc::new(FakeLlmSettingsRepository::new());
        let secret_box = Arc::new(SecretBox::new(
            "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
        ));
        let service = SettingsService::new(repo.clone(), secret_box);
        let user_id = Uuid::new_v4();

        repo.stored
            .lock()
            .expect("stored lock")
            .replace(LlmSettings {
                user_id,
                enabled: true,
                metadata_provider: "anthropic".into(),
                anthropic_api_key_encrypted: Some(vec![1, 2, 3]),
                anthropic_model: "claude-3-7-sonnet-latest".into(),
                openai_api_key_encrypted: None,
                openai_model: DEFAULT_OPENAI_MODEL.into(),
                image_generation_enabled: false,
                image_generation_model: DEFAULT_IMAGE_GENERATION_MODEL.into(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
            });

        let view = service
            .save(
                user_id,
                SaveLlmSettingsInput {
                    enabled: true,
                    anthropic_api_key: None,
                    clear_anthropic_api_key: false,
                    anthropic_model: Some("claude-3-7-sonnet-latest".into()),
                    ..Default::default()
                },
            )
            .await
            .expect("save");

        assert_eq!(view.anthropic_model, "claude-3-7-sonnet-latest");
        let last_upsert = repo
            .last_upsert
            .lock()
            .expect("last_upsert lock")
            .take()
            .expect("last_upsert");
        assert_eq!(last_upsert.anthropic_model, "claude-3-7-sonnet-latest");
    }

    #[tokio::test]
    async fn save_preserves_existing_model_when_the_field_is_omitted() {
        let repo = Arc::new(FakeLlmSettingsRepository::new());
        let secret_box = Arc::new(SecretBox::new(
            "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
        ));
        let service = SettingsService::new(repo.clone(), secret_box);
        let user_id = Uuid::new_v4();

        repo.stored
            .lock()
            .expect("stored lock")
            .replace(LlmSettings {
                user_id,
                enabled: true,
                metadata_provider: "anthropic".into(),
                anthropic_api_key_encrypted: Some(vec![1, 2, 3]),
                anthropic_model: "claude-3-7-sonnet-latest".into(),
                openai_api_key_encrypted: None,
                openai_model: DEFAULT_OPENAI_MODEL.into(),
                image_generation_enabled: false,
                image_generation_model: DEFAULT_IMAGE_GENERATION_MODEL.into(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
            });

        let view = service
            .save(
                user_id,
                SaveLlmSettingsInput {
                    enabled: false,
                    anthropic_api_key: None,
                    clear_anthropic_api_key: false,
                    anthropic_model: None,
                    ..Default::default()
                },
            )
            .await
            .expect("save");

        assert_eq!(view.anthropic_model, "claude-3-7-sonnet-latest");
        let last_upsert = repo
            .last_upsert
            .lock()
            .expect("last_upsert lock")
            .take()
            .expect("last_upsert");
        assert_eq!(last_upsert.anthropic_model, "claude-3-7-sonnet-latest");
    }

    #[tokio::test]
    async fn save_preserves_existing_model_when_the_field_is_blank() {
        let repo = Arc::new(FakeLlmSettingsRepository::new());
        let secret_box = Arc::new(SecretBox::new(
            "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
        ));
        let service = SettingsService::new(repo.clone(), secret_box);
        let user_id = Uuid::new_v4();

        repo.stored
            .lock()
            .expect("stored lock")
            .replace(LlmSettings {
                user_id,
                enabled: true,
                metadata_provider: "anthropic".into(),
                anthropic_api_key_encrypted: Some(vec![1, 2, 3]),
                anthropic_model: "claude-3-7-sonnet-latest".into(),
                openai_api_key_encrypted: None,
                openai_model: DEFAULT_OPENAI_MODEL.into(),
                image_generation_enabled: false,
                image_generation_model: DEFAULT_IMAGE_GENERATION_MODEL.into(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
            });

        let view = service
            .save(
                user_id,
                SaveLlmSettingsInput {
                    enabled: false,
                    anthropic_api_key: None,
                    clear_anthropic_api_key: false,
                    anthropic_model: Some("   ".into()),
                    ..Default::default()
                },
            )
            .await
            .expect("save");

        assert_eq!(view.anthropic_model, "claude-3-7-sonnet-latest");
        let last_upsert = repo
            .last_upsert
            .lock()
            .expect("last_upsert lock")
            .take()
            .expect("last_upsert");
        assert_eq!(last_upsert.anthropic_model, "claude-3-7-sonnet-latest");
    }

    #[tokio::test]
    async fn get_decrypted_api_key_returns_key_and_model_when_enabled() {
        let repo = Arc::new(FakeLlmSettingsRepository::new());
        let secret_box = Arc::new(SecretBox::new(
            "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
        ));
        let service = SettingsService::new(repo.clone(), secret_box.clone());
        let user_id = Uuid::new_v4();

        service
            .save(
                user_id,
                SaveLlmSettingsInput {
                    enabled: true,
                    anthropic_api_key: Some("sk-ant-test-key".into()),
                    clear_anthropic_api_key: false,
                    anthropic_model: Some("claude-haiku-4-5-20251001".into()),
                    ..Default::default()
                },
            )
            .await
            .expect("save");

        let result = service
            .get_decrypted_api_key(user_id)
            .await
            .expect("get key");
        let (key, model) = result.expect("should have key");
        assert_eq!(key, "sk-ant-test-key");
        assert_eq!(model, "claude-haiku-4-5-20251001");
    }

    #[tokio::test]
    async fn get_decrypted_api_key_returns_none_when_disabled() {
        let repo = Arc::new(FakeLlmSettingsRepository::new());
        let secret_box = Arc::new(SecretBox::new(
            "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
        ));
        let service = SettingsService::new(repo.clone(), secret_box.clone());
        let user_id = Uuid::new_v4();

        service
            .save(
                user_id,
                SaveLlmSettingsInput {
                    enabled: false,
                    anthropic_api_key: Some("sk-ant-test-key".into()),
                    clear_anthropic_api_key: false,
                    anthropic_model: Some("claude-haiku-4-5-20251001".into()),
                    ..Default::default()
                },
            )
            .await
            .expect("save");

        let result = service
            .get_decrypted_api_key(user_id)
            .await
            .expect("get key");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn get_decrypted_api_key_returns_none_when_no_settings() {
        let repo = Arc::new(FakeLlmSettingsRepository::new());
        let secret_box = Arc::new(SecretBox::new(
            "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
        ));
        let service = SettingsService::new(repo.clone(), secret_box.clone());
        let user_id = Uuid::new_v4();

        let result = service
            .get_decrypted_api_key(user_id)
            .await
            .expect("get key");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn save_rejects_new_unsupported_model_values() {
        let repo = Arc::new(FakeLlmSettingsRepository::new());
        let secret_box = Arc::new(SecretBox::new(
            "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
        ));
        let service = SettingsService::new(repo, secret_box);

        let error = service
            .save(
                Uuid::new_v4(),
                SaveLlmSettingsInput {
                    enabled: true,
                    anthropic_api_key: None,
                    clear_anthropic_api_key: false,
                    anthropic_model: Some("claude-3-7-sonnet-latest".into()),
                    ..Default::default()
                },
            )
            .await
            .err()
            .expect("invalid model should fail");

        assert!(matches!(error, DomainError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn save_supports_openai_metadata_and_image_settings_with_encrypted_key() {
        let repo = Arc::new(FakeLlmSettingsRepository::new());
        let secret_box = Arc::new(SecretBox::new(
            "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
        ));
        let service = SettingsService::new(repo.clone(), secret_box.clone());
        let user_id = Uuid::new_v4();

        let view = service
            .save(
                user_id,
                SaveLlmSettingsInput {
                    enabled: true,
                    metadata_provider: Some("openai".into()),
                    openai_api_key: Some("sk-openai-test".into()),
                    openai_model: Some("gpt-5.6-terra".into()),
                    image_generation_enabled: true,
                    image_generation_model: Some("gpt-image-2".into()),
                    ..Default::default()
                },
            )
            .await
            .expect("save");

        assert!(view.enabled);
        assert_eq!(view.metadata_provider, "openai");
        assert!(view.has_openai_api_key);
        assert_eq!(view.openai_model, "gpt-5.6-terra");
        assert!(view.image_generation_enabled);
        assert_eq!(view.image_generation_model, "gpt-image-2");

        let credentials = service
            .get_text_provider_credentials(user_id)
            .await
            .expect("credentials")
            .expect("configured credentials");
        assert_eq!(credentials.provider, TextProvider::OpenAi);
        assert_eq!(credentials.api_key, "sk-openai-test");
        assert_eq!(credentials.model, "gpt-5.6-terra");

        let image = service
            .get_image_generation_settings(user_id)
            .await
            .expect("image settings")
            .expect("configured image settings");
        assert_eq!(image.api_key, "sk-openai-test");
        assert_eq!(image.model, "gpt-image-2");

        let last_upsert = repo
            .last_upsert
            .lock()
            .expect("last_upsert lock")
            .take()
            .expect("last_upsert");
        let encrypted = last_upsert
            .replace_openai_api_key_encrypted
            .expect("encrypted OpenAI key");
        assert_ne!(encrypted, b"sk-openai-test");
        assert_eq!(
            secret_box.decrypt(&encrypted).expect("decrypt"),
            "sk-openai-test"
        );
        assert_eq!(last_upsert.metadata_provider, "openai");
        assert_eq!(last_upsert.openai_model, "gpt-5.6-terra");
        assert!(last_upsert.image_generation_enabled);
    }

    #[tokio::test]
    async fn switching_provider_keeps_independent_anthropic_key() {
        let repo = Arc::new(FakeLlmSettingsRepository::new());
        let secret_box = Arc::new(SecretBox::new(
            "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
        ));
        let service = SettingsService::new(repo.clone(), secret_box.clone());
        let user_id = Uuid::new_v4();

        service
            .save(
                user_id,
                SaveLlmSettingsInput {
                    enabled: true,
                    anthropic_api_key: Some("sk-ant-independent".into()),
                    anthropic_model: Some(DEFAULT_ANTHROPIC_MODEL.into()),
                    ..Default::default()
                },
            )
            .await
            .expect("Anthropic save");
        service
            .save(
                user_id,
                SaveLlmSettingsInput {
                    enabled: true,
                    metadata_provider: Some("openai".into()),
                    openai_api_key: Some("sk-openai-independent".into()),
                    openai_model: Some(DEFAULT_OPENAI_MODEL.into()),
                    ..Default::default()
                },
            )
            .await
            .expect("OpenAI save");

        let stored = repo
            .stored
            .lock()
            .expect("stored lock")
            .clone()
            .expect("settings");
        let anthropic_key = stored
            .anthropic_api_key_encrypted
            .as_ref()
            .expect("Anthropic key retained");
        let openai_key = stored
            .openai_api_key_encrypted
            .as_ref()
            .expect("OpenAI key retained");
        assert_eq!(
            secret_box
                .decrypt(anthropic_key)
                .expect("decrypt Anthropic"),
            "sk-ant-independent"
        );
        assert_eq!(
            secret_box.decrypt(openai_key).expect("decrypt OpenAI"),
            "sk-openai-independent"
        );
    }

    #[tokio::test]
    async fn configured_image_settings_bridge_uses_selected_text_provider() {
        let repo = Arc::new(FakeLlmSettingsRepository::new());
        let secret_box = Arc::new(SecretBox::new(
            "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
        ));
        let settings = Arc::new(SettingsService::new(repo, secret_box));
        let user_id = Uuid::new_v4();
        settings
            .save(
                user_id,
                SaveLlmSettingsInput {
                    enabled: true,
                    metadata_provider: Some("openai".into()),
                    openai_api_key: Some("sk-openai-for-prompt".into()),
                    openai_model: Some(DEFAULT_OPENAI_MODEL.into()),
                    ..Default::default()
                },
            )
            .await
            .expect("save");

        let prompt_router = crate::adapters::llm_router::LlmPromptAssistantRouter::new(
            Arc::new(PromptAssistantStub {
                marker: "anthropic",
            }),
            Arc::new(PromptAssistantStub { marker: "openai" }),
        );
        let image_settings =
            ConfiguredImageAiSettingsProvider::new(settings, Arc::new(prompt_router));
        let prompt = image_settings
            .assist_image_prompt(
                user_id,
                ImageGenerationContext {
                    url: "https://example.com".into(),
                    title: Some("Example".into()),
                    description: None,
                },
                Some("use a warm palette".into()),
            )
            .await
            .expect("prompt assistance")
            .expect("selected provider prompt");
        assert_eq!(prompt, "openai");
    }
}
