use chrono::{DateTime, Utc};
use uuid::Uuid;

pub struct AnthropicModelOption {
    pub label: &'static str,
    pub value: &'static str,
}

pub struct ImageGenerationModelOption {
    pub label: &'static str,
    pub value: &'static str,
}

pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-haiku-4-5-20251001";
pub const DEFAULT_IMAGE_GENERATION_MODEL: &str = "gemini-3.1-flash-lite-image";
pub const DEFAULT_IMAGE_ART_STYLE: &str = "Bold editorial illustration with clean geometric shapes, a limited vibrant color palette, subtle depth, and one clear visual focal point. Modern, simple, playful, and polished.";
pub const IMAGE_GENERATION_MODEL_OPTIONS: [ImageGenerationModelOption; 1] =
    [ImageGenerationModelOption {
        label: "Nano Banana 2 Lite (Gemini 3.1 Flash Lite Image)",
        value: DEFAULT_IMAGE_GENERATION_MODEL,
    }];
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
    pub image_generation_enabled: bool,
    pub gemini_api_key_encrypted: Option<Vec<u8>>,
    pub image_generation_model: String,
    pub image_generation_art_style: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Default for LlmSettings {
    fn default() -> Self {
        Self {
            user_id: Uuid::nil(),
            enabled: false,
            anthropic_api_key_encrypted: None,
            anthropic_model: DEFAULT_ANTHROPIC_MODEL.to_string(),
            image_generation_enabled: false,
            gemini_api_key_encrypted: None,
            image_generation_model: DEFAULT_IMAGE_GENERATION_MODEL.to_string(),
            image_generation_art_style: DEFAULT_IMAGE_ART_STYLE.to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ANTHROPIC_MODEL_OPTIONS, DEFAULT_ANTHROPIC_MODEL, DEFAULT_IMAGE_ART_STYLE,
        DEFAULT_IMAGE_GENERATION_MODEL, IMAGE_GENERATION_MODEL_OPTIONS,
    };

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

    #[test]
    fn image_generation_metadata_exposes_only_nano_banana_2_lite() {
        assert_eq!(IMAGE_GENERATION_MODEL_OPTIONS.len(), 1);
        assert_eq!(
            IMAGE_GENERATION_MODEL_OPTIONS[0].value,
            DEFAULT_IMAGE_GENERATION_MODEL
        );
        assert_eq!(
            IMAGE_GENERATION_MODEL_OPTIONS[0].label,
            "Nano Banana 2 Lite (Gemini 3.1 Flash Lite Image)"
        );
        assert!(!DEFAULT_IMAGE_ART_STYLE.trim().is_empty());
    }
}
