use crate::domain::error::DomainError;
use crate::domain::ports::api_key_repo::{ApiKey, ApiKeyRepository};
use super::PostgresPool;
use uuid::Uuid;

impl ApiKeyRepository for PostgresPool {
    async fn create(&self, user_id: Uuid, key_hash: &str, name: &str) -> Result<ApiKey, DomainError> {
        sqlx::query_as::<_, ApiKey>(
            "INSERT INTO api_keys (user_id, key_hash, name) VALUES ($1, $2, $3)
             RETURNING id, user_id, key_hash, name, created_at",
        )
        .bind(user_id)
        .bind(key_hash)
        .bind(name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))
    }

    async fn list(&self, user_id: Uuid) -> Result<Vec<ApiKey>, DomainError> {
        sqlx::query_as::<_, ApiKey>(
            "SELECT id, user_id, key_hash, name, created_at FROM api_keys WHERE user_id = $1 ORDER BY created_at DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))
    }

    async fn find_by_hash(&self, key_hash: &str) -> Result<Option<ApiKey>, DomainError> {
        sqlx::query_as::<_, ApiKey>(
            "SELECT id, user_id, key_hash, name, created_at FROM api_keys WHERE key_hash = $1",
        )
        .bind(key_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))
    }

    async fn delete(&self, id: Uuid, user_id: Uuid) -> Result<(), DomainError> {
        sqlx::query("DELETE FROM api_keys WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_err(|e| DomainError::Internal(e.to_string()))?;
        Ok(())
    }
}
