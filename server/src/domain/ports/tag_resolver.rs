use crate::domain::error::DomainError;
use crate::domain::tag_alias::TagWrite;
use std::future::Future;
use std::pin::Pin;
use uuid::Uuid;

/// The resolved tags per write, in write order.
pub type ResolveFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<Vec<String>>, DomainError>> + Send + 'a>>;

/// Resolves bookmark tag writes to the user's canonical tags.
///
/// Each [`TagWrite`] carries the submitted tags and the tags already on the
/// bookmark; the latter are returned unchanged so re-saving never rewrites
/// existing data. Results are cleaned, deduplicated and in write order. An
/// import passes all its writes in one call.
pub trait TagResolver: Send + Sync {
    fn resolve_tags(&self, user_id: Uuid, writes: Vec<TagWrite>) -> ResolveFuture<'_>;
}
