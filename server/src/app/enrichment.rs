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
        let enrichment = self.try_llm_enrich(user_id, url, &metadata, existing_tags).await;

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
