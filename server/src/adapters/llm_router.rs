use crate::domain::error::DomainError;
use crate::domain::llm_settings::TextProvider;
use crate::domain::ports::image_generator::ImageGenerationContext;
use crate::domain::ports::llm_enricher::{EnrichmentInput, EnrichmentOutput, LlmEnricher};
use crate::domain::ports::llm_prompt_assistant::LlmPromptAssistant;
use crate::domain::ports::tag_consolidator::{
    ConsolidationInput, ConsolidationOutput, TagConsolidator,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Provider router for metadata enrichment. Provider choice comes from the
/// persisted settings value, not from model-name prefix guessing, so legacy
/// Anthropic model IDs remain safe and future model naming is explicit.
#[derive(Clone)]
pub struct LlmEnricherRouter {
    anthropic: Arc<dyn LlmEnricher>,
    openai: Arc<dyn LlmEnricher>,
}

impl LlmEnricherRouter {
    pub fn new(anthropic: Arc<dyn LlmEnricher>, openai: Arc<dyn LlmEnricher>) -> Self {
        Self { anthropic, openai }
    }

    fn provider(&self, provider: TextProvider) -> &Arc<dyn LlmEnricher> {
        match provider {
            TextProvider::Anthropic => &self.anthropic,
            TextProvider::OpenAi => &self.openai,
        }
    }
}

impl LlmEnricher for LlmEnricherRouter {
    fn enrich(
        &self,
        api_key: &str,
        model: &str,
        input: EnrichmentInput,
    ) -> Pin<Box<dyn Future<Output = Result<EnrichmentOutput, DomainError>> + Send + '_>> {
        // Compatibility callers do not pass a provider. Existing callers all
        // use the selected credentials, and OpenAI model IDs are unambiguous.
        let provider = if model.starts_with("gpt-") {
            TextProvider::OpenAi
        } else {
            TextProvider::Anthropic
        };
        self.enrich_with_provider(provider, api_key, model, input)
    }

    fn enrich_with_provider(
        &self,
        provider: TextProvider,
        api_key: &str,
        model: &str,
        input: EnrichmentInput,
    ) -> Pin<Box<dyn Future<Output = Result<EnrichmentOutput, DomainError>> + Send + '_>> {
        self.provider(provider).enrich(api_key, model, input)
    }
}

/// Provider router for image-prompt assistance. Pixel rendering remains
/// OpenAI-only, but the selected text provider writes the prompt direction.
#[derive(Clone)]
pub struct LlmPromptAssistantRouter {
    anthropic: Arc<dyn LlmPromptAssistant>,
    openai: Arc<dyn LlmPromptAssistant>,
}

impl LlmPromptAssistantRouter {
    pub fn new(
        anthropic: Arc<dyn LlmPromptAssistant>,
        openai: Arc<dyn LlmPromptAssistant>,
    ) -> Self {
        Self { anthropic, openai }
    }

    fn provider(&self, provider: TextProvider) -> &Arc<dyn LlmPromptAssistant> {
        match provider {
            TextProvider::Anthropic => &self.anthropic,
            TextProvider::OpenAi => &self.openai,
        }
    }
}

impl LlmPromptAssistant for LlmPromptAssistantRouter {
    fn assist_image_prompt(
        &self,
        api_key: &str,
        model: &str,
        context: ImageGenerationContext,
        instruction: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Result<String, DomainError>> + Send + '_>> {
        let provider = if model.starts_with("gpt-") {
            TextProvider::OpenAi
        } else {
            TextProvider::Anthropic
        };
        self.assist_image_prompt_with_provider(provider, api_key, model, context, instruction)
    }

    fn assist_image_prompt_with_provider(
        &self,
        provider: TextProvider,
        api_key: &str,
        model: &str,
        context: ImageGenerationContext,
        instruction: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Result<String, DomainError>> + Send + '_>> {
        self.provider(provider)
            .assist_image_prompt(api_key, model, context, instruction)
    }
}

/// Provider router for tag consolidation, paired with `LlmEnricherRouter` so
/// both operations always honor the same metadata-provider setting.
#[derive(Clone)]
pub struct TagConsolidatorRouter {
    anthropic: Arc<dyn TagConsolidator>,
    openai: Arc<dyn TagConsolidator>,
}

impl TagConsolidatorRouter {
    pub fn new(anthropic: Arc<dyn TagConsolidator>, openai: Arc<dyn TagConsolidator>) -> Self {
        Self { anthropic, openai }
    }

    fn provider(&self, provider: TextProvider) -> &Arc<dyn TagConsolidator> {
        match provider {
            TextProvider::Anthropic => &self.anthropic,
            TextProvider::OpenAi => &self.openai,
        }
    }
}

