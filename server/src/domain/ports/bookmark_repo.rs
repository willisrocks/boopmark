use crate::domain::bookmark::{Bookmark, BookmarkFilter, CreateBookmark, UpdateBookmark};
use crate::domain::error::DomainError;
use uuid::Uuid;

/// The client-visible operation identity and the reviewed create payload it
/// represents.  The fingerprint is calculated before any server-side
/// metadata/enrichment is applied, so a transport replay can be compared with
/// the exact values the user submitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateIdempotency {
    pub key: Uuid,
    pub fingerprint_version: i16,
    pub fingerprint: String,
}

#[derive(Debug, Clone)]
pub enum CreateIdempotencyClaim {
    Acquired,
    Pending,
    Completed(Box<Bookmark>),
    Conflict,
}

#[trait_variant::make(Send)]
pub trait BookmarkRepository: Send + Sync {
    async fn create(&self, user_id: Uuid, input: CreateBookmark) -> Result<Bookmark, DomainError>;

    /// Atomically claim an operation identity before any enrichment or other
    /// create side effect is attempted.
    async fn claim_create(
        &self,
        user_id: Uuid,
        operation: CreateIdempotency,
    ) -> Result<CreateIdempotencyClaim, DomainError>;

    /// Insert the bookmark and finalize a previously acquired operation in
    /// one database transaction.
    async fn create_claimed(
        &self,
        user_id: Uuid,
        input: CreateBookmark,
        operation: CreateIdempotency,
    ) -> Result<Bookmark, DomainError>;
    async fn get(&self, id: Uuid, user_id: Uuid) -> Result<Bookmark, DomainError>;
    async fn list(
        &self,
        user_id: Uuid,
        filter: BookmarkFilter,
    ) -> Result<Vec<Bookmark>, DomainError>;
    async fn update(
        &self,
        id: Uuid,
        user_id: Uuid,
        input: UpdateBookmark,
    ) -> Result<Bookmark, DomainError>;
    #[allow(dead_code)]
    async fn delete(&self, id: Uuid, user_id: Uuid) -> Result<(), DomainError>;
    async fn export_all(&self, user_id: Uuid) -> Result<Vec<Bookmark>, DomainError>;
    async fn find_by_url(&self, user_id: Uuid, url: &str) -> Result<Option<Bookmark>, DomainError>;
    async fn insert_with_id(&self, bookmark: Bookmark) -> Result<Bookmark, DomainError>;
    async fn upsert_full(&self, bookmark: Bookmark) -> Result<Bookmark, DomainError>;
    async fn update_image_url(
        &self,
        id: Uuid,
        user_id: Uuid,
        image_url: &str,
    ) -> Result<(), DomainError>;
    /// Atomically replace the optional user override and return the previous
    /// override URL, if any, for safe object cleanup.
    async fn replace_override_image_url(
        &self,
        id: Uuid,
        user_id: Uuid,
        image_url: Option<&str>,
    ) -> Result<Option<String>, DomainError>;
    /// Delete a bookmark while returning its owned override URL, if any.
    async fn delete_with_override(
        &self,
        id: Uuid,
        user_id: Uuid,
    ) -> Result<Option<String>, DomainError>;
}
