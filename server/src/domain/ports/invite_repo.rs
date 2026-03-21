use uuid::Uuid;

use crate::domain::error::DomainError;
use crate::domain::invite::{CreateInvite, Invite};

#[allow(dead_code)]
#[trait_variant::make(Send)]
pub trait InviteRepository: Send + Sync {
    async fn create(
        &self,
        invite: &CreateInvite,
        token: &str,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Invite, DomainError>;
    async fn find_by_token(&self, token: &str) -> Result<Option<Invite>, DomainError>;
    async fn claim(&self, token: &str, user_id: Uuid) -> Result<(), DomainError>;
    async fn revoke(&self, invite_id: Uuid) -> Result<(), DomainError>;
    async fn list_all(&self) -> Result<Vec<Invite>, DomainError>;
}
