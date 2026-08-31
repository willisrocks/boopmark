use crate::app::secrets::SecretBox;
use crate::domain::error::DomainError;
use crate::domain::llm_settings::{
    ANTHROPIC_MODEL_OPTIONS, DEFAULT_ANTHROPIC_MODEL, DEFAULT_IMAGE_ART_STYLE,
    DEFAULT_IMAGE_GENERATION_MODEL, LlmSettings,
};
use crate::domain::ports::llm_settings_repo::LlmSettingsRepository;
use std::sync::Arc;
use uuid::Uuid;

pub struct SettingsService<R> {
    repo: Arc<R>,
    secret_box: Arc<SecretBox>,
}

pub struct SettingsView {
    pub enabled: bool,
    pub has_anthropic_api_key: bool,
    pub anthropic_model: String,
    pub image_generation_enabled: bool,
    pub has_gemini_api_key: bool,
    pub image_generation_model: String,
    pub image_generation_art_style: String,
}

#[derive(Clone, Debug)]
pub struct ImageGenerationSettings {
    pub api_key: String,
    pub model: String,
    pub art_style: String,
}

pub struct SaveLlmSettingsInput {
    pub enabled: bool,
    pub anthropic_api_key: Option<String>,
    pub clear_anthropic_api_key: bool,
    pub anthropic_model: Option<String>,
    pub image_generation_enabled: bool,
    pub gemini_api_key: Option<String>,
    pub clear_gemini_api_key: bool,
    pub image_generation_model: Option<String>,
    pub image_generation_art_style: Option<String>,
}

