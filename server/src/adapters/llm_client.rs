// Nothing calls the port yet: tag management and article search move onto it
// in their own changes. Remove this once they land.
#![allow(dead_code)]

//! Vendor adapters for the shared `LlmClient` port, plus the router that picks
//! one per request. Each adapter only knows how to send one structured request
//! to its vendor and read back JSON and token usage; prompts and schemas belong
//! to the use cases built on top.
//!
//! ```text
//! LLM_BACKEND=hosted | llamacpp        (default: hosted, routed by the user's provider)
//! LLAMACPP_BASE_URL=http://127.0.0.1:8080
//! ANTHROPIC_EFFORT=low                 (low | medium | high | xhigh | max | none)
//! ```

use crate::domain::error::DomainError;
use crate::domain::llm_settings::TextProvider;
use crate::domain::ports::llm_client::{
    BoxFuture, LlmClient, StructuredRequest, StructuredResponse, TokenUsage,
};
use reqwest::StatusCode;
use serde_json::{Map, Value, json};
use std::sync::Arc;
use std::time::Duration;

const ANTHROPIC_MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const OPENAI_RESPONSES_URL: &str = "https://api.openai.com/v1/responses";
const DEFAULT_LLAMACPP_BASE_URL: &str = "http://127.0.0.1:8080";
const HOSTED_TIMEOUT: Duration = Duration::from_secs(60);
/// CPU inference over a long tag list can take minutes.
const LLAMACPP_TIMEOUT: Duration = Duration::from_secs(900);
const DEFAULT_ANTHROPIC_EFFORT: &str = "low";
const ANTHROPIC_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

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

fn http_client(timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .expect("failed to build LLM HTTP client")
}

/// Keywords whose values are data, not schemas: never rewritten.
const LITERAL_KEYWORDS: [&str; 4] = ["enum", "const", "default", "examples"];

/// Copy of `schema` without the keywords in `UNENFORCED_KEYWORDS`, at any depth.
/// Property names and literal values (`enum`, `const`, ...) are kept even when
/// they contain a keyword.
pub fn strip_unenforced(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| !UNENFORCED_KEYWORDS.contains(&key.as_str()))
                .map(|(key, value)| {
                    let value = match key.as_str() {
                        "properties" | "$defs" | "definitions" => strip_each(value),
                        literal if LITERAL_KEYWORDS.contains(&literal) => value.clone(),
                        _ => strip_unenforced(value),
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(strip_unenforced).collect()),
        other => other.clone(),
    }
}

/// `properties` and `$defs` map names to schemas: strip inside each schema
/// without filtering the names.
fn strip_each(map: &Value) -> Value {
    match map {
        Value::Object(entries) => Value::Object(
            entries
                .iter()
                .map(|(name, schema)| (name.clone(), strip_unenforced(schema)))
                .collect(),
        ),
        other => strip_unenforced(other),
    }
}

/// Whether `value` follows `schema` in the keywords structured outputs
/// enforce: `type`, `enum`, `const`, `properties`, `required`,
/// `additionalProperties`, `items`, `anyOf` and local `$ref`s. Bounds
/// (`UNENFORCED_KEYWORDS`) and unknown keywords are not checked here.
pub fn matches_schema(schema: &Value, value: &Value) -> bool {
    matches_at(schema, value, schema, 0)
}

/// Deeper than any real answer schema; stops a self-referencing `$ref` loop.
const MAX_SCHEMA_DEPTH: usize = 64;

fn matches_at(schema: &Value, value: &Value, root: &Value, depth: usize) -> bool {
    if depth > MAX_SCHEMA_DEPTH {
        return false;
    }
    let Some(schema) = schema.as_object() else {
        // `true`/`false` schemas.
        return schema.as_bool().unwrap_or(true);
    };
    // Sibling keywords of `$ref` apply too (JSON Schema 2020-12).
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str)
        && !resolve_ref(root, reference)
            .is_some_and(|target| matches_at(target, value, root, depth + 1))
    {
        return false;
    }
    if let Some(options) = schema.get("anyOf").and_then(Value::as_array)
        && !options
            .iter()
            .any(|option| matches_at(option, value, root, depth + 1))
    {
        return false;
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array)
        && !allowed.contains(value)
    {
        return false;
    }
    if let Some(expected) = schema.get("const")
        && expected != value
    {
        return false;
    }
    if let Some(types) = schema.get("type") {
        let ok = match types {
            Value::String(name) => has_type(value, name),
            Value::Array(names) => names
                .iter()
                .filter_map(Value::as_str)
                .any(|name| has_type(value, name)),
            _ => true,
        };
        if !ok {
            return false;
        }
    }
    match value {
        Value::Object(fields) => object_matches(schema, fields, root, depth),
        Value::Array(items) => schema.get("items").is_none_or(|item_schema| {
            items
                .iter()
                .all(|item| matches_at(item_schema, item, root, depth + 1))
        }),
        _ => true,
    }
}

