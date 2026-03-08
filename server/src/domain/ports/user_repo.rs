use crate::domain::error::DomainError;
use crate::domain::user::{CreateUser, User};
use uuid::Uuid;

#[trait_variant::make(Send)]
pub trait UserRepository: Send + Sync {
    async fn find_by_id(&self, id: Uuid) -> Result<User, DomainError>;
    async fn find_by_email(&self, email: &str) -> Result<Option<User>, DomainError>;
    async fn upsert(&self, input: CreateUser) -> Result<User, DomainError>;
}
