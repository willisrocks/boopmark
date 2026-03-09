use crate::app::secrets::SecretBox;
use crate::domain::error::DomainError;
use crate::domain::llm_settings::{ANTHROPIC_MODEL_OPTIONS, DEFAULT_ANTHROPIC_MODEL, LlmSettings};
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

    pub async fn save(
        &self,
        user_id: Uuid,
        input: SaveLlmSettingsInput,
    ) -> Result<SettingsView, DomainError> {
        let existing = self.repo.get(user_id).await?;
        let model_for_save = resolve_model_for_save(existing.as_ref(), input.anthropic_model)?;
        let key_change =
            resolve_api_key_change(input.anthropic_api_key, input.clear_anthropic_api_key);

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
                &model_for_save,
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
                anthropic_api_key_encrypted: Some(vec![1, 2, 3]),
                anthropic_model: "claude-3-7-sonnet-latest".into(),
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
                },
            )
            .await
            .err()
            .expect("invalid model should fail");

        assert!(matches!(error, DomainError::InvalidInput(_)));
    }
}
