use crate::domain::bookmark::{Bookmark, BookmarkFilter, CreateBookmark, UpdateBookmark};
use crate::domain::error::DomainError;
use uuid::Uuid;

#[trait_variant::make(Send)]
pub trait BookmarkRepository: Send + Sync {
    async fn create(&self, user_id: Uuid, input: CreateBookmark) -> Result<Bookmark, DomainError>;
    async fn get(&self, id: Uuid, user_id: Uuid) -> Result<Bookmark, DomainError>;
    async fn list(&self, user_id: Uuid, filter: BookmarkFilter) -> Result<Vec<Bookmark>, DomainError>;
    async fn update(&self, id: Uuid, user_id: Uuid, input: UpdateBookmark) -> Result<Bookmark, DomainError>;
    async fn delete(&self, id: Uuid, user_id: Uuid) -> Result<(), DomainError>;
}
