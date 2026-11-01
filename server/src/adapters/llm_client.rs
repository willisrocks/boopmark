#![allow(dead_code)]
//! Vendor adapters for the shared `LlmClient` port, plus the router that picks
//! one per request. Each adapter only knows how to send one structured request
//! to its vendor and read back JSON and token usage; prompts and schemas belong
//! to the use cases built on top.

use crate::adapters::openai::extract_response_text;
use crate::domain::error::DomainError;
use crate::domain::llm_settings::TextProvider;
use crate::domain::ports::llm_client::{
    BoxFuture, LlmClient, StructuredRequest, StructuredResponse, TokenUsage,
};
use serde_json::{Value, json};
use std::sync::Arc;

const ANTHROPIC_MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const OPENAI_RESPONSES_URL: &str = "https://api.openai.com/v1/responses";
const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// Schema keywords that structured-output engines do not enforce (Anthropic
/// rejects them; OpenAI strict mode accepts only some). They are removed before
/// sending and the caller enforces them on the parsed result.
const UNENFORCED_KEYWORDS: [&str; 7] = [
    "maxItems",
    "minItems",
    "minLength",
    "maxLength",
    "minimum",
    "maximum",
    "multipleOf",
];

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS))
        .build()
        .expect("failed to build LLM HTTP client")
}

/// Copy of `schema` without the keywords in `UNENFORCED_KEYWORDS`, at any depth.
pub fn strip_unenforced(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| !UNENFORCED_KEYWORDS.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), strip_unenforced(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(strip_unenforced).collect()),
        other => other.clone(),
    }
}

fn parse_json_text(text: &str, vendor: &str) -> Result<Value, DomainError> {
    serde_json::from_str(text.trim())
        .map_err(|_| DomainError::Internal(format!("{vendor} returned invalid structured JSON")))
}

fn usage_field(payload: &Value, path: [&str; 2]) -> u32 {
    payload
        .get(path[0])
        .and_then(|usage| usage.get(path[1]))
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32
}

async fn post_json(
    request: reqwest::RequestBuilder,
    body: &Value,
    vendor: &str,
) -> Result<Value, DomainError> {
    let response = request
        .header("content-type", "application/json")
        .json(body)
        .send()
        .await
        .map_err(|_| DomainError::Internal(format!("{vendor} API request failed")))?;
    if !response.status().is_success() {
        return Err(DomainError::Internal(format!(
            "{vendor} API returned HTTP {}",
            response.status()
        )));
    }
    response
        .json()
        .await
        .map_err(|_| DomainError::Internal(format!("{vendor} response parse failed")))
}

/// Anthropic Messages API with `output_config.format` JSON-schema outputs.
#[derive(Clone)]
pub struct AnthropicLlmClient {
    client: reqwest::Client,
    url: String,
    /// `output_config.effort` for models that accept it. Claude Haiku 5.5
    /// defaults to `medium`; short tagging calls do not need that much thinking.
    effort: Option<String>,
}

impl AnthropicLlmClient {
    pub fn new(effort: Option<String>) -> Self {
        Self {
            client: http_client(),
            url: ANTHROPIC_MESSAGES_URL.to_string(),
            effort,
        }
    }

    /// Claude Haiku 4.5 and older models reject `effort`.
    fn accepts_effort(model: &str) -> bool {
        !(model.starts_with("claude-haiku-4") || model.starts_with("claude-3"))
    }

    fn request_body(&self, req: &StructuredRequest) -> Value {
        let mut output_config = json!({
            "format": {"type": "json_schema", "schema": strip_unenforced(&req.json_schema)}
        });
        if let Some(effort) = self
            .effort
            .as_deref()
            .filter(|_| Self::accepts_effort(&req.model))
        {
            output_config["effort"] = json!(effort);
        }
        json!({
            "model": req.model,
            "max_tokens": req.max_output_tokens,
            "system": req.system,
            "messages": [{"role": "user", "content": req.user}],
            "output_config": output_config,
        })
    }

