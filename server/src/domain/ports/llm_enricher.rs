use crate::domain::error::DomainError;

/// Raw scraped metadata sent to the LLM for enrichment.
pub struct EnrichmentInput {
    pub url: String,
    pub scraped_title: Option<String>,
    pub scraped_description: Option<String>,
}

/// LLM-suggested improvements.
pub struct EnrichmentOutput {
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
}

#[trait_variant::make(Send)]
pub trait LlmEnricher: Send + Sync {
    async fn enrich(
        &self,
        api_key: &str,
        model: &str,
        input: EnrichmentInput,
    ) -> Result<EnrichmentOutput, DomainError>;
}
