use chrono::{DateTime, Utc};
use sqlx::FromRow;
use uuid::Uuid;

#[allow(dead_code)]
#[derive(Debug, Clone, FromRow)]
pub struct Invite {
    pub id: Uuid,
    pub token: String,
    pub email: Option<String>,
    pub created_by: Uuid,
    pub claimed_by: Option<Uuid>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[allow(dead_code)]
pub struct CreateInvite {
    pub email: Option<String>,
    pub created_by: Uuid,
}

#[allow(dead_code)]
impl Invite {
    pub fn is_pending(&self) -> bool {
        self.claimed_by.is_none() && self.revoked_at.is_none() && self.expires_at > Utc::now()
    }

    pub fn status(&self) -> &'static str {
        if self.claimed_by.is_some() {
            "claimed"
        } else if self.revoked_at.is_some() {
            "revoked"
        } else if self.expires_at <= Utc::now() {
            "expired"
        } else {
            "pending"
        }
    }
}