impl Default for SaveLlmSettingsInput {
    fn default() -> Self {
        Self {
            enabled: false,
            anthropic_api_key: None,
            clear_anthropic_api_key: false,
            anthropic_model: None,
            image_generation_enabled: false,
            gemini_api_key: None,
            clear_gemini_api_key: false,
            image_generation_model: None,
            image_generation_art_style: None,
        }
    }
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
        let settings = self.repo.get(user_id).await?;
        match settings {
            Some(s) if s.enabled => {
                if let Some(encrypted) = &s.anthropic_api_key_encrypted {
                    let decrypted = self
                        .secret_box
                        .decrypt(encrypted)
                        .map_err(DomainError::Internal)?;
                    Ok(Some((decrypted, s.anthropic_model)))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    pub async fn get_image_generation_settings(
        &self,
        user_id: Uuid,
    ) -> Result<Option<ImageGenerationSettings>, DomainError> {
        let settings = self.repo.get(user_id).await?;
        match settings {
            Some(s) if s.image_generation_enabled => {
                let Some(encrypted) = &s.gemini_api_key_encrypted else {
                    return Ok(None);
                };
                let api_key = self
                    .secret_box
                    .decrypt(encrypted)
                    .map_err(DomainError::Internal)?;
                Ok(Some(ImageGenerationSettings {
                    api_key,
                    model: normalize_image_model(Some(&s.image_generation_model)),
                    art_style: normalize_art_style(Some(&s.image_generation_art_style)),
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
        let model_for_save = resolve_model_for_save(existing.as_ref(), input.anthropic_model)?;
        let key_change =
            resolve_api_key_change(input.anthropic_api_key, input.clear_anthropic_api_key);
        let gemini_key_change =
            resolve_api_key_change(input.gemini_api_key, input.clear_gemini_api_key);

        let (replace_key, clear_key) = match key_change {
            ApiKeyChange::KeepExisting => (None, false),
            ApiKeyChange::Clear => (None, true),
            ApiKeyChange::Replace(value) => (
                Some(
                    self.secret_box
                        .encrypt(&value)
                        .map_err(DomainError::InvalidInput)?,
                ),
                false,
            ),
        };
        let (replace_gemini_key, clear_gemini_key) = match gemini_key_change {
            ApiKeyChange::KeepExisting => (None, false),
            ApiKeyChange::Clear => (None, true),
            ApiKeyChange::Replace(value) => (
                Some(
                    self.secret_box
                        .encrypt(&value)
                        .map_err(DomainError::InvalidInput)?,
                ),
                false,
            ),
        };
        let image_model = resolve_image_model(existing.as_ref(), input.image_generation_model)?;
        let art_style = resolve_art_style(existing.as_ref(), input.image_generation_art_style);

        let saved = self
            .repo
            .upsert(
                user_id,
                input.enabled,
                replace_key.as_deref(),
                clear_key,
                &model_for_save,
                input.image_generation_enabled,
                replace_gemini_key.as_deref(),
                clear_gemini_key,
                &image_model,
                &art_style,
            )
            .await?;

        Ok(to_view(Some(&saved)))
    }
}

fn resolve_image_model(
    existing: Option<&LlmSettings>,
    model: Option<String>,
) -> Result<String, DomainError> {
    match model.as_deref().map(str::trim) {
        None | Some("") => Ok(existing
            .map(|settings| normalize_image_model(Some(&settings.image_generation_model)))
            .unwrap_or_else(|| DEFAULT_IMAGE_GENERATION_MODEL.to_string())),
        Some(DEFAULT_IMAGE_GENERATION_MODEL) => Ok(DEFAULT_IMAGE_GENERATION_MODEL.to_string()),
        Some(_) => Err(DomainError::InvalidInput(
            "Unsupported Gemini image model selection".into(),
        )),
    }
}

fn normalize_image_model(model: Option<&str>) -> String {
    match model.map(str::trim) {
        Some(DEFAULT_IMAGE_GENERATION_MODEL) => DEFAULT_IMAGE_GENERATION_MODEL.to_string(),
        _ => DEFAULT_IMAGE_GENERATION_MODEL.to_string(),
    }
}

fn normalize_art_style(style: Option<&str>) -> String {
    style
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_IMAGE_ART_STYLE)
        .to_string()
}

fn resolve_art_style(existing: Option<&LlmSettings>, submitted: Option<String>) -> String {
    match submitted {
        Some(value) if !value.trim().is_empty() => value.trim().to_string(),
        Some(_) => DEFAULT_IMAGE_ART_STYLE.to_string(),
        None => existing
            .map(|settings| normalize_art_style(Some(&settings.image_generation_art_style)))
            .unwrap_or_else(|| DEFAULT_IMAGE_ART_STYLE.to_string()),
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

fn resolve_model_for_save(
    existing: Option<&LlmSettings>,
    submitted: Option<String>,
) -> Result<String, DomainError> {
    match submitted.as_deref().map(str::trim) {
        None | Some("") => {
            if let Some(settings) = existing {
                return Ok(settings.anthropic_model.trim().to_string());
            }
            Ok(DEFAULT_ANTHROPIC_MODEL.to_string())
        }
        Some(value)
            if ANTHROPIC_MODEL_OPTIONS
                .iter()
                .any(|option| option.value == value) =>
        {
            Ok(value.to_string())
        }
        Some(value)
            if existing
                .map(|settings| settings.anthropic_model.trim() == value)
                .unwrap_or(false) =>
        {
            Ok(value.to_string())
        }
        Some(_) => Err(DomainError::InvalidInput(
            "Unsupported Anthropic model selection".into(),
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
        Some(settings) => SettingsView {
            enabled: settings.enabled,
            has_anthropic_api_key: settings.anthropic_api_key_encrypted.is_some(),
            anthropic_model: normalize_model(Some(settings.anthropic_model.clone())),
            image_generation_enabled: settings.image_generation_enabled,
            has_gemini_api_key: settings.gemini_api_key_encrypted.is_some(),
            image_generation_model: normalize_image_model(Some(&settings.image_generation_model)),
            image_generation_art_style: normalize_art_style(Some(
                &settings.image_generation_art_style,
            )),
        },
        None => SettingsView {
            enabled: false,
            has_anthropic_api_key: false,
            anthropic_model: DEFAULT_ANTHROPIC_MODEL.to_string(),
            image_generation_enabled: false,
            has_gemini_api_key: false,
            image_generation_model: DEFAULT_IMAGE_GENERATION_MODEL.to_string(),
            image_generation_art_style: DEFAULT_IMAGE_ART_STYLE.to_string(),
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

    struct LastUpsert {
        enabled: bool,
        replace_anthropic_api_key_encrypted: Option<Vec<u8>>,
        clear_anthropic_api_key: bool,
        anthropic_model: String,
        image_generation_enabled: bool,
        replace_gemini_api_key_encrypted: Option<Vec<u8>>,
        clear_gemini_api_key: bool,
        image_generation_model: String,
        image_generation_art_style: String,
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
            enabled: bool,
            replace_anthropic_api_key_encrypted: Option<&[u8]>,
            clear_anthropic_api_key: bool,
            anthropic_model: &str,
            image_generation_enabled: bool,
            replace_gemini_api_key_encrypted: Option<&[u8]>,
            clear_gemini_api_key: bool,
            image_generation_model: &str,
            image_generation_art_style: &str,
        ) -> Result<LlmSettings, DomainError> {
            let existing = self.stored.lock().expect("stored lock").clone();
            let anthropic_encrypted = if clear_anthropic_api_key {
                None
            } else {
                replace_anthropic_api_key_encrypted
                    .map(|value| value.to_vec())
                    .or_else(|| {
                        existing
                            .as_ref()
                            .and_then(|settings| settings.anthropic_api_key_encrypted.clone())
                    })
            };
            let gemini_encrypted = if clear_gemini_api_key {
                None
            } else {
                replace_gemini_api_key_encrypted
                    .map(|value| value.to_vec())
                    .or_else(|| {
                        existing
                            .as_ref()
                            .and_then(|settings| settings.gemini_api_key_encrypted.clone())
                    })
            };

            *self.last_upsert.lock().expect("last_upsert lock") = Some(LastUpsert {
                enabled,
                replace_anthropic_api_key_encrypted: replace_anthropic_api_key_encrypted
                    .map(|value| value.to_vec()),
                clear_anthropic_api_key,
                anthropic_model: anthropic_model.to_string(),
                image_generation_enabled,
                replace_gemini_api_key_encrypted: replace_gemini_api_key_encrypted
                    .map(|value| value.to_vec()),
                clear_gemini_api_key,
                image_generation_model: image_generation_model.to_string(),
                image_generation_art_style: image_generation_art_style.to_string(),
            });

            let saved = LlmSettings {
                user_id,
                enabled,
                anthropic_api_key_encrypted: anthropic_encrypted,
                anthropic_model: anthropic_model.to_string(),
                image_generation_enabled,
                gemini_api_key_encrypted: gemini_encrypted,
                image_generation_model: image_generation_model.to_string(),
                image_generation_art_style: image_generation_art_style.to_string(),
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
                anthropic_api_key_encrypted: Some(vec![1, 2, 3]),
                anthropic_model: "claude-3-7-sonnet-latest".into(),
                ..Default::default()
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
                anthropic_api_key_encrypted: Some(vec![1, 2, 3]),
                anthropic_model: "claude-3-7-sonnet-latest".into(),
                ..Default::default()
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
                anthropic_api_key_encrypted: Some(vec![1, 2, 3]),
                anthropic_model: "claude-3-7-sonnet-latest".into(),
                ..Default::default()
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
                anthropic_api_key_encrypted: Some(vec![1, 2, 3]),
                anthropic_model: "claude-3-7-sonnet-latest".into(),
                ..Default::default()
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
    async fn save_encrypts_gemini_key_and_persists_image_generation_settings() {
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
                    image_generation_enabled: true,
                    gemini_api_key: Some("AIza-test-key".into()),
                    image_generation_model: Some(DEFAULT_IMAGE_GENERATION_MODEL.into()),
                    image_generation_art_style: Some(
                        "Flat paper-cut collage in coral and cobalt".into(),
                    ),
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
            .replace_gemini_api_key_encrypted
            .expect("encrypted Gemini key");

        assert!(last_upsert.image_generation_enabled);
        assert!(!last_upsert.clear_gemini_api_key);
        assert_eq!(
            last_upsert.image_generation_model,
            DEFAULT_IMAGE_GENERATION_MODEL
        );
        assert_eq!(
            last_upsert.image_generation_art_style,
            "Flat paper-cut collage in coral and cobalt"
        );
        assert_ne!(encrypted, b"AIza-test-key");
        assert_eq!(
            secret_box.decrypt(&encrypted).expect("decrypt"),
            "AIza-test-key"
        );
        assert!(view.image_generation_enabled);
        assert!(view.has_gemini_api_key);
        assert_eq!(view.image_generation_model, DEFAULT_IMAGE_GENERATION_MODEL);
        assert_eq!(
            view.image_generation_art_style,
            "Flat paper-cut collage in coral and cobalt"
        );

        let image_settings = service
            .get_image_generation_settings(user_id)
            .await
            .expect("get image settings")
            .expect("configured image settings");
        assert_eq!(image_settings.api_key, "AIza-test-key");
        assert_eq!(image_settings.model, DEFAULT_IMAGE_GENERATION_MODEL);
        assert_eq!(
            image_settings.art_style,
            "Flat paper-cut collage in coral and cobalt"
        );
    }

    #[tokio::test]
    async fn clearing_gemini_key_removes_image_generation_access() {
        let repo = Arc::new(FakeLlmSettingsRepository::new());
        let secret_box = Arc::new(SecretBox::new(
            "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
        ));
        let service = SettingsService::new(repo.clone(), secret_box);
        let user_id = Uuid::new_v4();

        service
            .save(
                user_id,
                SaveLlmSettingsInput {
                    image_generation_enabled: true,
                    gemini_api_key: Some("AIza-test-key".into()),
                    ..Default::default()
                },
            )
            .await
            .expect("initial save");
        let view = service
            .save(
                user_id,
                SaveLlmSettingsInput {
                    image_generation_enabled: true,
                    clear_gemini_api_key: true,
                    ..Default::default()
                },
            )
            .await
            .expect("clear save");

        assert!(!view.has_gemini_api_key);
        assert!(
            service
                .get_image_generation_settings(user_id)
                .await
                .expect("get image settings")
                .is_none()
        );
        let last_upsert = repo
            .last_upsert
            .lock()
            .expect("last_upsert lock")
            .take()
            .expect("last_upsert");
        assert!(last_upsert.clear_gemini_api_key);
        assert!(last_upsert.replace_gemini_api_key_encrypted.is_none());
    }

    #[tokio::test]
    async fn blank_art_style_uses_the_default_style() {
        let repo = Arc::new(FakeLlmSettingsRepository::new());
        let secret_box = Arc::new(SecretBox::new(
            "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
        ));
        let service = SettingsService::new(repo, secret_box);

        let view = service
            .save(
                Uuid::new_v4(),
                SaveLlmSettingsInput {
                    image_generation_art_style: Some("   ".into()),
                    ..Default::default()
                },
            )
            .await
            .expect("save");

        assert_eq!(view.image_generation_art_style, DEFAULT_IMAGE_ART_STYLE);
    }

    #[tokio::test]
    async fn save_rejects_unsupported_image_generation_model() {
        let repo = Arc::new(FakeLlmSettingsRepository::new());
        let secret_box = Arc::new(SecretBox::new(
            "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
        ));
        let service = SettingsService::new(repo, secret_box);

        let error = service
            .save(
                Uuid::new_v4(),
                SaveLlmSettingsInput {
                    image_generation_model: Some("gemini-3.1-flash-image".into()),
                    ..Default::default()
                },
            )
            .await
            .err()
            .expect("invalid image model should fail");

        assert!(matches!(error, DomainError::InvalidInput(_)));
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
}
