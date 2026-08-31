use chrono::{DateTime, Utc};
use uuid::Uuid;

pub struct AnthropicModelOption {
    pub label: &'static str,
    pub value: &'static str,
}

pub struct OpenAiModelOption {
    pub label: &'static str,
    pub value: &'static str,
}

pub struct ImageGenerationModelOption {
    pub label: &'static str,
    pub value: &'static str,
}

pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-haiku-4-5-20251001";
pub const DEFAULT_OPENAI_MODEL: &str = "gpt-5.6-luna";
pub const DEFAULT_IMAGE_GENERATION_MODEL: &str = "gpt-image-2";

// Claude Fable 5 is intentionally not offered for these short JSON tasks:
// its adaptive thinking is always on and cannot be disabled, so its token and
// retention behavior is not a safe fit for the low-latency bookmark path.
pub const ANTHROPIC_MODEL_OPTIONS: [AnthropicModelOption; 3] = [
    AnthropicModelOption {
        label: "Claude Opus 5",
        value: "claude-opus-5",
    },
    AnthropicModelOption {
        label: "Claude Sonnet 5",
        value: "claude-sonnet-5",
    },
    AnthropicModelOption {
        label: "Claude Haiku 4.5",
        value: "claude-haiku-4-5-20251001",
    },
];

pub const OPENAI_MODEL_OPTIONS: [OpenAiModelOption; 3] = [
    OpenAiModelOption {
        label: "GPT-5.6 Luna (efficient)",
        value: "gpt-5.6-luna",
    },
    OpenAiModelOption {
        label: "GPT-5.6 Terra (balanced)",
        value: "gpt-5.6-terra",
    },
    OpenAiModelOption {
        label: "GPT-5.6 Sol (flagship)",
        value: "gpt-5.6-sol",
    },
];

pub const IMAGE_GENERATION_MODEL_OPTIONS: [ImageGenerationModelOption; 1] =
    [ImageGenerationModelOption {
        label: "GPT Image 2",
        value: DEFAULT_IMAGE_GENERATION_MODEL,
    }];

/// Text provider used for bookmark metadata and tag operations.
///
/// This is intentionally represented as a small, stable enum at the domain
/// boundary. Persistence uses `as_str`/`from_str` so adding another provider
/// does not couple database rows to an adapter implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextProvider {
    #[default]
    Anthropic,
    OpenAi,
}

impl TextProvider {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "anthropic" => Some(Self::Anthropic),
            "openai" | "open-ai" => Some(Self::OpenAi),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub struct LlmSettings {
    pub user_id: Uuid,
    pub enabled: bool,
    pub metadata_provider: String,
    pub anthropic_api_key_encrypted: Option<Vec<u8>>,
    pub anthropic_model: String,
    pub openai_api_key_encrypted: Option<Vec<u8>>,
    pub openai_model: String,
    pub image_generation_enabled: bool,
    pub image_generation_model: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Default for LlmSettings {
    fn default() -> Self {
        Self {
            user_id: Uuid::nil(),
            enabled: false,
            metadata_provider: TextProvider::default().as_str().to_string(),
            anthropic_api_key_encrypted: None,
            anthropic_model: DEFAULT_ANTHROPIC_MODEL.to_string(),
            openai_api_key_encrypted: None,
            openai_model: DEFAULT_OPENAI_MODEL.to_string(),
            image_generation_enabled: false,
            image_generation_model: DEFAULT_IMAGE_GENERATION_MODEL.to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ANTHROPIC_MODEL_OPTIONS, DEFAULT_ANTHROPIC_MODEL, DEFAULT_IMAGE_GENERATION_MODEL,
        DEFAULT_OPENAI_MODEL, IMAGE_GENERATION_MODEL_OPTIONS, OPENAI_MODEL_OPTIONS, TextProvider,
    };

    #[test]
    fn anthropic_model_metadata_matches_current_official_allow_list() {
        assert_eq!(DEFAULT_ANTHROPIC_MODEL, "claude-haiku-4-5-20251001");
        assert_eq!(ANTHROPIC_MODEL_OPTIONS.len(), 3);
        assert_eq!(
            ANTHROPIC_MODEL_OPTIONS.map(|option| option.value),
            [
                "claude-opus-5",
                "claude-sonnet-5",
                "claude-haiku-4-5-20251001",
            ]
        );
    }

    #[test]
    fn openai_model_metadata_matches_requested_gpt_56_allow_list() {
        assert_eq!(DEFAULT_OPENAI_MODEL, "gpt-5.6-luna");
        assert_eq!(OPENAI_MODEL_OPTIONS.len(), 3);
        assert_eq!(
            OPENAI_MODEL_OPTIONS.map(|option| option.value),
            ["gpt-5.6-luna", "gpt-5.6-terra", "gpt-5.6-sol"]
        );
    }

    #[test]
    fn image_model_metadata_exposes_gpt_image_2() {
        assert_eq!(DEFAULT_IMAGE_GENERATION_MODEL, "gpt-image-2");
        assert_eq!(IMAGE_GENERATION_MODEL_OPTIONS.len(), 1);
        assert_eq!(IMAGE_GENERATION_MODEL_OPTIONS[0].value, "gpt-image-2");
    }

    #[test]
    fn text_provider_parsing_is_case_insensitive_and_anthropic_is_default() {
        assert_eq!(TextProvider::default(), TextProvider::Anthropic);
        assert_eq!(TextProvider::from_str("OPENAI"), Some(TextProvider::OpenAi));
        assert_eq!(
            TextProvider::from_str("anthropic"),
            Some(TextProvider::Anthropic)
        );
        assert_eq!(TextProvider::from_str("unknown"), None);
    }
}