fn object_matches(
    schema: &Map<String, Value>,
    fields: &Map<String, Value>,
    root: &Value,
    depth: usize,
) -> bool {
    let properties = schema.get("properties").and_then(Value::as_object);
    let required_present = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .all(|name| fields.contains_key(name));
    if !required_present {
        return false;
    }
    fields.iter().all(|(name, field)| {
        match properties.and_then(|properties| properties.get(name)) {
            Some(field_schema) => matches_at(field_schema, field, root, depth + 1),
            None => match schema.get("additionalProperties") {
                Some(Value::Bool(allowed)) => *allowed,
                Some(extra_schema) => matches_at(extra_schema, field, root, depth + 1),
                None => true,
            },
        }
    })
}

fn has_type(value: &Value, name: &str) -> bool {
    match name {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "integer" => {
            value.is_i64()
                || value.is_u64()
                || value.as_f64().is_some_and(|number| number.fract() == 0.0)
        }
        _ => true,
    }
}

/// Resolves `#/$defs/name`-style pointers within the schema itself.
fn resolve_ref<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    root.pointer(reference.strip_prefix('#')?)
}

/// Statuses where the same request can succeed later: the SDKs' retry set.
fn is_retryable_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::REQUEST_TIMEOUT | StatusCode::CONFLICT | StatusCode::TOO_MANY_REQUESTS
    ) || status.is_server_error()
}

/// Maps a non-success HTTP status to the port's error classes: `Internal`
/// for retryable statuses, `InvalidInput` for the rest of 4xx. The message
/// carries the status only, never the response body.
pub(crate) fn http_status_error(vendor: &str, status: StatusCode) -> DomainError {
    let message = format!("{vendor} API returned HTTP {status}");
    if is_retryable_status(status) || !status.is_client_error() {
        DomainError::Internal(message)
    } else {
        DomainError::InvalidInput(message)
    }
}

/// Maps a transport failure. reqwest errors can embed the URL, so only the
/// kind of failure is reported. A request that cannot be built (a key that
/// is not a valid header value) fails the same way every time.
pub(crate) fn transport_error(vendor: &str, error: &reqwest::Error) -> DomainError {
    if error.is_builder() {
        DomainError::InvalidInput(format!("{vendor} API request could not be built"))
    } else if error.is_timeout() {
        DomainError::Internal(format!("{vendor} API request timed out"))
    } else {
        DomainError::Internal(format!("{vendor} API request failed"))
    }
}

async fn post_json(
    request: reqwest::RequestBuilder,
    body: &Value,
    vendor: &str,
) -> Result<Value, DomainError> {
    let response = request
        .json(body)
        .send()
        .await
        .map_err(|error| transport_error(vendor, &error))?;
    let status = response.status();
    if !status.is_success() {
        return Err(http_status_error(vendor, status));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| transport_error(vendor, &error))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| DomainError::Internal(format!("{vendor} returned a malformed response")))
}

/// Parses the answer text and checks it against the request's schema.
fn parse_answer(text: &str, schema: &Value, vendor: &str) -> Result<Value, DomainError> {
    let json: Value = serde_json::from_str(text.trim())
        .map_err(|_| DomainError::Internal(format!("{vendor} returned invalid structured JSON")))?;
    if !matches_schema(schema, &json) {
        return Err(DomainError::Internal(format!(
            "{vendor} answer did not match the requested schema"
        )));
    }
    Ok(json)
}

/// The response, or the error after logging what the failed answer still
/// cost. Refusals, truncation and off-schema answers are billed; the port
/// returns no usage with an error, so the tokens are logged (counts only).
fn billed(
    vendor: &str,
    usage: TokenUsage,
    answer: Result<Value, DomainError>,
) -> Result<StructuredResponse, DomainError> {
    answer
        .map(|json| StructuredResponse { json, usage })
        .inspect_err(|error| {
            if usage != TokenUsage::default() {
                tracing::warn!(
                    vendor,
                    input_tokens = usage.input_tokens,
                    output_tokens = usage.output_tokens,
                    "billed LLM call failed: {error}"
                );
            }
        })
}

