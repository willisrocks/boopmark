use crate::domain::error::DomainError;
use crate::domain::ports::api_key_repo::{ApiKey, ApiKeyRepository};
use crate::domain::ports::session_repo::SessionRepository;
use crate::domain::ports::user_repo::UserRepository;
use crate::domain::user::{CreateUser, User};
use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use chrono::{Duration, Utc};
use rand::Rng;
use sha2::{Digest, Sha256};
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
        Self {
            users,
            sessions,
            api_keys,
        }
    }

    pub async fn find_user_by_email(&self, email: &str) -> Result<Option<User>, DomainError> {
        self.users.find_by_email(email).await
    }

    pub async fn local_login(
        &self,
        email: &str,
        password: &str,
    ) -> Result<(User, String), DomainError> {
        let user = self.users.find_by_email(email).await?;

        // When the user is not found or has no password_hash, perform a dummy
        // argon2 hash so the response time is indistinguishable from a real
        // verification. This prevents timing-based user enumeration.
        let user = match user {
            Some(u) => u,
            None => {
                dummy_argon2_verify(password);
                return Err(DomainError::Unauthorized);
            }
        };

        let hash_str = match user.password_hash.clone() {
            Some(h) => h,
            None => {
                dummy_argon2_verify(password);
                return Err(DomainError::Unauthorized);
            }
        };

        let parsed_hash = PasswordHash::new(&hash_str)
            .map_err(|_| DomainError::Internal("invalid hash".into()))?;

        Argon2::default()
            .verify_password(password.as_bytes(), &parsed_hash)
            .map_err(|_| DomainError::Unauthorized)?;

        if !user.is_active() {
            return Err(DomainError::Unauthorized);
        }

        let token = self.create_session(user.id).await?;
        Ok((user, token))
    }

    pub async fn upsert_user(
        &self,
        email: String,
        name: Option<String>,
        image: Option<String>,
    ) -> Result<User, DomainError> {
        self.users.upsert(CreateUser { email, name, image }).await
    }

    pub async fn create_session(&self, user_id: Uuid) -> Result<String, DomainError> {
        let token = generate_token();
        let expires_at = Utc::now() + Duration::days(30);
        self.sessions.create(user_id, &token, expires_at).await?;
        Ok(token)
    }

    pub async fn validate_session(&self, token: &str) -> Result<User, DomainError> {
        let session = self
            .sessions
            .find_by_token(token)
            .await?
            .ok_or(DomainError::Unauthorized)?;
        let user = self.users.find_by_id(session.user_id).await?;
        if !user.is_active() {
            return Err(DomainError::Unauthorized);
        }
        Ok(user)
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
        let api_key = self
            .api_keys
            .find_by_hash(&hash)
            .await?
            .ok_or(DomainError::Unauthorized)?;
        let user = self.users.find_by_id(api_key.user_id).await?;
        if !user.is_active() {
            return Err(DomainError::Unauthorized);
        }
        Ok(user)
    }

    pub async fn list_api_keys(&self, user_id: Uuid) -> Result<Vec<ApiKey>, DomainError> {
        self.api_keys.list(user_id).await
    }

    pub async fn delete_api_key(&self, id: Uuid, user_id: Uuid) -> Result<(), DomainError> {
        self.api_keys.delete(id, user_id).await
    }

    #[allow(dead_code)]
    pub async fn list_users(&self) -> Result<Vec<User>, DomainError> {
        self.users.list_all().await
    }

    #[allow(dead_code)]
    pub async fn find_user_by_id(&self, id: Uuid) -> Result<User, DomainError> {
        self.users.find_by_id(id).await
    }

    #[allow(dead_code)]
    pub async fn update_user_role(
        &self,
        user_id: Uuid,
        role: crate::domain::user::UserRole,
    ) -> Result<(), DomainError> {
        self.users.update_role(user_id, role).await
    }

    #[allow(dead_code)]
    pub async fn deactivate_user(&self, user_id: Uuid) -> Result<(), DomainError> {
        self.users.deactivate(user_id).await
    }
}

fn generate_token() -> String {
    use rand::distr::Alphanumeric;
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(char::from)
        .collect()
}

