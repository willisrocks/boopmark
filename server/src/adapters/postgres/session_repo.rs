use super::PostgresPool;
use crate::domain::error::DomainError;
use crate::domain::ports::session_repo::{Session, SessionRepository};
use chrono::{DateTime, Utc};
use uuid::Uuid;

impl SessionRepository for PostgresPool {
    async fn create(
        &self,
        user_id: Uuid,
        token: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<Session, DomainError> {
        sqlx::query_as::<_, Session>(
            "INSERT INTO sessions (user_id, token, expires_at) VALUES ($1, $2, $3)
             RETURNING id, user_id, token, expires_at",
        )
        .bind(user_id)
        .bind(token)
        .bind(expires_at)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))
    }

    async fn find_by_token(&self, token: &str) -> Result<Option<Session>, DomainError> {
        sqlx::query_as::<_, Session>(
            "SELECT id, user_id, token, expires_at FROM sessions WHERE token = $1 AND expires_at > now()",
        )
        .bind(token)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))
    }

    async fn delete(&self, token: &str) -> Result<(), DomainError> {
        sqlx::query("DELETE FROM sessions WHERE token = $1")
            .bind(token)
            .execute(&self.pool)
            .await
            .map_err(|e| DomainError::Internal(e.to_string()))?;
        Ok(())
    }
}
