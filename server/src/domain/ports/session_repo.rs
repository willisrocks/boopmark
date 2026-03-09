use crate::domain::error::DomainError;
use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(sqlx::FromRow)]
pub struct Session {
    pub id: Uuid,
    pub user_id: Uuid,
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

#[trait_variant::make(Send)]
pub trait SessionRepository: Send + Sync {
    async fn create(&self, user_id: Uuid, token: &str, expires_at: DateTime<Utc>) -> Result<Session, DomainError>;
    async fn find_by_token(&self, token: &str) -> Result<Option<Session>, DomainError>;
    async fn delete(&self, token: &str) -> Result<(), DomainError>;
}
