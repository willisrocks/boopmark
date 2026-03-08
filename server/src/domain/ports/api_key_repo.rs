use crate::domain::error::DomainError;
use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(sqlx::FromRow)]
pub struct ApiKey {
    pub id: Uuid,
    pub user_id: Uuid,
    pub key_hash: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[trait_variant::make(Send)]
pub trait ApiKeyRepository: Send + Sync {
    async fn create(&self, user_id: Uuid, key_hash: &str, name: &str) -> Result<ApiKey, DomainError>;
    async fn list(&self, user_id: Uuid) -> Result<Vec<ApiKey>, DomainError>;
    async fn find_by_hash(&self, key_hash: &str) -> Result<Option<ApiKey>, DomainError>;
    async fn delete(&self, id: Uuid, user_id: Uuid) -> Result<(), DomainError>;
}
