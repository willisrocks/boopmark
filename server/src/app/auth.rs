use crate::domain::error::DomainError;
use crate::domain::ports::api_key_repo::{ApiKey, ApiKeyRepository};
use crate::domain::ports::session_repo::SessionRepository;
use crate::domain::ports::user_repo::UserRepository;
use crate::domain::user::{CreateUser, User};
use chrono::{Duration, Utc};
use rand::Rng;
use sha2::{Sha256, Digest};
use std::sync::Arc;
use uuid::Uuid;

pub struct AuthService<U, S, K> {
    users: Arc<U>,
    sessions: Arc<S>,
    api_keys: Arc<K>,
}

impl<U, S, K> AuthService<U, S, K>
where
    U: UserRepository + Send + Sync,
    S: SessionRepository + Send + Sync,
    K: ApiKeyRepository + Send + Sync,
{
    pub fn new(users: Arc<U>, sessions: Arc<S>, api_keys: Arc<K>) -> Self {
        Self { users, sessions, api_keys }
    }

    pub async fn upsert_user(&self, email: String, name: Option<String>, image: Option<String>) -> Result<User, DomainError> {
        self.users.upsert(CreateUser { email, name, image }).await
    }

    pub async fn create_session(&self, user_id: Uuid) -> Result<String, DomainError> {
        let token = generate_token();
        let expires_at = Utc::now() + Duration::days(30);
        self.sessions.create(user_id, &token, expires_at).await?;
        Ok(token)
    }

    pub async fn validate_session(&self, token: &str) -> Result<User, DomainError> {
        let session = self.sessions.find_by_token(token).await?
            .ok_or(DomainError::Unauthorized)?;
        self.users.find_by_id(session.user_id).await
    }

    pub async fn delete_session(&self, token: &str) -> Result<(), DomainError> {
        self.sessions.delete(token).await
    }

    pub async fn create_api_key(&self, user_id: Uuid, name: &str) -> Result<String, DomainError> {
        let raw_key = format!("boop_{}", generate_token());
        let hash = hash_api_key(&raw_key);
        self.api_keys.create(user_id, &hash, name).await?;
        Ok(raw_key)
    }

    pub async fn validate_api_key(&self, raw_key: &str) -> Result<User, DomainError> {
        let hash = hash_api_key(raw_key);
        let api_key = self.api_keys.find_by_hash(&hash).await?
            .ok_or(DomainError::Unauthorized)?;
        self.users.find_by_id(api_key.user_id).await
    }

    pub async fn list_api_keys(&self, user_id: Uuid) -> Result<Vec<ApiKey>, DomainError> {
        self.api_keys.list(user_id).await
    }

    pub async fn delete_api_key(&self, id: Uuid, user_id: Uuid) -> Result<(), DomainError> {
        self.api_keys.delete(id, user_id).await
    }
}

fn generate_token() -> String {
    use rand::distr::Alphanumeric;
    rand::rng().sample_iter(&Alphanumeric).take(32).map(char::from).collect()
}

fn hash_api_key(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    format!("{:x}", hasher.finalize())
}
