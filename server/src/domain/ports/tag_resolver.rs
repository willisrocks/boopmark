use crate::domain::error::DomainError;
use std::future::Future;
use std::pin::Pin;
use uuid::Uuid;

/// Resolves the tags of one bookmark write to the user's canonical tags.
///
/// `tags` are the tags as submitted. `keep` are tags already on the bookmark
/// (on an edit); they are returned unchanged so re-saving never rewrites
/// existing data. The result is cleaned and deduplicated.
pub trait TagResolver: Send + Sync {
    fn resolve_tags(
        &self,
        user_id: Uuid,
        tags: Vec<String>,
        keep: Vec<String>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, DomainError>> + Send + '_>>;
}
