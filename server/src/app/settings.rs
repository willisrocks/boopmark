use crate::app::secrets::SecretBox;
use crate::domain::error::DomainError;
use crate::domain::llm_settings::{DEFAULT_ANTHROPIC_MODEL, LlmSettings};
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
}

pub struct SaveLlmSettingsInput {
    pub enabled: bool,
    pub anthropic_api_key: Option<String>,
    pub clear_anthropic_api_key: bool,
    pub anthropic_model: Option<String>,
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

    pub async fn save(
        &self,
        user_id: Uuid,
        input: SaveLlmSettingsInput,
    ) -> Result<SettingsView, DomainError> {
        let normalized_model = normalize_model(input.anthropic_model);
        let key_change = resolve_api_key_change(
            input.anthropic_api_key,
            input.clear_anthropic_api_key,
        );

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

        let saved = self
            .repo
            .upsert(
                user_id,
                input.enabled,
                replace_key.as_deref(),
                clear_key,
                &normalized_model,
            )
            .await?;

        Ok(to_view(Some(&saved)))
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
        },
        None => SettingsView {
            enabled: false,
            has_anthropic_api_key: false,
            anthropic_model: DEFAULT_ANTHROPIC_MODEL.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        ) -> Result<LlmSettings, DomainError> {
            let existing = self.stored.lock().expect("stored lock").clone();
            let encrypted = if clear_anthropic_api_key {
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

            *self.last_upsert.lock().expect("last_upsert lock") = Some(LastUpsert {
                enabled,
                replace_anthropic_api_key_encrypted: replace_anthropic_api_key_encrypted
                    .map(|value| value.to_vec()),
                clear_anthropic_api_key,
                anthropic_model: anthropic_model.to_string(),
            });

            let saved = LlmSettings {
                user_id,
                enabled,
                anthropic_api_key_encrypted: encrypted,
                anthropic_model: anthropic_model.to_string(),
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
    fn normalize_model_defaults_to_latest_haiku_alias() {
        assert_eq!(normalize_model(None), "claude-haiku-4-5");
        assert_eq!(normalize_model(Some("   ".into())), "claude-haiku-4-5");
        assert_eq!(
            normalize_model(Some("claude-haiku-4-5-20251001".into())),
            "claude-haiku-4-5-20251001"
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
        assert_eq!(secret_box.decrypt(&encrypted).expect("decrypt"), "sk-ant-test");
        assert!(view.enabled);
        assert!(view.has_anthropic_api_key);
        assert_eq!(view.anthropic_model, "claude-haiku-4-5-20251001");
    }
}
