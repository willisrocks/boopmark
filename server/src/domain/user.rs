use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Type;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Type, Serialize, Deserialize)]
#[sqlx(type_name = "user_role", rename_all = "lowercase")]
pub enum UserRole {
    Owner,
    Admin,
    User,
}

#[allow(dead_code)]
impl UserRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            UserRole::Owner => "owner",
            UserRole::Admin => "admin",
            UserRole::User => "user",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub name: Option<String>,
    pub image: Option<String>,
    #[serde(skip_serializing)]
    pub password_hash: Option<String>,
    pub role: UserRole,
    pub deactivated_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[allow(dead_code)]
impl User {
    pub fn is_admin_or_owner(&self) -> bool {
        matches!(self.role, UserRole::Owner | UserRole::Admin)
    }

    pub fn is_owner(&self) -> bool {
        matches!(self.role, UserRole::Owner)
    }

    pub fn is_active(&self) -> bool {
        self.deactivated_at.is_none()
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateUser {
    pub email: String,
    pub name: Option<String>,
    pub image: Option<String>,
}
