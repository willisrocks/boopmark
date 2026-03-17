use chrono::{DateTime, Utc};
use uuid::Uuid;

pub struct AnthropicModelOption {
    pub label: &'static str,
    pub value: &'static str,
}

pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-haiku-4-5-20251001";
pub const ANTHROPIC_MODEL_OPTIONS: [AnthropicModelOption; 3] = [
    AnthropicModelOption {
        label: "Claude Opus 4.6",
        value: "claude-opus-4-6",
    },
    AnthropicModelOption {
        label: "Claude Sonnet 4.6",
        value: "claude-sonnet-4-6",
    },
    AnthropicModelOption {
        label: "Claude Haiku 4.5",
        value: "claude-haiku-4-5-20251001",
    },
];

#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub struct LlmSettings {
    pub user_id: Uuid,
    pub enabled: bool,
    pub anthropic_api_key_encrypted: Option<Vec<u8>>,
    pub anthropic_model: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::{ANTHROPIC_MODEL_OPTIONS, DEFAULT_ANTHROPIC_MODEL};

    #[test]
    fn anthropic_model_metadata_matches_current_official_allow_list() {
        assert_eq!(DEFAULT_ANTHROPIC_MODEL, "claude-haiku-4-5-20251001");
        assert_eq!(ANTHROPIC_MODEL_OPTIONS.len(), 3);
        assert_eq!(
            ANTHROPIC_MODEL_OPTIONS.map(|option| option.value),
            [
                "claude-opus-4-6",
                "claude-sonnet-4-6",
                "claude-haiku-4-5-20251001",
            ]
        );
    }
}