    fn parse_response(payload: &Value) -> Result<StructuredResponse, DomainError> {
        match payload.get("stop_reason").and_then(Value::as_str) {
            Some("refusal") => {
                return Err(DomainError::Internal(
                    "Anthropic declined the request".into(),
                ));
            }
            Some("max_tokens") => {
                return Err(DomainError::Internal(
                    "Anthropic response hit max_tokens before finishing".into(),
                ));
            }
            _ => {}
        }
        // Thinking blocks may come first; the answer is the text block.
        let text = payload
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .and_then(|block| block.get("text").and_then(Value::as_str))
            .ok_or_else(|| DomainError::Internal("Anthropic response had no text".into()))?;
        Ok(StructuredResponse {
            json: parse_json_text(text, "Anthropic")?,
            usage: TokenUsage {
                input_tokens: usage_field(payload, ["usage", "input_tokens"]),
                output_tokens: usage_field(payload, ["usage", "output_tokens"]),
            },
        })
    }
}

impl LlmClient for AnthropicLlmClient {
    fn complete_structured(
        &self,
        req: StructuredRequest,
    ) -> BoxFuture<'_, Result<StructuredResponse, DomainError>> {
        Box::pin(async move {
            let body = self.request_body(&req);
            let request = self
                .client
                .post(&self.url)
                .header("x-api-key", &req.api_key)
                .header("anthropic-version", "2023-06-01");
            Self::parse_response(&post_json(request, &body, "Anthropic").await?)
        })
    }
}

/// OpenAI Responses API with strict `text.format` JSON-schema outputs.
#[derive(Clone)]
pub struct OpenAiLlmClient {
    client: reqwest::Client,
    url: String,
}

impl OpenAiLlmClient {
    pub fn new() -> Self {
        Self {
            client: http_client(),
            url: OPENAI_RESPONSES_URL.to_string(),
        }
    }

    fn request_body(req: &StructuredRequest) -> Value {
        json!({
            "model": req.model,
            "instructions": req.system,
            "input": req.user,
            "max_output_tokens": req.max_output_tokens,
            "store": false,
            "text": {"format": {
                "type": "json_schema",
                "name": "result",
                "strict": true,
                "schema": strip_unenforced(&req.json_schema),
            }},
        })
    }

    fn parse_response(payload: &Value) -> Result<StructuredResponse, DomainError> {
        let text = extract_response_text(payload)
            .ok_or_else(|| DomainError::Internal("OpenAI response had no text".into()))?;
        Ok(StructuredResponse {
            json: parse_json_text(&text, "OpenAI")?,
            usage: TokenUsage {
                input_tokens: usage_field(payload, ["usage", "input_tokens"]),
                output_tokens: usage_field(payload, ["usage", "output_tokens"]),
            },
        })
    }
}

impl Default for OpenAiLlmClient {
    fn default() -> Self {
        Self::new()
    }
}

impl LlmClient for OpenAiLlmClient {
    fn complete_structured(
        &self,
        req: StructuredRequest,
    ) -> BoxFuture<'_, Result<StructuredResponse, DomainError>> {
        Box::pin(async move {
            let body = Self::request_body(&req);
            let request = self.client.post(&self.url).bearer_auth(&req.api_key);
            Self::parse_response(&post_json(request, &body, "OpenAI").await?)
        })
    }
}

/// A local llama.cpp `llama-server` (OpenAI-compatible chat completions with
/// grammar-constrained `response_format`). Used by the tag lab and for offline
/// development; the API key and the requested model name are ignored.
#[derive(Clone)]
pub struct LlamaCppLlmClient {
    client: reqwest::Client,
    base_url: String,
}

impl LlamaCppLlmClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            client: reqwest::Client::builder()
                // CPU inference over a long tag list can take minutes.
                .timeout(std::time::Duration::from_secs(900))
                .build()
                .expect("failed to build llama.cpp HTTP client"),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    fn request_body(req: &StructuredRequest) -> Value {
        json!({
            "model": req.model,
            "max_tokens": req.max_output_tokens,
            "messages": [
                {"role": "system", "content": req.system},
                {"role": "user", "content": req.user},
            ],
            "response_format": {"type": "json_schema", "json_schema": {
                "name": "result", "strict": true, "schema": req.json_schema,
            }},
        })
    }

    fn parse_response(payload: &Value) -> Result<StructuredResponse, DomainError> {
        let text = payload
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .ok_or_else(|| DomainError::Internal("llama.cpp response had no text".into()))?;
        Ok(StructuredResponse {
            json: parse_json_text(text, "llama.cpp")?,
            usage: TokenUsage {
                input_tokens: usage_field(payload, ["usage", "prompt_tokens"]),
                output_tokens: usage_field(payload, ["usage", "completion_tokens"]),
            },
        })
    }
}

