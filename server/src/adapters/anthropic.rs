use crate::domain::error::DomainError;
use crate::domain::ports::llm_enricher::{EnrichmentInput, EnrichmentOutput, LlmEnricher};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;

#[derive(Clone)]
pub struct AnthropicEnricher {
    client: reqwest::Client,
}

impl AnthropicEnricher {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    fn build_prompt(input: &EnrichmentInput) -> String {
        let existing_tags_instruction = match &input.existing_tags {
            Some(tags) if !tags.is_empty() => {
                let tag_list: Vec<String> =
                    tags.iter().map(|(t, c)| format!("{t} ({c})")).collect();
                format!(
                    "\n\nThe user already has these tags (listed most-popular first): {}. \
                     Prefer reusing these existing tags. Only create new tags if none of these fit.",
                    tag_list.join(", ")
                )
            }
            _ => String::new(),
        };

        format!(
            "You are a bookmark organizer. Given a URL and its scraped metadata, suggest:\n\
             1. A concise, clear title (improve the scraped title if present)\n\
             2. A brief, useful description (1-2 sentences, improve the scraped description if present)\n\
             3. 3-5 relevant tags for categorization{existing_tags_instruction}\n\n\
             URL: {}\n\
             Scraped title: {}\n\
             Scraped description: {}\n\n\
             Respond with ONLY valid JSON in this exact format, no other text:\n\
             {{\"title\": \"...\", \"description\": \"...\", \"tags\": [\"tag1\", \"tag2\", \"tag3\"]}}",
            input.url,
            input.scraped_title.as_deref().unwrap_or("(none)"),
            input.scraped_description.as_deref().unwrap_or("(none)"),
        )
    }
}

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<Message>,
}

#[derive(Serialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    text: Option<String>,
}

#[derive(Deserialize)]
struct EnrichmentJson {
    title: Option<String>,
    description: Option<String>,
    tags: Option<Vec<String>>,
}

/// Extract the first JSON object from a text response by finding the first `{` and last `}`.
/// Handles markdown fences, leading text, or other noise the LLM may wrap around the JSON.
fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end >= start {
        Some(&text[start..=end])
    } else {
        None
    }
}

impl LlmEnricher for AnthropicEnricher {
    fn enrich(
        &self,
        api_key: &str,
        model: &str,
        input: EnrichmentInput,
    ) -> Pin<Box<dyn Future<Output = Result<EnrichmentOutput, DomainError>> + Send + '_>> {
        let api_key = api_key.to_string();
        let model = model.to_string();
        Box::pin(async move { self.do_enrich(&api_key, &model, input).await })
    }
}

impl AnthropicEnricher {
    async fn do_enrich(
        &self,
        api_key: &str,
        model: &str,
        input: EnrichmentInput,
    ) -> Result<EnrichmentOutput, DomainError> {
        let prompt = Self::build_prompt(&input);

        let request_body = AnthropicRequest {
            model: model.to_string(),
            max_tokens: 512,
            messages: vec![Message {
                role: "user".to_string(),
                content: prompt,
            }],
        };

        let resp = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| DomainError::Internal(format!("Anthropic API error: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(DomainError::Internal(format!(
                "Anthropic API returned HTTP {status}: {body}"
            )));
        }

        let api_resp: AnthropicResponse = resp
            .json()
            .await
            .map_err(|e| DomainError::Internal(format!("Anthropic response parse error: {e}")))?;

        let text = api_resp
            .content
            .into_iter()
            .find_map(|block| block.text)
            .ok_or_else(|| DomainError::Internal("Anthropic response had no text".to_string()))?;

        // Extract the JSON object from the response, handling markdown fences
        // or any leading/trailing text the LLM may have added
        let json_str = extract_json_object(&text).ok_or_else(|| {
            DomainError::Internal("LLM response contained no JSON object".to_string())
        })?;

        let parsed: EnrichmentJson = serde_json::from_str(json_str)
            .map_err(|e| DomainError::Internal(format!("LLM JSON parse error: {e}")))?;

        Ok(EnrichmentOutput {
            title: parsed.title,
            description: parsed.description,
            tags: parsed.tags.unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ports::llm_enricher::EnrichmentInput;

    #[test]
    fn build_prompt_includes_url_and_scraped_metadata() {
        let input = EnrichmentInput {
            url: "https://example.com".to_string(),
            scraped_title: Some("Example Title".to_string()),
            scraped_description: Some("Example description".to_string()),
            existing_tags: None,
        };
        let prompt = AnthropicEnricher::build_prompt(&input);
        assert!(prompt.contains("https://example.com"));
        assert!(prompt.contains("Example Title"));
        assert!(prompt.contains("Example description"));
    }

    #[test]
    fn build_prompt_handles_missing_metadata() {
        let input = EnrichmentInput {
            url: "https://example.com".to_string(),
            scraped_title: None,
            scraped_description: None,
            existing_tags: None,
        };
        let prompt = AnthropicEnricher::build_prompt(&input);
        assert!(prompt.contains("(none)"));
    }

    #[test]
    fn build_prompt_includes_existing_tags_when_present() {
        let input = EnrichmentInput {
            url: "https://example.com".to_string(),
            scraped_title: Some("Example".to_string()),
            scraped_description: None,
            existing_tags: Some(vec![("rust".to_string(), 5), ("web".to_string(), 3)]),
        };
        let prompt = AnthropicEnricher::build_prompt(&input);
        assert!(prompt.contains("rust (5)"));
        assert!(prompt.contains("web (3)"));
        assert!(prompt.contains("Prefer reusing these existing tags"));
    }

    #[test]
    fn build_prompt_omits_existing_tags_when_empty() {
        let input = EnrichmentInput {
            url: "https://example.com".to_string(),
            scraped_title: None,
            scraped_description: None,
            existing_tags: Some(vec![]),
        };
        let prompt = AnthropicEnricher::build_prompt(&input);
        assert!(!prompt.contains("Prefer reusing these existing tags"));
        assert!(prompt.contains("https://example.com"));
    }

    #[test]
    fn parse_enrichment_json_from_clean_response() {
        let json =
            r#"{"title": "Better Title", "description": "Better desc", "tags": ["rust", "web"]}"#;
        let parsed: EnrichmentJson = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.title.as_deref(), Some("Better Title"));
        assert_eq!(parsed.tags.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn parse_enrichment_json_with_markdown_fences() {
        let text = "```json\n{\"title\": \"T\", \"description\": \"D\", \"tags\": [\"a\"]}\n```";
        let json_str = extract_json_object(text).unwrap();
        let parsed: EnrichmentJson = serde_json::from_str(json_str).unwrap();
        assert_eq!(parsed.title.as_deref(), Some("T"));
    }

    #[test]
    fn parse_enrichment_json_with_leading_text() {
        let text =
            "Here is the JSON:\n{\"title\": \"T\", \"description\": \"D\", \"tags\": [\"a\"]}";
        let json_str = extract_json_object(text).unwrap();
        let parsed: EnrichmentJson = serde_json::from_str(json_str).unwrap();
        assert_eq!(parsed.title.as_deref(), Some("T"));
    }

    #[test]
    fn extract_json_object_returns_none_for_no_braces() {
        assert!(extract_json_object("no json here").is_none());
    }
}
