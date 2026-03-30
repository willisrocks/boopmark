use chrono::{Duration, Utc};
use std::sync::Arc;
use uuid::Uuid;

use crate::domain::error::DomainError;
use crate::domain::invite::{CreateInvite, Invite};
use crate::domain::ports::invite_repo::InviteRepository;

#[allow(dead_code)]
pub struct InviteService<R: InviteRepository> {
    repo: Arc<R>,
}

#[allow(dead_code)]
impl<R: InviteRepository> InviteService<R> {
    pub fn new(repo: Arc<R>) -> Self {
        Self { repo }
    }

    pub async fn create_invite(
        &self,
        created_by: Uuid,
        email: Option<String>,
    ) -> Result<Invite, DomainError> {
        let token = generate_token();
        let expires_at = Utc::now() + Duration::days(7);
        let input = CreateInvite { email, created_by };
        self.repo.create(&input, &token, expires_at).await
    }

    pub async fn validate_token(&self, token: &str) -> Result<Option<Invite>, DomainError> {
        let invite = self.repo.find_by_token(token).await?;
        Ok(invite.filter(|i| i.is_pending()))
    }

    pub async fn claim_invite(&self, token: &str, user_id: Uuid) -> Result<(), DomainError> {
        self.repo.claim(token, user_id).await
    }

    pub async fn revoke_invite(&self, invite_id: Uuid) -> Result<(), DomainError> {
        self.repo.revoke(invite_id).await
    }

    pub async fn list_invites(&self) -> Result<Vec<Invite>, DomainError> {
        self.repo.list_all().await
    }
}