impl LlmClient for LlamaCppLlmClient {
    fn complete_structured(
        &self,
        req: StructuredRequest,
    ) -> BoxFuture<'_, Result<StructuredResponse, DomainError>> {
        Box::pin(async move {
            let body = Self::request_body(&req);
            let request = self
                .client
                .post(format!("{}/v1/chat/completions", self.base_url));
            Self::parse_response(&post_json(request, &body, "llama.cpp").await?)
        })
    }
}

/// Deployment-level choice of where structured requests go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmClientConfig {
    /// Route by `StructuredRequest::provider` (the user's setting).
    Hosted { anthropic_effort: Option<String> },
    /// Send every request to a local llama.cpp server (lab and offline dev).
    LlamaCpp { base_url: String },
}

impl LlmClientConfig {
    /// `LLM_BACKEND=llamacpp` with `LLAMACPP_BASE_URL`, otherwise hosted;
    /// `ANTHROPIC_EFFORT` (default `low`) sets the Anthropic effort level.
    pub fn from_env() -> Self {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    fn from_vars(var: impl Fn(&str) -> Option<String>) -> Self {
        match var("LLM_BACKEND").as_deref() {
            Some("llamacpp") => Self::LlamaCpp {
                base_url: var("LLAMACPP_BASE_URL")
                    .unwrap_or_else(|| "http://127.0.0.1:8080".to_string()),
            },
            _ => Self::Hosted {
                anthropic_effort: Some(var("ANTHROPIC_EFFORT").unwrap_or_else(|| "low".into())),
            },
        }
    }
}

/// Picks the adapter for each request from its `provider`, unless the
/// deployment pins every request to one backend.
#[derive(Clone)]
pub struct LlmClientRouter {
    anthropic: Arc<dyn LlmClient>,
    openai: Arc<dyn LlmClient>,
    pinned: Option<Arc<dyn LlmClient>>,
}

impl LlmClientRouter {
    pub fn new(
        anthropic: Arc<dyn LlmClient>,
        openai: Arc<dyn LlmClient>,
        pinned: Option<Arc<dyn LlmClient>>,
    ) -> Self {
        Self {
            anthropic,
            openai,
            pinned,
        }
    }

    pub fn from_config(config: &LlmClientConfig) -> Self {
        match config {
            LlmClientConfig::Hosted { anthropic_effort } => Self::new(
                Arc::new(AnthropicLlmClient::new(anthropic_effort.clone())),
                Arc::new(OpenAiLlmClient::new()),
                None,
            ),
            LlmClientConfig::LlamaCpp { base_url } => Self::new(
                Arc::new(AnthropicLlmClient::new(None)),
                Arc::new(OpenAiLlmClient::new()),
                Some(Arc::new(LlamaCppLlmClient::new(base_url))),
            ),
        }
    }

