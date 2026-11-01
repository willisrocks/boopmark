use crate::domain::error::DomainError;
use crate::domain::ports::llm_client::BoxFuture;
use uuid::Uuid;

/// Read-only view of a user's canonical tags, for save-time suggestions.
pub trait TagVocabulary: Send + Sync {
    /// Canonical tags with bookmark counts, most used first.
    fn canonical_tags(
        &self,
        user_id: Uuid,
    ) -> BoxFuture<'_, Result<Vec<(String, i64)>, DomainError>>;

    /// The tags `tags` would be stored as, resolved like a bookmark write
    /// but without remembering any alias. Cleaned and deduplicated.
    fn preview_tags(
        &self,
        user_id: Uuid,
        tags: Vec<String>,
    ) -> BoxFuture<'_, Result<Vec<String>, DomainError>>;
}
