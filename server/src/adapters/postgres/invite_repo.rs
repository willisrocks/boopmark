use super::PostgresPool;
use crate::domain::error::DomainError;
use crate::domain::invite::{CreateInvite, Invite};
use crate::domain::ports::invite_repo::InviteRepository;
use chrono::{DateTime, Utc};
use uuid::Uuid;

impl InviteRepository for PostgresPool {
    async fn create(
        &self,
        invite: &CreateInvite,
        token: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<Invite, DomainError> {
        sqlx::query_as::<_, Invite>(
            "INSERT INTO invites (token, email, created_by, expires_at)
             VALUES ($1, $2, $3, $4)
             RETURNING id, token, email, created_by, claimed_by, revoked_at, expires_at, created_at",
        )
        .bind(token)
        .bind(&invite.email)
        .bind(invite.created_by)
        .bind(expires_at)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))
    }

    async fn find_by_token(&self, token: &str) -> Result<Option<Invite>, DomainError> {
        sqlx::query_as::<_, Invite>(
            "SELECT id, token, email, created_by, claimed_by, revoked_at, expires_at, created_at
             FROM invites WHERE token = $1",
        )
        .bind(token)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))
    }

    async fn claim(&self, token: &str, user_id: Uuid) -> Result<(), DomainError> {
        sqlx::query("UPDATE invites SET claimed_by = $1 WHERE token = $2")
            .bind(user_id)
            .bind(token)
            .execute(&self.pool)
            .await
            .map_err(|e| DomainError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn revoke(&self, invite_id: Uuid) -> Result<(), DomainError> {
        sqlx::query("UPDATE invites SET revoked_at = now() WHERE id = $1")
            .bind(invite_id)
            .execute(&self.pool)
            .await
            .map_err(|e| DomainError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn list_all(&self) -> Result<Vec<Invite>, DomainError> {
        sqlx::query_as::<_, Invite>(
            "SELECT id, token, email, created_by, claimed_by, revoked_at, expires_at, created_at
             FROM invites ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))
    }
}
