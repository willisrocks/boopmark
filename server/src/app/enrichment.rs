use crate::app::settings::SettingsService;
use crate::domain::ports::llm_enricher::{EnrichmentInput, LlmEnricher};
use crate::domain::ports::llm_settings_repo::LlmSettingsRepository;
use crate::domain::ports::metadata::MetadataExtractor;
use serde::Serialize;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
pub struct SuggestionResult {
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub image_url: Option<String>,
    pub domain: Option<String>,
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use crate::app::secrets::SecretBox;
    use crate::domain::bookmark::UrlMetadata;
    use crate::domain::error::DomainError;
    use crate::domain::llm_settings::{DEFAULT_ANTHROPIC_MODEL, LlmSettings};
    use crate::domain::ports::llm_enricher::EnrichmentOutput;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeSettings(Option<LlmSettings>);

    impl LlmSettingsRepository for FakeSettings {
        async fn get(&self, _user_id: Uuid) -> Result<Option<LlmSettings>, DomainError> {
            Ok(self.0.clone())
        }

        async fn upsert(
            &self,
            _user_id: Uuid,
            _enabled: bool,
            _replace_anthropic_api_key_encrypted: Option<&[u8]>,
            _clear_anthropic_api_key: bool,
            _anthropic_model: &str,
            _image_generation_enabled: bool,
            _replace_gemini_api_key_encrypted: Option<&[u8]>,
            _clear_gemini_api_key: bool,
            _image_generation_model: &str,
            _image_generation_art_style: &str,
        ) -> Result<LlmSettings, DomainError> {
            panic!("suggestions must not modify account settings")
        }
    }

    struct FakeMetadata {
        fail: bool,
    }

    impl MetadataExtractor for FakeMetadata {
        fn extract(
            &self,
            _url: &str,
        ) -> Pin<Box<dyn Future<Output = Result<UrlMetadata, DomainError>> + Send + '_>> {
            Box::pin(async move {
                if self.fail {
                    return Err(DomainError::InvalidInput("scrape failed".into()));
                }
                Ok(UrlMetadata {
                    title: Some("Scraped title".into()),
                    description: Some("Scraped description".into()),
                    image_url: Some("https://example.com/image.png".into()),
                    domain: Some("example.com".into()),
                })
            })
        }
    }

    struct CountingEnricher {
        calls: AtomicUsize,
        fail: bool,
        partial: bool,
    }

    impl LlmEnricher for CountingEnricher {
        fn enrich(
            &self,
            api_key: &str,
            model: &str,
            input: EnrichmentInput,
        ) -> Pin<Box<dyn Future<Output = Result<EnrichmentOutput, DomainError>> + Send + '_>>
        {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(api_key, "fake-provider-key");
            assert_eq!(model, DEFAULT_ANTHROPIC_MODEL);
            assert_eq!(input.url, "https://example.com/article?capture=1#section");
            Box::pin(async move {
                if self.fail {
                    return Err(DomainError::InvalidInput("provider failed".into()));
                }
                Ok(EnrichmentOutput {
                    title: Some("AI title".into()),
                    description: (!self.partial).then(|| "AI description".into()),
                    tags: if self.partial {
                        vec![]
                    } else {
                        vec!["ai-tag".into()]
                    },
                })
            })
        }
    }

    async fn suggest_fixture(
        configuration: Option<(bool, bool)>,
        provider_fail: bool,
        provider_partial: bool,
        scrape_fail: bool,
    ) -> (SuggestionResult, usize) {
        let secret_box = Arc::new(SecretBox::new(
            "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
        ));
        let user_id = Uuid::new_v4();
        let stored = configuration.map(|(enabled, has_key)| LlmSettings {
            user_id,
            enabled,
            anthropic_api_key_encrypted: has_key.then(|| {
                secret_box
                    .encrypt("fake-provider-key")
                    .expect("encrypt fake key")
            }),
            anthropic_model: DEFAULT_ANTHROPIC_MODEL.into(),
            ..Default::default()
        });
        let settings = Arc::new(SettingsService::new(
            Arc::new(FakeSettings(stored)),
            secret_box,
        ));
        let enricher = Arc::new(CountingEnricher {
            calls: AtomicUsize::new(0),
            fail: provider_fail,
            partial: provider_partial,
        });
        let service = EnrichmentService::new(
            Arc::new(FakeMetadata { fail: scrape_fail }),
            enricher.clone(),
            settings,
        );
        let result = service
            .suggest(
                user_id,
                "https://example.com/article?capture=1#section",
                None,
            )
            .await;
        (result, enricher.calls.load(Ordering::SeqCst))
    }

    fn assert_scrape_fallback(result: &SuggestionResult) {
        assert_eq!(result.title.as_deref(), Some("Scraped title"));
        assert_eq!(result.description.as_deref(), Some("Scraped description"));
        assert!(result.tags.is_empty());
        assert_eq!(
            result.image_url.as_deref(),
            Some("https://example.com/image.png")
        );
        assert_eq!(result.domain.as_deref(), Some("example.com"));
    }

    #[tokio::test]
    async fn explicitly_disabled_ai_with_stored_key_never_calls_provider() {
        let (result, calls) = suggest_fixture(Some((false, true)), false, false, false).await;
        assert_eq!(calls, 0);
        assert_scrape_fallback(&result);
    }

    #[tokio::test]
    async fn missing_ai_settings_never_calls_provider() {
        let (result, calls) = suggest_fixture(None, false, false, false).await;
        assert_eq!(calls, 0);
        assert_scrape_fallback(&result);
    }

    #[tokio::test]
    async fn enabled_ai_without_key_never_calls_provider() {
        let (result, calls) = suggest_fixture(Some((true, false)), false, false, false).await;
        assert_eq!(calls, 0);
        assert_scrape_fallback(&result);
    }

    #[tokio::test]
    async fn configured_ai_runs_automatically_and_takes_priority_over_scrape() {
        let (result, calls) = suggest_fixture(Some((true, true)), false, false, false).await;
        assert_eq!(calls, 1);
        assert_eq!(result.title.as_deref(), Some("AI title"));
        assert_eq!(result.description.as_deref(), Some("AI description"));
        assert_eq!(result.tags, ["ai-tag"]);
        assert_eq!(
            result.image_url.as_deref(),
            Some("https://example.com/image.png")
        );
    }

    #[tokio::test]
    async fn provider_failure_returns_scraped_metadata_without_retry() {
        let (result, calls) = suggest_fixture(Some((true, true)), true, false, false).await;
        assert_eq!(calls, 1);
        assert_scrape_fallback(&result);
    }

    #[tokio::test]
    async fn partial_ai_output_falls_back_per_field() {
        let (result, calls) = suggest_fixture(Some((true, true)), false, true, false).await;
        assert_eq!(calls, 1);
        assert_eq!(result.title.as_deref(), Some("AI title"));
        assert_eq!(result.description.as_deref(), Some("Scraped description"));
        assert!(result.tags.is_empty());
    }

    #[tokio::test]
    async fn failed_scrape_and_unconfigured_ai_return_optional_empty_fields() {
        let (result, calls) = suggest_fixture(None, false, false, true).await;
        assert_eq!(calls, 0);
        assert!(result.title.is_none());
        assert!(result.description.is_none());
        assert!(result.tags.is_empty());
        assert!(result.image_url.is_none());
        assert!(result.domain.is_none());
    }
}

