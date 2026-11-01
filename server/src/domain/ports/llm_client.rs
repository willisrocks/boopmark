//! One provider-neutral LLM port shared by every text feature: tag
//! suggestions on save, tag consolidation, and later callers. Vendor adapters
//! live in `adapters::llm_client`. The feature ports (`LlmEnricher`,
//! `LlmPromptAssistant`) and tag consolidation are thin use cases on top of
//! this one (`app::tag_llm`, `app::image_prompt`), so no feature re-implements
//! vendor HTTP, auth or response parsing.

use crate::domain::error::DomainError;
use crate::domain::llm_settings::TextProvider;
use std::future::Future;
use std::pin::Pin;

/// Same shape as `futures::future::BoxFuture`, spelled out so the domain
/// layer does not take on the `futures` crate for one alias.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// One structured completion: a system prompt, one user turn, and a JSON
/// Schema the answer must follow. Adapters translate the schema to their
/// vendor's structured-output feature and drop keywords the vendor does not
/// enforce (array lengths, string lengths, numeric bounds); callers enforce
/// those on the parsed result themselves.
///
/// The provider, key and model come from the user's `llm_settings` and
/// travel with each request.
#[derive(Clone)]
pub struct StructuredRequest {
    pub provider: TextProvider,
    pub api_key: String,
    pub model: String,
    pub system: String,
    pub user: String,
    pub json_schema: serde_json::Value,
    pub max_output_tokens: u32,
}

/// Hand-written so the key and the prompts never reach a log line through
/// `{:?}`.
impl std::fmt::Debug for StructuredRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StructuredRequest")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("max_output_tokens", &self.max_output_tokens)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

#[derive(Clone, PartialEq)]
pub struct StructuredResponse {
    /// The answer, already checked against the request's schema (types,
    /// `enum`, `required`, `additionalProperties: false`).
    pub json: serde_json::Value,
    pub usage: TokenUsage,
}

/// Hand-written so the answer, which can echo bookmark content, never
/// reaches a log line through `{:?}`.
impl std::fmt::Debug for StructuredResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StructuredResponse")
            .field("usage", &self.usage)
            .finish_non_exhaustive()
    }
}

pub trait LlmClient: Send + Sync {
    /// Errors tell the caller whether a retry can help:
    ///
    /// - `InvalidInput`: the vendor will not accept this request as sent
    ///   (HTTP 4xx such as a bad request or a rejected key, or a refusal).
    ///   Do not retry.
    /// - `Internal`: transient or vendor-side (network, timeout, 408, 409,
    ///   429, 5xx, malformed or off-schema output, truncation). A retry with
    ///   backoff may succeed.
    ///
    /// Messages carry the vendor and HTTP status, never a key, prompt or
    /// response body.
    fn complete_structured(
        &self,
        req: StructuredRequest,
    ) -> BoxFuture<'_, Result<StructuredResponse, DomainError>>;
}
