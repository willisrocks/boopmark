use crate::domain::error::DomainError;
use crate::domain::llm_settings::TextProvider;
use std::future::Future;
use std::pin::Pin;

/// Raw scraped metadata sent to the LLM for enrichment.
pub struct EnrichmentInput {
    pub url: String,
    pub scraped_title: Option<String>,
    pub scraped_description: Option<String>,
    pub existing_tags: Option<Vec<(String, i64)>>,
}

/// LLM-suggested improvements.
pub struct EnrichmentOutput {
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
}

pub trait LlmEnricher: Send + Sync {
    fn enrich(
        &self,
        api_key: &str,
        model: &str,
        input: EnrichmentInput,
    ) -> Pin<Box<dyn Future<Output = Result<EnrichmentOutput, DomainError>> + Send + '_>>;

    /// Provider-aware entry point used by the application layer. Existing
    /// adapters remain source-compatible because their provider-independent
    /// implementation is a sensible default; routers can override this to
    /// dispatch explicitly instead of inferring a provider from a model ID.
    fn enrich_with_provider(
        &self,
        _provider: TextProvider,
        api_key: &str,
        model: &str,
        input: EnrichmentInput,
    ) -> Pin<Box<dyn Future<Output = Result<EnrichmentOutput, DomainError>> + Send + '_>> {
        self.enrich(api_key, model, input)
    }
}
