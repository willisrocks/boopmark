use chrono::{DateTime, Utc};
use uuid::Uuid;

pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-haiku-4-5";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LlmSettings {
    pub user_id: Uuid,
    pub enabled: bool,
    pub anthropic_api_key_encrypted: Option<Vec<u8>>,
    pub anthropic_model: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