fn usage_field(payload: &Value, pointer: &str) -> u32 {
    payload
        .pointer(pointer)
        .and_then(Value::as_u64)
        .map_or(0, |tokens| u32::try_from(tokens).unwrap_or(u32::MAX))
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
        Self::with_url(ANTHROPIC_MESSAGES_URL, effort)
    }

    fn with_url(url: &str, effort: Option<String>) -> Self {
        Self {
            client: http_client(HOSTED_TIMEOUT),
            url: url.to_string(),
            effort,
        }
    }

    /// Effort arrived with Claude Opus 4.5; Haiku 4.5, Sonnet 4.5 and older
    /// models reject it.
    fn accepts_effort(model: &str) -> bool {
        // `-4-2` matches the dated 4.0 snapshots (`claude-opus-4-20250514`).
        const REJECTING_PREFIXES: [&str; 7] = [
            "claude-3",
            "claude-haiku-4",
            "claude-sonnet-4-2",
            "claude-sonnet-4-5",
            "claude-opus-4-0",
            "claude-opus-4-1",
            "claude-opus-4-2",
        ];
        !(model == "claude-sonnet-4"
            || model == "claude-opus-4"
            || REJECTING_PREFIXES
                .iter()
                .any(|prefix| model.starts_with(prefix)))
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
        // No `temperature`: current models reject non-default sampling.
        json!({
            "model": req.model,
            "max_tokens": req.max_output_tokens,
            "system": req.system,
            "messages": [{"role": "user", "content": req.user}],
            "output_config": output_config,
        })
    }

    fn parse_response(payload: &Value, schema: &Value) -> Result<StructuredResponse, DomainError> {
        let usage = TokenUsage {
            input_tokens: usage_field(payload, "/usage/input_tokens"),
            output_tokens: usage_field(payload, "/usage/output_tokens"),
        };
        billed("Anthropic", usage, Self::answer(payload, schema))
    }

    fn answer(payload: &Value, schema: &Value) -> Result<Value, DomainError> {
        match payload.get("stop_reason").and_then(Value::as_str) {
            Some("refusal") => {
                return Err(DomainError::InvalidInput(
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
        parse_answer(text, schema, "Anthropic")
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
                .header("anthropic-version", ANTHROPIC_VERSION);
            Self::parse_response(
                &post_json(request, &body, "Anthropic").await?,
                &req.json_schema,
            )
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
        Self::with_url(OPENAI_RESPONSES_URL)
    }

    fn with_url(url: &str) -> Self {
        Self {
            client: http_client(HOSTED_TIMEOUT),
            url: url.to_string(),
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

    fn parse_response(payload: &Value, schema: &Value) -> Result<StructuredResponse, DomainError> {
        let usage = TokenUsage {
            input_tokens: usage_field(payload, "/usage/input_tokens"),
            output_tokens: usage_field(payload, "/usage/output_tokens"),
        };
        billed("OpenAI", usage, Self::answer(payload, schema))
    }

    fn answer(payload: &Value, schema: &Value) -> Result<Value, DomainError> {
        if payload.get("status").and_then(Value::as_str) == Some("incomplete") {
            let reason = payload
                .pointer("/incomplete_details/reason")
                .and_then(Value::as_str);
            return Err(if reason == Some("content_filter") {
                DomainError::InvalidInput("OpenAI declined the request".into())
            } else {
                DomainError::Internal("OpenAI response was incomplete".into())
            });
        }
        // Reasoning items may come first; the answer is the message content.
        let contents: Vec<&Value> = payload
            .get("output")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| item.get("content").and_then(Value::as_array))
            .flatten()
            .collect();
        fn content_type(content: &Value) -> Option<&str> {
            content.get("type").and_then(Value::as_str)
        }
        if contents
            .iter()
            .any(|content| content_type(content) == Some("refusal"))
        {
            return Err(DomainError::InvalidInput(
                "OpenAI declined the request".into(),
            ));
        }
        let text = payload
            .get("output_text")
            .and_then(Value::as_str)
            .or_else(|| {
                contents
                    .iter()
                    .find(|content| content_type(content) == Some("output_text"))
                    .and_then(|content| content.get("text").and_then(Value::as_str))
            })
            .ok_or_else(|| DomainError::Internal("OpenAI response had no text".into()))?;
        parse_answer(text, schema, "OpenAI")
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
            Self::parse_response(
                &post_json(request, &body, "OpenAI").await?,
                &req.json_schema,
            )
        })
    }
}

/// A local llama.cpp `llama-server` (OpenAI-compatible chat completions with
/// grammar-constrained `response_format`). Used by the tag lab and for offline
/// development; the API key is not sent and the model name is informational.
#[derive(Clone)]
pub struct LlamaCppLlmClient {
    client: reqwest::Client,
    url: String,
}

impl LlamaCppLlmClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            client: http_client(LLAMACPP_TIMEOUT),
            url: format!("{}/v1/chat/completions", base_url.trim_end_matches('/')),
        }
    }

    /// The grammar can enforce every keyword, so the schema is sent whole.
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

    fn parse_response(payload: &Value, schema: &Value) -> Result<StructuredResponse, DomainError> {
        let usage = TokenUsage {
            input_tokens: usage_field(payload, "/usage/prompt_tokens"),
            output_tokens: usage_field(payload, "/usage/completion_tokens"),
        };
        billed("llama.cpp", usage, Self::answer(payload, schema))
    }

    fn answer(payload: &Value, schema: &Value) -> Result<Value, DomainError> {
        if payload
            .pointer("/choices/0/finish_reason")
            .and_then(Value::as_str)
            == Some("length")
        {
            return Err(DomainError::Internal(
                "llama.cpp response hit max_tokens before finishing".into(),
            ));
        }
        let text = payload
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .ok_or_else(|| DomainError::Internal("llama.cpp response had no text".into()))?;
        parse_answer(text, schema, "llama.cpp")
    }
}

