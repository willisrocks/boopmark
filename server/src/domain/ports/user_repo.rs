use crate::domain::error::DomainError;
use crate::domain::user::{CreateUser, User, UserRole};
use uuid::Uuid;

#[allow(dead_code)]
#[trait_variant::make(Send)]
pub trait UserRepository: Send + Sync {
    async fn find_by_id(&self, id: Uuid) -> Result<User, DomainError>;
    async fn find_by_email(&self, email: &str) -> Result<Option<User>, DomainError>;
    async fn upsert(&self, input: CreateUser) -> Result<User, DomainError>;
    async fn list_all(&self) -> Result<Vec<User>, DomainError>;
    async fn update_role(&self, user_id: Uuid, role: UserRole) -> Result<(), DomainError>;
    async fn deactivate(&self, user_id: Uuid) -> Result<(), DomainError>;
}