impl TagConsolidator for TagConsolidatorRouter {
    fn consolidate(
        &self,
        api_key: &str,
        model: &str,
        input: ConsolidationInput,
    ) -> Pin<Box<dyn Future<Output = Result<ConsolidationOutput, DomainError>> + Send + '_>> {
        let provider = if model.starts_with("gpt-") {
            TextProvider::OpenAi
        } else {
            TextProvider::Anthropic
        };
        self.consolidate_with_provider(provider, api_key, model, input)
    }

    fn consolidate_with_provider(
        &self,
        provider: TextProvider,
        api_key: &str,
        model: &str,
        input: ConsolidationInput,
    ) -> Pin<Box<dyn Future<Output = Result<ConsolidationOutput, DomainError>> + Send + '_>> {
        self.provider(provider).consolidate(api_key, model, input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ports::llm_enricher::EnrichmentOutput;
    use crate::domain::ports::tag_consolidator::TagSample;
    use std::collections::HashMap;

    struct StubEnricher {
        marker: &'static str,
    }

    impl LlmEnricher for StubEnricher {
        fn enrich(
            &self,
            _api_key: &str,
            _model: &str,
            _input: EnrichmentInput,
        ) -> Pin<Box<dyn Future<Output = Result<EnrichmentOutput, DomainError>> + Send + '_>>
        {
            let marker = self.marker;
            Box::pin(async move {
                Ok(EnrichmentOutput {
                    title: Some(marker.to_string()),
                    description: None,
                    tags: vec![],
                })
            })
        }
    }

    struct StubConsolidator {
        marker: &'static str,
    }

    impl TagConsolidator for StubConsolidator {
        fn consolidate(
            &self,
            _api_key: &str,
            _model: &str,
            _input: ConsolidationInput,
        ) -> Pin<Box<dyn Future<Output = Result<ConsolidationOutput, DomainError>> + Send + '_>>
        {
            let marker = self.marker;
            Box::pin(async move {
                Ok(ConsolidationOutput {
                    mapping: HashMap::from([(marker.to_string(), vec![marker.to_string()])]),
                })
            })
        }
    }

    struct StubPromptAssistant {
        marker: &'static str,
    }

    impl LlmPromptAssistant for StubPromptAssistant {
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

    #[tokio::test]
    async fn enrichment_router_dispatches_by_explicit_provider() {
        let router = LlmEnricherRouter::new(
            Arc::new(StubEnricher {
                marker: "anthropic",
            }),
            Arc::new(StubEnricher { marker: "openai" }),
        );
        let output = router
            .enrich_with_provider(
                TextProvider::OpenAi,
                "key",
                "gpt-5.6-luna",
                EnrichmentInput {
                    url: "https://example.com".into(),
                    scraped_title: None,
                    scraped_description: None,
                    existing_tags: None,
                },
            )
            .await
            .expect("output");
        assert_eq!(output.title.as_deref(), Some("openai"));
    }

    #[tokio::test]
    async fn consolidation_router_dispatches_by_explicit_provider() {
        let router = TagConsolidatorRouter::new(
            Arc::new(StubConsolidator {
                marker: "anthropic",
            }),
            Arc::new(StubConsolidator { marker: "openai" }),
        );
        let output = router
            .consolidate_with_provider(
                TextProvider::OpenAi,
                "key",
                "gpt-5.6-luna",
                ConsolidationInput {
                    tags: vec![TagSample {
                        tag: "one".into(),
                        count: 1,
                        sample_titles: vec![],
                    }],
                },
            )
            .await
            .expect("output");
        assert!(output.mapping.contains_key("openai"));
    }

    #[tokio::test]
    async fn prompt_assistant_router_dispatches_to_both_explicit_providers() {
        let router = LlmPromptAssistantRouter::new(
            Arc::new(StubPromptAssistant {
                marker: "anthropic",
            }),
            Arc::new(StubPromptAssistant { marker: "openai" }),
        );
        let context = || ImageGenerationContext {
            url: "https://example.com".to_string(),
            title: Some("Title".to_string()),
            description: Some("Description".to_string()),
        };

        let anthropic = router
            .assist_image_prompt_with_provider(
                TextProvider::Anthropic,
                "key",
                "claude-haiku-4-5-20251001",
                context(),
                None,
            )
            .await
            .expect("Anthropic prompt");
        assert_eq!(anthropic, "anthropic");

        let openai = router
            .assist_image_prompt_with_provider(
                TextProvider::OpenAi,
                "key",
                "gpt-5.6-luna",
                context(),
                None,
            )
            .await
            .expect("OpenAI prompt");
        assert_eq!(openai, "openai");
    }
}
