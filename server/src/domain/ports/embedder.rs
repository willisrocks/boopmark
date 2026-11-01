//! Provider-neutral text embedding port, used by article ingestion (document
//! chunks) and by search (the query). Vendor adapters live in
//! `adapters::embedder`; the backend is picked by `EMBEDDING_BACKEND`, so a
//! vendor can be swapped by configuration. See
//! docs/superpowers/specs/2026-10-31-hybrid-article-search-design.md.
//!
//! Unlike `LlmClient`, a request carries no API key: the index must be one
//! vector space for every user, so the key is the operator's and lives in the
//! adapter's configuration.

use crate::domain::error::DomainError;

pub use crate::domain::ports::llm_client::BoxFuture;

/// Models such as BGE, Voyage and Nomic embed queries and documents
/// differently (a prefix or an `input_type`). Adapters also use it to pick a
/// timeout: a query is on the request path, a document is background work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// Constructed by article search, which lands separately.
#[allow(dead_code)]
pub enum EmbedKind {
    Document,
    Query,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Embeddings {
    /// One vector per input, in input order, each `dims()` long and
    /// L2-normalised (so cosine distance is `1 - dot`).
    pub vectors: Vec<Vec<f32>>,
    /// Billable tokens reported by a hosted vendor; 0 for local models.
    pub usage_tokens: u32,
}

pub trait Embedder: Send + Sync {
    /// Identity of the vector space: `<backend>:<model>@<dims>`, for example
    /// `openai:text-embedding-3-large@1024`. Stored with every embedded
    /// bookmark; vectors from different ids are never compared, and a change
    /// of id means a re-embed backfill.
    fn model_id(&self) -> &str;

    /// Length of every vector; 0 when embeddings are disabled.
    fn dims(&self) -> usize;

    /// False for the disabled backend: search then skips the vector leg and
    /// ingestion stores text without vectors. Defaulted so real adapters need
    /// not implement it.
    fn enabled(&self) -> bool {
        true
    }

    /// Embed any number of inputs. Adapters split them into vendor-sized
    /// batches and keep the order. Errors:
    ///
    /// - `InvalidInput`: the request will never be accepted (an empty input,
    ///   HTTP 4xx such as a bad request or a rejected key, or the backend is
    ///   disabled). Do not retry.
    /// - `Internal`: transient or vendor-side (network, timeout, 408, 409,
    ///   429, 5xx, a malformed response). The ingest worker retries with
    ///   backoff; search drops the vector leg.
    ///
    /// Messages carry the HTTP status, never a key or response body.
    fn embed(
        &self,
        inputs: Vec<String>,
        kind: EmbedKind,
    ) -> BoxFuture<'_, Result<Embeddings, DomainError>>;
}
