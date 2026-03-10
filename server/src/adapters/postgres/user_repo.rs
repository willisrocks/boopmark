use super::PostgresPool;
use crate::domain::error::DomainError;
use crate::domain::ports::user_repo::UserRepository;
use crate::domain::user::{CreateUser, User};
use uuid::Uuid;

impl UserRepository for PostgresPool {
    async fn find_by_id(&self, id: Uuid) -> Result<User, DomainError> {
        sqlx::query_as::<_, User>(
            "SELECT id, email, name, image, password_hash, created_at FROM users WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?
        .ok_or(DomainError::NotFound)
    }

    async fn find_by_email(&self, email: &str) -> Result<Option<User>, DomainError> {
        sqlx::query_as::<_, User>(
            "SELECT id, email, name, image, password_hash, created_at FROM users WHERE email = $1",
        )
        .bind(email)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))
    }

    async fn upsert(&self, input: CreateUser) -> Result<User, DomainError> {
        sqlx::query_as::<_, User>(
            "INSERT INTO users (email, name, image) VALUES ($1, $2, $3)
             ON CONFLICT (email) DO UPDATE SET name = COALESCE($2, users.name), image = COALESCE($3, users.image)
             RETURNING id, email, name, image, password_hash, created_at",
        )
        .bind(&input.email)
        .bind(&input.name)
        .bind(&input.image)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))
    }

    async fn upsert_with_password(
        &self,
        email: &str,
        name: Option<&str>,
        password_hash: &str,
    ) -> Result<User, DomainError> {
        sqlx::query_as::<_, User>(
            "INSERT INTO users (email, name, password_hash) VALUES ($1, $2, $3)
             ON CONFLICT (email) DO UPDATE SET
               name = COALESCE($2, users.name),
               password_hash = $3
             RETURNING id, email, name, image, password_hash, created_at",
        )
        .bind(email)
        .bind(name)
        .bind(password_hash)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))
    }
}