fn hash_api_key(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Perform a dummy argon2 hash to equalize timing when a user is not found.
fn dummy_argon2_verify(password: &str) {
    let salt = SaltString::generate(&mut OsRng);
    let _ = Argon2::default().hash_password(password.as_bytes(), &salt);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ports::api_key_repo::{ApiKey, ApiKeyRepository};
    use crate::domain::ports::session_repo::{Session, SessionRepository};
    use crate::domain::ports::user_repo::UserRepository;
    use crate::domain::user::{CreateUser, User, UserRole};
    use chrono::{DateTime, Utc};
    use std::sync::Mutex;

    /// Hash a password with argon2 for test fixtures.
    fn hash_password(password: &str) -> String {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .expect("hash_password")
            .to_string()
    }

    struct FakeUserRepo {
        users: Mutex<Vec<User>>,
    }

    impl FakeUserRepo {
        fn new() -> Self {
            Self {
                users: Mutex::new(Vec::new()),
            }
        }

        fn add_user(&self, email: &str, password_hash: Option<String>) -> Uuid {
            self.add_user_ex(email, password_hash, None)
        }

        fn add_user_ex(
            &self,
            email: &str,
            password_hash: Option<String>,
            deactivated_at: Option<DateTime<Utc>>,
        ) -> Uuid {
            let id = Uuid::new_v4();
            let mut users = self.users.lock().unwrap();
            users.push(User {
                id,
                email: email.to_string(),
                name: Some(email.to_string()),
                image: None,
                password_hash,
                role: UserRole::User,
                deactivated_at,
                created_at: Utc::now(),
            });
            id
        }
    }

    impl UserRepository for FakeUserRepo {
        async fn find_by_id(&self, id: Uuid) -> Result<User, DomainError> {
            self.users
                .lock()
                .unwrap()
                .iter()
                .find(|u| u.id == id)
                .cloned()
                .ok_or(DomainError::NotFound)
        }

        async fn find_by_email(&self, email: &str) -> Result<Option<User>, DomainError> {
            Ok(self
                .users
                .lock()
                .unwrap()
                .iter()
                .find(|u| u.email == email)
                .cloned())
        }

        async fn upsert(&self, input: CreateUser) -> Result<User, DomainError> {
            let user = User {
                id: Uuid::new_v4(),
                email: input.email,
                name: input.name,
                image: input.image,
                password_hash: None,
                role: UserRole::User,
                deactivated_at: None,
                created_at: Utc::now(),
            };
            self.users.lock().unwrap().push(user.clone());
            Ok(user)
        }

        async fn list_all(&self) -> Result<Vec<User>, DomainError> {
            Ok(self.users.lock().unwrap().clone())
        }

        async fn update_role(&self, user_id: Uuid, role: UserRole) -> Result<(), DomainError> {
            let mut users = self.users.lock().unwrap();
            if let Some(u) = users.iter_mut().find(|u| u.id == user_id) {
                u.role = role;
                Ok(())
            } else {
                Err(DomainError::NotFound)
            }
        }

        async fn deactivate(&self, user_id: Uuid) -> Result<(), DomainError> {
            let mut users = self.users.lock().unwrap();
            if let Some(u) = users.iter_mut().find(|u| u.id == user_id) {
                u.deactivated_at = Some(Utc::now());
                Ok(())
            } else {
                Err(DomainError::NotFound)
            }
        }
    }

    /// Stores (id, user_id, token, expires_at) tuples for session tracking.
    struct FakeSessionRepo {
        #[allow(clippy::type_complexity)]
        sessions: Mutex<Vec<(Uuid, Uuid, String, DateTime<Utc>)>>,
    }

    impl FakeSessionRepo {
        fn new() -> Self {
            Self {
                sessions: Mutex::new(Vec::new()),
            }
        }
    }

    impl SessionRepository for FakeSessionRepo {
        async fn create(
            &self,
            user_id: Uuid,
            token: &str,
            expires_at: DateTime<Utc>,
        ) -> Result<Session, DomainError> {
            let id = Uuid::new_v4();
            self.sessions
                .lock()
                .unwrap()
                .push((id, user_id, token.to_string(), expires_at));
            Ok(Session {
                id,
                user_id,
                token: token.to_string(),
                expires_at,
            })
        }

        async fn find_by_token(&self, token: &str) -> Result<Option<Session>, DomainError> {
            Ok(self
                .sessions
                .lock()
                .unwrap()
                .iter()
                .find(|s| s.2 == token)
                .map(|(id, user_id, tok, exp)| Session {
                    id: *id,
                    user_id: *user_id,
                    token: tok.clone(),
                    expires_at: *exp,
                }))
        }

        async fn delete(&self, _token: &str) -> Result<(), DomainError> {
            Ok(())
        }
    }

    struct FakeApiKeyRepo {
        keys: Mutex<Vec<ApiKey>>,
    }

    impl FakeApiKeyRepo {
        fn new() -> Self {
            Self {
                keys: Mutex::new(Vec::new()),
            }
        }

        fn add_key(&self, user_id: Uuid, key_hash: &str) {
            self.keys.lock().unwrap().push(ApiKey {
                id: Uuid::new_v4(),
                user_id,
                key_hash: key_hash.to_string(),
                name: "test".to_string(),
                created_at: Utc::now(),
            });
        }
    }

    impl ApiKeyRepository for FakeApiKeyRepo {
        async fn create(
            &self,
            _user_id: Uuid,
            _key_hash: &str,
            _name: &str,
        ) -> Result<ApiKey, DomainError> {
            unimplemented!()
        }

        async fn list(&self, _user_id: Uuid) -> Result<Vec<ApiKey>, DomainError> {
            unimplemented!()
        }

        async fn find_by_hash(&self, key_hash: &str) -> Result<Option<ApiKey>, DomainError> {
            Ok(self
                .keys
                .lock()
                .unwrap()
                .iter()
                .find(|k| k.key_hash == key_hash)
                .cloned())
        }

        async fn delete(&self, _id: Uuid, _user_id: Uuid) -> Result<(), DomainError> {
            unimplemented!()
        }
    }

    fn build_service(
        user_repo: Arc<FakeUserRepo>,
    ) -> AuthService<FakeUserRepo, FakeSessionRepo, FakeApiKeyRepo> {
        build_service_full(
            user_repo,
            Arc::new(FakeSessionRepo::new()),
            Arc::new(FakeApiKeyRepo::new()),
        )
    }

    fn build_service_full(
        user_repo: Arc<FakeUserRepo>,
        session_repo: Arc<FakeSessionRepo>,
        api_key_repo: Arc<FakeApiKeyRepo>,
    ) -> AuthService<FakeUserRepo, FakeSessionRepo, FakeApiKeyRepo> {
        AuthService::new(user_repo, session_repo, api_key_repo)
    }

    #[tokio::test]
    async fn local_login_succeeds_with_correct_credentials() {
        let user_repo = Arc::new(FakeUserRepo::new());
        let hashed = hash_password("correctpass");
        user_repo.add_user("alice@example.com", Some(hashed));
        let service = build_service(user_repo);

        let result = service
            .local_login("alice@example.com", "correctpass")
            .await;
        assert!(result.is_ok());
        let (user, token) = result.unwrap();
        assert_eq!(user.email, "alice@example.com");
        assert!(!token.is_empty());
    }

    #[tokio::test]
    async fn local_login_fails_with_wrong_password() {
        let user_repo = Arc::new(FakeUserRepo::new());
        let hashed = hash_password("correctpass");
        user_repo.add_user("alice@example.com", Some(hashed));
        let service = build_service(user_repo);

        let result = service.local_login("alice@example.com", "wrongpass").await;
        assert!(matches!(result, Err(DomainError::Unauthorized)));
    }

    #[tokio::test]
    async fn local_login_fails_for_nonexistent_user() {
        let user_repo = Arc::new(FakeUserRepo::new());
        let service = build_service(user_repo);

        let result = service.local_login("nobody@example.com", "anypass").await;
        assert!(matches!(result, Err(DomainError::Unauthorized)));
    }

    #[tokio::test]
    async fn local_login_fails_when_user_has_no_password_hash() {
        let user_repo = Arc::new(FakeUserRepo::new());
        user_repo.add_user("oauth-only@example.com", None);
        let service = build_service(user_repo);

        let result = service
            .local_login("oauth-only@example.com", "anypass")
            .await;
        assert!(matches!(result, Err(DomainError::Unauthorized)));
    }

    #[tokio::test]
    async fn local_login_fails_for_deactivated_user() {
        let user_repo = Arc::new(FakeUserRepo::new());
        let hashed = hash_password("correctpass");
        user_repo.add_user_ex("deactivated@example.com", Some(hashed), Some(Utc::now()));
        let service = build_service(user_repo);

        let result = service
            .local_login("deactivated@example.com", "correctpass")
            .await;
        assert!(matches!(result, Err(DomainError::Unauthorized)));
    }

    #[tokio::test]
    async fn validate_session_fails_for_deactivated_user() {
        let user_repo = Arc::new(FakeUserRepo::new());
        let session_repo = Arc::new(FakeSessionRepo::new());
        let user_id = user_repo.add_user_ex("deactivated@example.com", None, Some(Utc::now()));
        let service = build_service_full(
            user_repo,
            session_repo.clone(),
            Arc::new(FakeApiKeyRepo::new()),
        );

        let token = service.create_session(user_id).await.unwrap();
        let result = service.validate_session(&token).await;
        assert!(matches!(result, Err(DomainError::Unauthorized)));
    }

    #[tokio::test]
    async fn validate_api_key_fails_for_deactivated_user() {
        let user_repo = Arc::new(FakeUserRepo::new());
        let api_key_repo = Arc::new(FakeApiKeyRepo::new());
        let user_id = user_repo.add_user_ex("deactivated@example.com", None, Some(Utc::now()));

        let raw_key = "boop_testkey123";
        let hashed_key = hash_api_key(raw_key);
        api_key_repo.add_key(user_id, &hashed_key);

        let service = build_service_full(user_repo, Arc::new(FakeSessionRepo::new()), api_key_repo);

        let result = service.validate_api_key(raw_key).await;
        assert!(matches!(result, Err(DomainError::Unauthorized)));
    }
}