pub struct EnrichmentService<M, R> {
    metadata: Arc<M>,
    enricher: Arc<dyn LlmEnricher>,
    settings: Arc<SettingsService<R>>,
}

impl<M, R> EnrichmentService<M, R>
where
    M: MetadataExtractor + Send + Sync,
    R: LlmSettingsRepository + Send + Sync,
{
    pub fn new(
        metadata: Arc<M>,
        enricher: Arc<dyn LlmEnricher>,
        settings: Arc<SettingsService<R>>,
    ) -> Self {
        Self {
            metadata,
            enricher,
            settings,
        }
    }

    pub async fn suggest(
        &self,
        user_id: Uuid,
        url: &str,
        existing_tags: Option<Vec<(String, i64)>>,
    ) -> SuggestionResult {
        if url.trim().is_empty() {
            return SuggestionResult {
                title: None,
                description: None,
                tags: vec![],
                image_url: None,
                domain: None,
            };
        }

        // Scrape metadata
        let metadata = match self.metadata.extract(url).await {
            Ok(meta) => Some(meta),
            Err(e) => {
                tracing::warn!(url = %url, error = %e, "metadata scrape failed");
                None
            }
        };

        // Attempt LLM enrichment
        let enrichment = self
            .try_llm_enrich(user_id, url, &metadata, existing_tags)
            .await;

        // Merge: LLM takes priority over scrape for title/description/tags
        SuggestionResult {
            title: enrichment
                .as_ref()
                .and_then(|e| e.title.clone())
                .or_else(|| metadata.as_ref().and_then(|m| m.title.clone())),
            description: enrichment
                .as_ref()
                .and_then(|e| e.description.clone())
                .or_else(|| metadata.as_ref().and_then(|m| m.description.clone())),
            tags: enrichment
                .as_ref()
                .map(|e| e.tags.clone())
                .filter(|t| !t.is_empty())
                .unwrap_or_default(),
            image_url: metadata.as_ref().and_then(|m| m.image_url.clone()),
            domain: metadata.as_ref().and_then(|m| m.domain.clone()),
        }
    }

    async fn try_llm_enrich(
        &self,
        user_id: Uuid,
        url: &str,
        metadata: &Option<crate::domain::bookmark::UrlMetadata>,
        existing_tags: Option<Vec<(String, i64)>>,
    ) -> Option<crate::domain::ports::llm_enricher::EnrichmentOutput> {
        let (api_key, model) = match self.settings.get_decrypted_api_key(user_id).await {
            Ok(Some(pair)) => pair,
            Ok(None) => return None,
            Err(e) => {
                tracing::warn!(user_id = %user_id, error = %e, "failed to load LLM settings for enrichment");
                return None;
            }
        };

        let input = EnrichmentInput {
            url: url.to_string(),
            scraped_title: metadata.as_ref().and_then(|m| m.title.clone()),
            scraped_description: metadata.as_ref().and_then(|m| m.description.clone()),
            existing_tags,
        };

        match self.enricher.enrich(&api_key, &model, input).await {
            Ok(output) => Some(output),
            Err(e) => {
                tracing::warn!(user_id = %user_id, url = %url, error = %e, "LLM enrichment failed, falling back to scrape-only");
                None
            }
        }
    }
}
