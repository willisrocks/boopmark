use crate::domain::error::DomainError;
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
}