    fn select(&self, provider: TextProvider) -> &Arc<dyn LlmClient> {
        if let Some(pinned) = &self.pinned {
            return pinned;
        }
        match provider {
            TextProvider::Anthropic => &self.anthropic,
            TextProvider::OpenAi => &self.openai,
        }
    }
}

impl LlmClient for LlmClientRouter {
    fn complete_structured(
        &self,
        req: StructuredRequest,
    ) -> BoxFuture<'_, Result<StructuredResponse, DomainError>> {
        self.select(req.provider).complete_structured(req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(model: &str) -> StructuredRequest {
        StructuredRequest {
            provider: TextProvider::Anthropic,
            api_key: "key".into(),
            model: model.into(),
            system: "sys".into(),
            user: "usr".into(),
            json_schema: json!({
                "type": "object",
                "properties": {"tags": {"type": "array", "items": {"type": "string", "enum": ["a"]}, "maxItems": 5}},
                "required": ["tags"],
                "additionalProperties": false
            }),
            max_output_tokens: 512,
        }
    }

    #[test]
    fn strip_removes_unenforced_keywords_at_depth_and_keeps_enum() {
        let stripped = strip_unenforced(&request("m").json_schema);
        assert!(stripped["properties"]["tags"].get("maxItems").is_none());
        assert_eq!(
            stripped["properties"]["tags"]["items"]["enum"],
            json!(["a"])
        );
    }

    #[test]
    fn anthropic_body_uses_output_config_and_effort_only_where_accepted() {
        let client = AnthropicLlmClient::new(Some("low".into()));
        let body = client.request_body(&request("claude-haiku-5-5"));
        assert_eq!(body["output_config"]["format"]["type"], "json_schema");
        assert_eq!(body["output_config"]["effort"], "low");
        assert_eq!(body["system"], "sys");
        assert!(body.get("temperature").is_none());
        let old = client.request_body(&request("claude-haiku-4-5-20251001"));
        assert!(old["output_config"].get("effort").is_none());
    }

    #[test]
    fn anthropic_parse_skips_thinking_blocks_and_reads_usage() {
        let payload = json!({
            "stop_reason": "end_turn",
            "content": [{"type": "thinking", "thinking": ""}, {"type": "text", "text": "{\"tags\":[\"a\"]}"}],
            "usage": {"input_tokens": 120, "output_tokens": 9}
        });
        let parsed = AnthropicLlmClient::parse_response(&payload).expect("parsed");
        assert_eq!(parsed.json["tags"], json!(["a"]));
        assert_eq!(
            parsed.usage,
            TokenUsage {
                input_tokens: 120,
                output_tokens: 9
            }
        );
    }

    #[test]
    fn anthropic_parse_rejects_truncated_and_refused_answers() {
        for stop in ["max_tokens", "refusal"] {
            let payload = json!({"stop_reason": stop, "content": [{"type": "text", "text": "{"}]});
            assert!(AnthropicLlmClient::parse_response(&payload).is_err());
        }
    }

    #[test]
    fn openai_body_is_strict_json_schema_and_parse_reads_usage() {
        let body = OpenAiLlmClient::request_body(&request("gpt-5.6-luna"));
        assert_eq!(body["text"]["format"]["strict"], true);
        assert_eq!(body["instructions"], "sys");
        let parsed = OpenAiLlmClient::parse_response(&json!({
            "output_text": "{\"tags\":[]}",
            "usage": {"input_tokens": 50, "output_tokens": 4}
        }))
        .expect("parsed");
        assert_eq!(parsed.usage.input_tokens, 50);
    }

    #[test]
    fn llamacpp_body_keeps_full_schema_for_grammar() {
        let body = LlamaCppLlmClient::request_body(&request("qwen3-4b"));
        assert_eq!(
            body["response_format"]["json_schema"]["schema"]["properties"]["tags"]["maxItems"],
            5
        );
        let parsed = LlamaCppLlmClient::parse_response(&json!({
            "choices": [{"message": {"content": "{\"tags\":[\"a\"]}"}}],
            "usage": {"prompt_tokens": 70, "completion_tokens": 6}
        }))
        .expect("parsed");
        assert_eq!(parsed.usage.output_tokens, 6);
    }

    #[test]
    fn config_reads_backend_from_env_vars() {
        let hosted = LlmClientConfig::from_vars(|_| None);
        assert_eq!(
            hosted,
            LlmClientConfig::Hosted {
                anthropic_effort: Some("low".into())
            }
        );
        let local = LlmClientConfig::from_vars(|name| match name {
            "LLM_BACKEND" => Some("llamacpp".into()),
            "LLAMACPP_BASE_URL" => Some("http://127.0.0.1:4211".into()),
            _ => None,
        });
        assert_eq!(
            local,
            LlmClientConfig::LlamaCpp {
                base_url: "http://127.0.0.1:4211".into()
            }
        );
    }

    struct Marker(&'static str);

    impl LlmClient for Marker {
        fn complete_structured(
            &self,
            _req: StructuredRequest,
        ) -> BoxFuture<'_, Result<StructuredResponse, DomainError>> {
            let marker = self.0;
            Box::pin(async move {
                Ok(StructuredResponse {
                    json: json!(marker),
                    usage: TokenUsage::default(),
                })
            })
        }
    }

    #[tokio::test]
    async fn router_dispatches_by_provider_unless_pinned() {
        let hosted = LlmClientRouter::new(
            Arc::new(Marker("anthropic")),
            Arc::new(Marker("openai")),
            None,
        );
        let mut req = request("gpt-5.6-luna");
        req.provider = TextProvider::OpenAi;
        assert_eq!(
            hosted.complete_structured(req.clone()).await.unwrap().json,
            "openai"
        );
        let pinned = LlmClientRouter::new(
            Arc::new(Marker("anthropic")),
            Arc::new(Marker("openai")),
            Some(Arc::new(Marker("local"))),
        );
        assert_eq!(pinned.complete_structured(req).await.unwrap().json, "local");
    }
}
