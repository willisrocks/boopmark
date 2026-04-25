use crate::domain::error::DomainError;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

/// One tag in the user's library, with bookmark count and a few sample titles
/// for the LLM to disambiguate ambiguous names.
#[derive(Debug, Clone)]
pub struct TagSample {
    pub tag: String,
    pub count: i64,
    /// Up to 3 representative bookmark titles for this tag.
    pub sample_titles: Vec<String>,
}

pub struct ConsolidationInput {
    pub tags: Vec<TagSample>,
}

pub struct ConsolidationOutput {
    /// Maps each input tag (case-preserving) to the full list of tags a bookmark
    /// currently carrying that tag should end up with.
    pub mapping: HashMap<String, Vec<String>>,
}

pub trait TagConsolidator: Send + Sync {
    fn consolidate(
        &self,
        api_key: &str,
        model: &str,
        input: ConsolidationInput,
    ) -> Pin<Box<dyn Future<Output = Result<ConsolidationOutput, DomainError>> + Send + '_>>;
}
