use crate::domain::error::DomainError;
use crate::domain::llm_settings::TextProvider;
use crate::domain::ports::llm_client::BoxFuture;

/// Raw scraped metadata sent to the LLM for enrichment.
pub struct EnrichmentInput {
    pub url: String,
    pub scraped_title: Option<String>,
    pub scraped_description: Option<String>,
    /// The user's canonical tags with bookmark counts, most used first.
    pub canonical_tags: Vec<(String, i64)>,
}

/// LLM-suggested improvements.
pub struct EnrichmentOutput {
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
}

pub trait LlmEnricher: Send + Sync {
    /// `provider`, `api_key` and `model` come from the user's LLM settings.
    fn enrich(
        &self,
        provider: TextProvider,
        api_key: &str,
        model: &str,
        input: EnrichmentInput,
    ) -> BoxFuture<'_, Result<EnrichmentOutput, DomainError>>;
}