#[allow(dead_code)]
fn generate_token() -> String {
    use rand::Rng;
    use rand::distr::Alphanumeric;
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(char::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::invite::{CreateInvite, Invite};
    use crate::domain::ports::invite_repo::InviteRepository;
    use chrono::{DateTime, Duration, Utc};
    use std::sync::Mutex;

    struct FakeInviteRepository {
        invites: Mutex<Vec<Invite>>,
    }

    impl FakeInviteRepository {
        fn new() -> Self {
            Self {
                invites: Mutex::new(Vec::new()),
            }
        }

        fn add_invite(&self, invite: Invite) {
            self.invites.lock().unwrap().push(invite);
        }
    }

    impl InviteRepository for FakeInviteRepository {
        async fn create(
            &self,
            invite: &CreateInvite,
            token: &str,
            expires_at: DateTime<Utc>,
        ) -> Result<Invite, DomainError> {
            let new_invite = Invite {
                id: Uuid::new_v4(),
                token: token.to_string(),
                email: invite.email.clone(),
                created_by: invite.created_by,
                claimed_by: None,
                revoked_at: None,
                expires_at,
                created_at: Utc::now(),
            };
            self.invites.lock().unwrap().push(new_invite.clone());
            Ok(new_invite)
        }

        async fn find_by_token(&self, token: &str) -> Result<Option<Invite>, DomainError> {
            Ok(self
                .invites
                .lock()
                .unwrap()
                .iter()
                .find(|i| i.token == token)
                .cloned())
        }

        async fn claim(&self, token: &str, user_id: Uuid) -> Result<(), DomainError> {
            let mut invites = self.invites.lock().unwrap();
            if let Some(inv) = invites.iter_mut().find(|i| i.token == token) {
                inv.claimed_by = Some(user_id);
                Ok(())
            } else {
                Err(DomainError::NotFound)
            }
        }

        async fn revoke(&self, invite_id: Uuid) -> Result<(), DomainError> {
            let mut invites = self.invites.lock().unwrap();
            if let Some(inv) = invites.iter_mut().find(|i| i.id == invite_id) {
                inv.revoked_at = Some(Utc::now());
                Ok(())
            } else {
                Err(DomainError::NotFound)
            }
        }

        async fn list_all(&self) -> Result<Vec<Invite>, DomainError> {
            Ok(self.invites.lock().unwrap().clone())
        }
    }

    fn build_service(repo: Arc<FakeInviteRepository>) -> InviteService<FakeInviteRepository> {
        InviteService::new(repo)
    }

    fn make_invite(token: &str, expires_at: DateTime<Utc>) -> Invite {
        Invite {
            id: Uuid::new_v4(),
            token: token.to_string(),
            email: None,
            created_by: Uuid::new_v4(),
            claimed_by: None,
            revoked_at: None,
            expires_at,
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn create_invite_generates_token_and_sets_expiry() {
        let repo = Arc::new(FakeInviteRepository::new());
        let service = build_service(repo.clone());
        let user_id = Uuid::new_v4();

        let invite = service.create_invite(user_id, None).await.unwrap();

        assert_eq!(invite.token.len(), 32);
        assert_eq!(invite.created_by, user_id);
        assert!(invite.claimed_by.is_none());
        assert!(invite.revoked_at.is_none());
        // Expiry should be roughly 7 days from now
        let diff = invite.expires_at - Utc::now();
        assert!(diff > Duration::days(6));
        assert!(diff <= Duration::days(7));
    }

    #[tokio::test]
    async fn create_invite_stores_email_if_provided() {
        let repo = Arc::new(FakeInviteRepository::new());
        let service = build_service(repo.clone());
        let user_id = Uuid::new_v4();

        let invite = service
            .create_invite(user_id, Some("bob@example.com".to_string()))
            .await
            .unwrap();

        assert_eq!(invite.email.as_deref(), Some("bob@example.com"));
    }

    #[tokio::test]
    async fn validate_token_returns_invite_for_valid_pending_token() {
        let repo = Arc::new(FakeInviteRepository::new());
        let service = build_service(repo.clone());
        let pending = make_invite("validtoken", Utc::now() + Duration::days(7));
        repo.add_invite(pending);

        let result = service.validate_token("validtoken").await.unwrap();

        assert!(result.is_some());
        assert_eq!(result.unwrap().token, "validtoken");
    }

    #[tokio::test]
    async fn validate_token_returns_none_for_expired_token() {
        let repo = Arc::new(FakeInviteRepository::new());
        let service = build_service(repo.clone());
        let expired = make_invite("expiredtoken", Utc::now() - Duration::hours(1));
        repo.add_invite(expired);

        let result = service.validate_token("expiredtoken").await.unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn validate_token_returns_none_for_claimed_token() {
        let repo = Arc::new(FakeInviteRepository::new());
        let service = build_service(repo.clone());
        let mut claimed = make_invite("claimedtoken", Utc::now() + Duration::days(7));
        claimed.claimed_by = Some(Uuid::new_v4());
        repo.add_invite(claimed);

        let result = service.validate_token("claimedtoken").await.unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn validate_token_returns_none_for_revoked_token() {
        let repo = Arc::new(FakeInviteRepository::new());
        let service = build_service(repo.clone());
        let mut revoked = make_invite("revokedtoken", Utc::now() + Duration::days(7));
        revoked.revoked_at = Some(Utc::now());
        repo.add_invite(revoked);

        let result = service.validate_token("revokedtoken").await.unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn validate_token_returns_none_for_nonexistent_token() {
        let repo = Arc::new(FakeInviteRepository::new());
        let service = build_service(repo.clone());

        let result = service.validate_token("nosuchtoken").await.unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn claim_invite_marks_invite_as_claimed() {
        let repo = Arc::new(FakeInviteRepository::new());
        let service = build_service(repo.clone());
        let invite = make_invite("claimme", Utc::now() + Duration::days(7));
        repo.add_invite(invite);

        let claimer = Uuid::new_v4();
        service.claim_invite("claimme", claimer).await.unwrap();

        let found = repo.find_by_token("claimme").await.unwrap().unwrap();
        assert_eq!(found.claimed_by, Some(claimer));
    }

    #[tokio::test]
    async fn revoke_invite_marks_invite_as_revoked() {
        let repo = Arc::new(FakeInviteRepository::new());
        let service = build_service(repo.clone());
        let invite = make_invite("revokeme", Utc::now() + Duration::days(7));
        let invite_id = invite.id;
        repo.add_invite(invite);

        service.revoke_invite(invite_id).await.unwrap();

        let found = repo.find_by_token("revokeme").await.unwrap().unwrap();
        assert!(found.revoked_at.is_some());
    }
}