impl LlmClient for LlamaCppLlmClient {
    fn complete_structured(
        &self,
        req: StructuredRequest,
    ) -> BoxFuture<'_, Result<StructuredResponse, DomainError>> {
        Box::pin(async move {
            let body = Self::request_body(&req);
            let request = self.client.post(&self.url);
            Self::parse_response(
                &post_json(request, &body, "llama.cpp").await?,
                &req.json_schema,
            )
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
    /// `ANTHROPIC_EFFORT` (default `low`, `none` to omit it) sets the
    /// Anthropic effort level.
    pub fn from_env() -> Self {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    fn from_vars(var: impl Fn(&str) -> Option<String>) -> Self {
        let var = |name: &str| var(name).filter(|value| !value.trim().is_empty());
        let backend = var("LLM_BACKEND").map(|value| value.trim().to_ascii_lowercase());
        match backend.as_deref() {
            Some("llamacpp") => {
                return Self::LlamaCpp {
                    base_url: var("LLAMACPP_BASE_URL")
                        .unwrap_or_else(|| DEFAULT_LLAMACPP_BASE_URL.to_string()),
                };
            }
            None | Some("hosted") => {}
            Some(_) => tracing::warn!("unknown LLM_BACKEND, using hosted"),
        }
        let effort = var("ANTHROPIC_EFFORT").map(|value| value.trim().to_ascii_lowercase());
        let anthropic_effort = match effort.as_deref() {
            None => Some(DEFAULT_ANTHROPIC_EFFORT.to_string()),
            Some("none") => None,
            Some(level) if ANTHROPIC_EFFORTS.contains(&level) => Some(level.to_string()),
            Some(_) => {
                tracing::warn!("unknown ANTHROPIC_EFFORT, using {DEFAULT_ANTHROPIC_EFFORT}");
                Some(DEFAULT_ANTHROPIC_EFFORT.to_string())
            }
        };
        Self::Hosted { anthropic_effort }
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
    use axum::{Router, extract::State, http::HeaderMap, routing::post};
    use std::sync::Mutex;
    use tokio::net::TcpListener;

    const SECRET_BODY: &str = "secret detail from the vendor";

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "tags": {
                    "type": "array",
                    "items": {"type": "string", "enum": ["a", "b"]},
                    "maxItems": 1
                }
            },
            "required": ["tags"],
            "additionalProperties": false
        })
    }

    fn request(provider: TextProvider, model: &str) -> StructuredRequest {
        StructuredRequest {
            provider,
            api_key: "sk-test".into(),
            model: model.into(),
            system: "sys".into(),
            user: "usr".into(),
            json_schema: schema(),
            max_output_tokens: 512,
        }
    }

