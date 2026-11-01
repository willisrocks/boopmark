#![allow(dead_code)]
//! One provider-neutral LLM port shared by every text feature: tag suggestions
//! on save, tag consolidation, and future callers. Vendor adapters live in
//! `adapters::llm_client`; the feature
//! ports (`LlmEnricher`, `TagConsolidator`, `LlmPromptAssistant`) are meant to
//! become thin use-case adapters on top of this one instead of each vendor
//! adapter re-implementing HTTP, auth, and response parsing.

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
/// enforce; callers still validate bounds (array lengths) themselves.
#[derive(Debug, Clone)]
pub struct StructuredRequest {
    pub provider: TextProvider,
    pub api_key: String,
    pub model: String,
    pub system: String,
    pub user: String,
    pub json_schema: serde_json::Value,
    pub max_output_tokens: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

#[derive(Debug, Clone)]
pub struct StructuredResponse {
    pub json: serde_json::Value,
    pub usage: TokenUsage,
}

pub trait LlmClient: Send + Sync {
    fn complete_structured(
        &self,
        req: StructuredRequest,
    ) -> BoxFuture<'_, Result<StructuredResponse, DomainError>>;
}
