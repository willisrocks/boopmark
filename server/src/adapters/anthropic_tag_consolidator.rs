use crate::domain::error::DomainError;
use crate::domain::ports::tag_consolidator::{
    ConsolidationInput, ConsolidationOutput, TagConsolidator,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

#[derive(Clone)]
pub struct AnthropicTagConsolidator {
    client: reqwest::Client,
}

impl AnthropicTagConsolidator {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    fn build_prompt(input: &ConsolidationInput) -> String {
        let mut tag_lines = String::new();
        for sample in &input.tags {
            let titles = if sample.sample_titles.is_empty() {
                "(no sample titles)".to_string()
            } else {
                sample
                    .sample_titles
                    .iter()
                    .map(|t| format!("\"{}\"", t.replace('"', "\\\"")))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            tag_lines.push_str(&format!(
                "- \"{}\" ({}): {}\n",
                sample.tag.replace('"', "\\\""),
                sample.count,
                titles
            ));
        }

        format!(
            "You are a bookmark tag organizer. The user has the following tags on their bookmarks. \
             For each tag, decide what tag(s) a bookmark currently carrying it should end up with.\n\n\
             Rules:\n\
             1. Merge variants, synonyms, and typos into a single canonical form. \
             Example: \"js\", \"javascript\", \"JavaScript\" should all map to [\"javascript\"].\n\
             2. You MAY add a broader parent tag alongside a narrow tag. Do NOT replace the narrow tag. \
             Example: \"react\" might map to [\"react\", \"frontend\"].\n\
             3. Do not invent tags unrelated to the input set or the user's apparent topics.\n\
             4. Use lowercase. Prefer the most common, idiomatic form.\n\
             5. Every input tag MUST be a key in your output. If no change, return the tag itself: \"rust\" -> [\"rust\"].\n\n\
             Tags (with bookmark count and up to 3 sample titles per tag):\n\
             {tag_lines}\n\
             Respond with ONLY valid JSON, no other text. The format is an object whose keys are the input tags \
             (exact case as given) and whose values are arrays of output tag strings:\n\
             {{\"input_tag_1\": [\"output_a\", \"output_b\"], \"input_tag_2\": [\"output_c\"]}}"
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

/// Extract the first JSON object from a text response by finding the first `{`
/// and last `}`. Handles markdown fences or stray text around the JSON.
fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end >= start {
        Some(&text[start..=end])
    } else {
        None
    }
}

impl TagConsolidator for AnthropicTagConsolidator {
    fn consolidate(
        &self,
        api_key: &str,
        model: &str,
        input: ConsolidationInput,
    ) -> Pin<Box<dyn Future<Output = Result<ConsolidationOutput, DomainError>> + Send + '_>> {
        let api_key = api_key.to_string();
        let model = model.to_string();
        Box::pin(async move { self.do_consolidate(&api_key, &model, input).await })
    }
}

impl AnthropicTagConsolidator {
    async fn do_consolidate(
        &self,
        api_key: &str,
        model: &str,
        input: ConsolidationInput,
    ) -> Result<ConsolidationOutput, DomainError> {
        let prompt = Self::build_prompt(&input);

        let request_body = AnthropicRequest {
            model: model.to_string(),
            max_tokens: 16384,
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

        let json_str = extract_json_object(&text).ok_or_else(|| {
            DomainError::Internal("LLM response contained no JSON object".to_string())
        })?;

        let mapping: HashMap<String, Vec<String>> = serde_json::from_str(json_str)
            .map_err(|e| DomainError::Internal(format!("LLM JSON parse error: {e}")))?;

        // Normalize keys to lowercase. The prompt asks for "exact case as given" but
        // also "use lowercase", and LLMs tend to lowercase keys. The service-layer
        // lookup must match, so we canonicalize here at the parse boundary.
        let mapping = mapping
            .into_iter()
            .map(|(k, v)| (k.to_lowercase(), v))
            .collect();

        Ok(ConsolidationOutput { mapping })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ports::tag_consolidator::TagSample;

    fn sample_input() -> ConsolidationInput {
        ConsolidationInput {
            tags: vec![
                TagSample {
                    tag: "js".to_string(),
                    count: 12,
                    sample_titles: vec![
                        "Promise.all guide".to_string(),
                        "Async iterators".to_string(),
                    ],
                },
                TagSample {
                    tag: "javascript".to_string(),
                    count: 8,
                    sample_titles: vec!["ECMAScript 2024 features".to_string()],
                },
            ],
        }
    }

    #[test]
    fn prompt_includes_each_tag_with_count_and_samples() {
        let prompt = AnthropicTagConsolidator::build_prompt(&sample_input());
        assert!(prompt.contains("\"js\""), "missing js: {prompt}");
        assert!(prompt.contains("(12)"), "missing count: {prompt}");
        assert!(prompt.contains("Promise.all guide"), "missing sample: {prompt}");
        assert!(prompt.contains("\"javascript\""), "missing javascript: {prompt}");
    }

    #[test]
    fn prompt_instructs_lowercase_and_json_only() {
        let prompt = AnthropicTagConsolidator::build_prompt(&sample_input());
        assert!(prompt.to_lowercase().contains("lowercase"));
        assert!(prompt.to_lowercase().contains("json"));
    }

    #[test]
    fn prompt_describes_parent_tag_rule() {
        let prompt = AnthropicTagConsolidator::build_prompt(&sample_input());
        let lower = prompt.to_lowercase();
        assert!(lower.contains("parent"));
        assert!(lower.contains("not replace") || lower.contains("do not replace"));
    }

    #[test]
    fn extract_json_handles_markdown_fences() {
        let text = "```json\n{\"js\": [\"javascript\"]}\n```";
        let json = extract_json_object(text).expect("json");
        let parsed: HashMap<String, Vec<String>> = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.get("js"), Some(&vec!["javascript".to_string()]));
    }

    #[test]
    fn extract_json_handles_leading_text() {
        let text = "Here you go:\n{\"js\": [\"javascript\"]}\n";
        let json = extract_json_object(text).expect("json");
        let parsed: HashMap<String, Vec<String>> = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.get("js"), Some(&vec!["javascript".to_string()]));
    }

    #[test]
    fn extract_json_returns_none_for_no_braces() {
        assert!(extract_json_object("nothing here").is_none());
    }
}