    /// What the fake vendor received.
    #[derive(Clone, Default)]
    struct Seen(Arc<Mutex<Vec<(HeaderMap, Value)>>>);

    impl Seen {
        fn last(&self) -> (HeaderMap, Value) {
            self.0.lock().unwrap().last().cloned().expect("a request")
        }
    }

    /// Starts a fake vendor on a free port that answers every POST with
    /// `status` and `body`, and records the requests.
    async fn fake_vendor(path: &str, status: u16, body: String) -> (String, Seen) {
        let seen = Seen::default();
        let reply = move |State(seen): State<Seen>, headers: HeaderMap, request: String| {
            let body = body.clone();
            async move {
                let request = serde_json::from_str(&request).unwrap_or(Value::Null);
                seen.0.lock().unwrap().push((headers, request));
                (axum::http::StatusCode::from_u16(status).unwrap(), body)
            }
        };
        let router = Router::new()
            .route(path, post(reply))
            .with_state(seen.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (format!("http://{address}{path}"), seen)
    }

    fn anthropic_answer(text: &str) -> String {
        json!({
            "stop_reason": "end_turn",
            "content": [
                {"type": "thinking", "thinking": ""},
                {"type": "text", "text": text}
            ],
            "usage": {"input_tokens": 120, "output_tokens": 9}
        })
        .to_string()
    }

    fn openai_answer(text: &str) -> String {
        json!({
            "status": "completed",
            "output": [
                {"type": "reasoning", "summary": []},
                {"type": "message", "content": [{"type": "output_text", "text": text}]}
            ],
            "usage": {"input_tokens": 50, "output_tokens": 4}
        })
        .to_string()
    }

    async fn anthropic_call(status: u16, body: String) -> Result<StructuredResponse, DomainError> {
        let (url, _) = fake_vendor("/v1/messages", status, body).await;
        AnthropicLlmClient::with_url(&url, Some("low".into()))
            .complete_structured(request(TextProvider::Anthropic, "claude-haiku-5-5"))
            .await
    }

    async fn openai_call(status: u16, body: String) -> Result<StructuredResponse, DomainError> {
        let (url, _) = fake_vendor("/v1/responses", status, body).await;
        OpenAiLlmClient::with_url(&url)
            .complete_structured(request(TextProvider::OpenAi, "gpt-5.6-luna"))
            .await
    }

    fn assert_invalid_input(result: Result<StructuredResponse, DomainError>, needle: &str) {
        match result {
            Err(DomainError::InvalidInput(message)) => {
                assert!(message.contains(needle), "{message}");
                assert!(!message.contains(SECRET_BODY));
                assert!(!message.contains("sk-test"));
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    fn assert_internal(result: Result<StructuredResponse, DomainError>, needle: &str) {
        match result {
            Err(DomainError::Internal(message)) => {
                assert!(message.contains(needle), "{message}");
                assert!(!message.contains(SECRET_BODY));
                assert!(!message.contains("sk-test"));
            }
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[test]
    fn strip_removes_unenforced_keywords_at_depth_and_keeps_enum() {
        let stripped = strip_unenforced(&schema());
        assert!(stripped["properties"]["tags"].get("maxItems").is_none());
        assert_eq!(
            stripped["properties"]["tags"]["items"]["enum"],
            json!(["a", "b"])
        );
    }

    #[test]
    fn strip_keeps_properties_named_like_keywords() {
        let stripped = strip_unenforced(&json!({
            "type": "object",
            "properties": {"minimum": {"type": "integer", "minimum": 0}},
            "required": ["minimum"]
        }));
        assert_eq!(
            stripped["properties"]["minimum"],
            json!({"type": "integer"})
        );
    }

    #[test]
    fn strip_leaves_literal_values_alone() {
        let literals = json!({
            "enum": [{"minimum": 1}],
            "const": {"maxItems": 2},
            "default": {"minLength": 3},
            "examples": [{"maximum": 4}]
        });
        assert_eq!(strip_unenforced(&literals), literals);
    }

    #[test]
    fn schema_check_covers_types_enum_required_and_extra_fields() {
        let schema = schema();
        assert!(matches_schema(&schema, &json!({"tags": ["a"]})));
        // Bounds are the caller's job.
        assert!(matches_schema(&schema, &json!({"tags": ["a", "b"]})));
        assert!(!matches_schema(&schema, &json!({"tags": ["c"]})));
        assert!(!matches_schema(&schema, &json!({})));
        assert!(!matches_schema(&schema, &json!({"tags": "a"})));
        assert!(!matches_schema(&schema, &json!({"tags": [], "extra": 1})));
        assert!(!matches_schema(&schema, &json!(["a"])));

        let nullable = json!({
            "$defs": {"id": {"type": "integer"}},
            "type": "object",
            "properties": {"id": {"anyOf": [{"$ref": "#/$defs/id"}, {"type": "null"}]}}
        });
        assert!(matches_schema(&nullable, &json!({"id": 3})));
        assert!(matches_schema(&nullable, &json!({"id": null})));
        assert!(!matches_schema(&nullable, &json!({"id": "3"})));

        let looping = json!({"$ref": "#"});
        assert!(!matches_schema(&looping, &json!({})));

        let narrowed = json!({
            "$defs": {"tag": {"type": "string"}},
            "$ref": "#/$defs/tag",
            "enum": ["a"]
        });
        assert!(matches_schema(&narrowed, &json!("a")));
        assert!(
            !matches_schema(&narrowed, &json!("b")),
            "sibling enum applies"
        );
        assert!(!matches_schema(&narrowed, &json!(1)), "the target applies");
    }

    #[test]
    fn anthropic_body_uses_output_config_and_effort_only_where_accepted() {
        let client = AnthropicLlmClient::new(Some("low".into()));
        let body = client.request_body(&request(TextProvider::Anthropic, "claude-haiku-5-5"));
        assert_eq!(body["output_config"]["format"]["type"], "json_schema");
        assert!(
            body["output_config"]["format"]["schema"]["properties"]["tags"]
                .get("maxItems")
                .is_none()
        );
        assert_eq!(body["output_config"]["effort"], "low");
        assert_eq!(body["system"], "sys");
        assert_eq!(
            body["messages"],
            json!([{"role": "user", "content": "usr"}])
        );
        assert_eq!(body["max_tokens"], 512);
        assert!(body.get("temperature").is_none());

        for model in [
            "claude-haiku-4-5-20251001",
            "claude-sonnet-4-5",
            "claude-sonnet-4-20250514",
            "claude-opus-4-1",
            "claude-3-7-sonnet-latest",
        ] {
            let old = client.request_body(&request(TextProvider::Anthropic, model));
            assert!(old["output_config"].get("effort").is_none(), "{model}");
        }
        for model in [
            "claude-opus-5",
            "claude-sonnet-5",
            "claude-opus-4-5",
            "claude-sonnet-4-6",
        ] {
            assert!(AnthropicLlmClient::accepts_effort(model), "{model}");
        }
    }

    #[tokio::test]
    async fn anthropic_success_sends_auth_headers_skips_thinking_and_reads_usage() {
        let (url, seen) =
            fake_vendor("/v1/messages", 200, anthropic_answer("{\"tags\":[\"a\"]}")).await;
        let response = AnthropicLlmClient::with_url(&url, None)
            .complete_structured(request(TextProvider::Anthropic, "claude-haiku-5-5"))
            .await
            .expect("answer");
        assert_eq!(response.json, json!({"tags": ["a"]}));
        assert_eq!(
            response.usage,
            TokenUsage {
                input_tokens: 120,
                output_tokens: 9
            }
        );
        let (headers, body) = seen.last();
        assert_eq!(headers["x-api-key"], "sk-test");
        assert_eq!(headers["anthropic-version"], ANTHROPIC_VERSION);
        assert_eq!(body["model"], "claude-haiku-5-5");
        assert!(body["output_config"].get("effort").is_none());
    }

    #[tokio::test]
    async fn anthropic_maps_http_errors_by_retryability_without_leaking_bodies() {
        let secret = json!({"error": {"message": SECRET_BODY}}).to_string();
        assert_invalid_input(anthropic_call(401, secret.clone()).await, "401");
        assert_invalid_input(anthropic_call(400, secret.clone()).await, "400");
        assert_internal(anthropic_call(429, secret.clone()).await, "429");
        assert_internal(anthropic_call(500, secret.clone()).await, "500");
        assert_internal(anthropic_call(529, secret).await, "529");
    }

    #[tokio::test]
    async fn anthropic_rejects_malformed_and_off_schema_answers() {
        assert_internal(
            anthropic_call(200, format!("<html>{SECRET_BODY}")).await,
            "malformed",
        );
        assert_internal(
            anthropic_call(200, anthropic_answer("not json")).await,
            "invalid structured JSON",
        );
        assert_internal(
            anthropic_call(200, anthropic_answer("{\"labels\":[\"a\"]}")).await,
            "did not match",
        );
        assert_internal(
            anthropic_call(200, json!({"content": []}).to_string()).await,
            "no text",
        );
    }

    #[tokio::test]
    async fn anthropic_treats_refusal_as_final_and_truncation_as_retryable() {
        let refusal = json!({"stop_reason": "refusal", "content": []}).to_string();
        assert_invalid_input(anthropic_call(200, refusal).await, "declined");
        let truncated = json!({
            "stop_reason": "max_tokens",
            "content": [{"type": "text", "text": "{\"tags\":["}]
        })
        .to_string();
        assert_internal(anthropic_call(200, truncated).await, "max_tokens");
    }

    #[test]
    fn openai_body_is_strict_json_schema_without_bounds() {
        let body = OpenAiLlmClient::request_body(&request(TextProvider::OpenAi, "gpt-5.6-luna"));
        assert_eq!(body["text"]["format"]["type"], "json_schema");
        assert_eq!(body["text"]["format"]["strict"], true);
        assert!(
            body["text"]["format"]["schema"]["properties"]["tags"]
                .get("maxItems")
                .is_none()
        );
        assert_eq!(body["instructions"], "sys");
        assert_eq!(body["input"], "usr");
        assert_eq!(body["max_output_tokens"], 512);
        assert_eq!(body["store"], false);
    }

    #[tokio::test]
    async fn openai_success_sends_bearer_key_and_reads_message_text_and_usage() {
        let (url, seen) =
            fake_vendor("/v1/responses", 200, openai_answer("{\"tags\":[\"b\"]}")).await;
        let response = OpenAiLlmClient::with_url(&url)
            .complete_structured(request(TextProvider::OpenAi, "gpt-5.6-luna"))
            .await
            .expect("answer");
        assert_eq!(response.json, json!({"tags": ["b"]}));
        assert_eq!(
            response.usage,
            TokenUsage {
                input_tokens: 50,
                output_tokens: 4
            }
        );
        let (headers, body) = seen.last();
        assert_eq!(headers["authorization"], "Bearer sk-test");
        assert_eq!(body["model"], "gpt-5.6-luna");

        let convenience = json!({"output_text": "{\"tags\":[]}"}).to_string();
        assert_eq!(
            openai_call(200, convenience).await.expect("answer").json,
            json!({"tags": []})
        );
    }

    #[tokio::test]
    async fn openai_maps_http_errors_by_retryability_without_leaking_bodies() {
        let secret = json!({"error": {"message": SECRET_BODY}}).to_string();
        assert_invalid_input(openai_call(401, secret.clone()).await, "401");
        assert_invalid_input(openai_call(404, secret.clone()).await, "404");
        assert_internal(openai_call(429, secret.clone()).await, "429");
        assert_internal(openai_call(503, secret).await, "503");
    }

    #[tokio::test]
    async fn openai_rejects_malformed_off_schema_refused_and_incomplete_answers() {
        assert_internal(openai_call(200, "{\"output\":".into()).await, "malformed");
        assert_internal(
            openai_call(200, openai_answer("{\"tags\":")).await,
            "invalid structured JSON",
        );
        assert_internal(
            openai_call(200, openai_answer("{\"tags\":[\"z\"]}")).await,
            "did not match",
        );
        let refusal = json!({"output": [{"type": "message", "content": [
            {"type": "refusal", "refusal": "no"}
        ]}]})
        .to_string();
        assert_invalid_input(openai_call(200, refusal).await, "declined");
        let incomplete = json!({
            "status": "incomplete",
            "incomplete_details": {"reason": "max_output_tokens"}
        })
        .to_string();
        assert_internal(openai_call(200, incomplete).await, "incomplete");
    }

    #[tokio::test]
    async fn network_failures_are_retryable() {
        // Bind and drop a listener so the port is closed.
        let address = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap();
        let result = OpenAiLlmClient::with_url(&format!("http://{address}/v1/responses"))
            .complete_structured(request(TextProvider::OpenAi, "gpt-5.6-luna"))
            .await;
        assert_internal(result, "request failed");
    }

    #[tokio::test]
    async fn unbuildable_requests_are_final() {
        // A key with a newline is not a valid header value; it never reaches
        // the network and no retry can fix it.
        let (url, seen) = fake_vendor("/v1/responses", 200, openai_answer("{}")).await;
        let mut req = request(TextProvider::OpenAi, "gpt-5.6-luna");
        req.api_key = "sk-test\n".into();
        let result = OpenAiLlmClient::with_url(&url)
            .complete_structured(req)
            .await;
        assert_invalid_input(result, "could not be built");
        assert!(seen.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn llamacpp_sends_full_schema_without_key_and_reads_usage() {
        let answer = json!({
            "choices": [{"finish_reason": "stop", "message": {"content": "{\"tags\":[\"a\"]}"}}],
            "usage": {"prompt_tokens": 70, "completion_tokens": 6}
        })
        .to_string();
        let (url, seen) = fake_vendor("/v1/chat/completions", 200, answer).await;
        let base_url = url.trim_end_matches("/v1/chat/completions").to_string() + "/";
        let client = LlamaCppLlmClient::new(&base_url);
        let response = client
            .complete_structured(request(TextProvider::Anthropic, "qwen3-4b"))
            .await
            .expect("answer");
        assert_eq!(response.usage.output_tokens, 6);
        let (headers, body) = seen.last();
        assert!(headers.get("authorization").is_none());
        assert!(headers.get("x-api-key").is_none());
        assert_eq!(
            body["response_format"]["json_schema"]["schema"]["properties"]["tags"]["maxItems"],
            1
        );

        let truncated =
            json!({"choices": [{"finish_reason": "length", "message": {"content": "{"}}]});
        assert!(matches!(
            LlamaCppLlmClient::parse_response(&truncated, &schema()),
            Err(DomainError::Internal(_))
        ));
    }

    #[test]
    fn config_reads_backend_and_effort_from_env_vars() {
        let vars = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.to_string())
            }
        };
        assert_eq!(
            LlmClientConfig::from_vars(vars(&[])),
            LlmClientConfig::Hosted {
                anthropic_effort: Some("low".into())
            }
        );
        assert_eq!(
            LlmClientConfig::from_vars(vars(&[("ANTHROPIC_EFFORT", "High")])),
            LlmClientConfig::Hosted {
                anthropic_effort: Some("high".into())
            }
        );
        assert_eq!(
            LlmClientConfig::from_vars(vars(&[("ANTHROPIC_EFFORT", "none")])),
            LlmClientConfig::Hosted {
                anthropic_effort: None
            }
        );
        assert_eq!(
            LlmClientConfig::from_vars(vars(&[("ANTHROPIC_EFFORT", "turbo")])),
            LlmClientConfig::Hosted {
                anthropic_effort: Some("low".into())
            }
        );
        assert_eq!(
            LlmClientConfig::from_vars(vars(&[
                ("LLM_BACKEND", "llamacpp"),
                ("LLAMACPP_BASE_URL", "http://127.0.0.1:4211"),
            ])),
            LlmClientConfig::LlamaCpp {
                base_url: "http://127.0.0.1:4211".into()
            }
        );
        assert_eq!(
            LlmClientConfig::from_vars(vars(&[("LLM_BACKEND", "llamacpp")])),
            LlmClientConfig::LlamaCpp {
                base_url: DEFAULT_LLAMACPP_BASE_URL.into()
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
        let openai = request(TextProvider::OpenAi, "gpt-5.6-luna");
        let anthropic = request(TextProvider::Anthropic, "claude-haiku-5-5");
        assert_eq!(
            hosted
                .complete_structured(openai.clone())
                .await
                .unwrap()
                .json,
            "openai"
        );
        assert_eq!(
            hosted.complete_structured(anthropic).await.unwrap().json,
            "anthropic"
        );
        let pinned = LlmClientRouter::new(
            Arc::new(Marker("anthropic")),
            Arc::new(Marker("openai")),
            Some(Arc::new(Marker("local"))),
        );
        assert_eq!(
            pinned.complete_structured(openai).await.unwrap().json,
            "local"
        );
    }

    #[test]
    fn request_debug_hides_key_and_prompts() {
        let shown = format!("{:?}", request(TextProvider::OpenAi, "gpt-5.6-luna"));
        assert!(shown.contains("gpt-5.6-luna"));
        assert!(!shown.contains("sk-test"));
        assert!(!shown.contains("usr"));

        let response = StructuredResponse {
            json: json!({"title": "private bookmark title"}),
            usage: TokenUsage::default(),
        };
        assert!(!format!("{response:?}").contains("private"));
    }
}
