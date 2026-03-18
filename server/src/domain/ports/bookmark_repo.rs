use crate::domain::bookmark::{Bookmark, BookmarkFilter, CreateBookmark, UpdateBookmark};
use crate::domain::error::DomainError;
use uuid::Uuid;

#[trait_variant::make(Send)]
pub trait BookmarkRepository: Send + Sync {
    async fn create(&self, user_id: Uuid, input: CreateBookmark) -> Result<Bookmark, DomainError>;
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
    async fn delete(&self, id: Uuid, user_id: Uuid) -> Result<(), DomainError>;
    async fn all_tags(&self, user_id: Uuid) -> Result<Vec<String>, DomainError>;
    async fn tags_with_counts(&self, user_id: Uuid) -> Result<Vec<(String, i64)>, DomainError>;
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
}
