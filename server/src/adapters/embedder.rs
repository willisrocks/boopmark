// Nothing calls the port yet: tag management and article search move onto it
// in their own changes. Remove this once they land.
#![allow(dead_code)]

//! Vendor adapters for the `Embedder` port, plus the router that builds the
//! one selected by configuration:
//!
//! ```text
//! EMBEDDING_BACKEND=disabled | openai | local     (default: disabled; `fastembed` = `local`)
//! EMBEDDING_MODEL=text-embedding-3-large          (local default: BAAI/bge-small-en-v1.5)
//! EMBEDDING_DIMENSIONS=1024                       (openai only; default: 1024 for text-embedding-3-*,
//!                                                 else the native size; must be 1024 or 384)
//! OPENAI_EMBEDDING_API_KEY=sk-...                 (operator key, not a user's BYO key)
//! FASTEMBED_CACHE_DIR=...                         (local only; model download cache)
//! ```
//!
//! `local` needs the `local-embeddings` cargo feature, so the default build
//! carries no ONNX Runtime.

use crate::adapters::llm_client::{http_status_error, transport_error};
use crate::domain::error::DomainError;
use crate::domain::ports::embedder::{BoxFuture, EmbedKind, Embedder, Embeddings};
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

const OPENAI_EMBEDDINGS_URL: &str = "https://api.openai.com/v1/embeddings";
const DEFAULT_OPENAI_MODEL: &str = "text-embedding-3-large";
/// The measured production choice: text-embedding-3-large shortened to 1,024
/// dimensions. Every `text-embedding-3-*` model defaults to it, so naming the
/// model alone does not change the vector space size.
const DEFAULT_OPENAI_DIMENSIONS: usize = 1024;
const DEFAULT_LOCAL_MODEL: &str = "BAAI/bge-small-en-v1.5";
/// The vector sizes the article-search schema has an HNSW index for
/// (te3 at 1,024, bge-small at 384). Any other size could not be searched.
const INDEXED_DIMENSIONS: [usize; 2] = [1024, 384];
/// The API accepts 2,048 inputs and 300k tokens per request; stay well below.
const OPENAI_MAX_INPUTS: usize = 256;
/// Every token covers at least one byte of UTF-8, so a byte cap below 300k
/// also keeps a batch under the token limit, whatever the script.
const OPENAI_MAX_BATCH_BYTES: usize = 250_000;
/// A query embedding is on the search request path: past this, search runs
/// the lexical legs alone.
const QUERY_TIMEOUT: Duration = Duration::from_millis(1500);
const DOCUMENT_TIMEOUT: Duration = Duration::from_secs(30);
const VENDOR: &str = "OpenAI embeddings";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingBackend {
    Disabled,
    OpenAi,
    /// fastembed (ONNX) in-process; needs the `local-embeddings` feature.
    Local,
}

#[derive(Clone, PartialEq)]
pub struct EmbeddingSettings {
    pub backend: EmbeddingBackend,
    pub model: String,
    pub dimensions: Option<usize>,
    pub openai_api_key: Option<String>,
}

/// Hand-written so the operator key never reaches a log line.
impl std::fmt::Debug for EmbeddingSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbeddingSettings")
            .field("backend", &self.backend)
            .field("model", &self.model)
            .field("dimensions", &self.dimensions)
            .field(
                "openai_api_key",
                &self.openai_api_key.as_ref().map(|_| "set"),
            )
            .finish()
    }
}

impl EmbeddingSettings {
    pub fn from_env() -> Result<Self, DomainError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, DomainError> {
        let get = |key: &str| {
            get(key)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        let backend = match get("EMBEDDING_BACKEND")
            .map(|value| value.to_ascii_lowercase())
            .as_deref()
        {
            None | Some("disabled") => EmbeddingBackend::Disabled,
            Some("openai") => EmbeddingBackend::OpenAi,
            Some("local" | "fastembed") => EmbeddingBackend::Local,
            Some(other) => {
                return Err(DomainError::InvalidInput(format!(
                    "EMBEDDING_BACKEND={other} is not one of disabled, openai, local"
                )));
            }
        };
        let default_model = match backend {
            EmbeddingBackend::Local => DEFAULT_LOCAL_MODEL,
            _ => DEFAULT_OPENAI_MODEL,
        };
        let model = get("EMBEDDING_MODEL").unwrap_or_else(|| default_model.to_string());
        let default_dims = (backend == EmbeddingBackend::OpenAi
            && model.starts_with("text-embedding-3-"))
        .then_some(DEFAULT_OPENAI_DIMENSIONS);
        let dimensions = match get("EMBEDDING_DIMENSIONS") {
            Some(value) => Some(value.parse().map_err(|_| {
                DomainError::InvalidInput(format!(
                    "EMBEDDING_DIMENSIONS={value} is not a positive integer"
                ))
            })?),
            None => default_dims,
        };
        Ok(Self {
            backend,
            model,
            dimensions,
            openai_api_key: get("OPENAI_EMBEDDING_API_KEY"),
        })
    }
}

/// The unit-length copy of `v`, or `None` for a vector that cannot be one:
/// all zeros, or a non-finite norm (NaN, infinity, or an overflowing sum).
fn l2_normalize(mut v: Vec<f32>) -> Option<Vec<f32>> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if !norm.is_finite() || norm == 0.0 {
        return None;
    }
    v.iter_mut().for_each(|x| *x /= norm);
    Some(v)
}

fn check_indexed(dims: usize) -> Result<(), DomainError> {
    if INDEXED_DIMENSIONS.contains(&dims) {
        return Ok(());
    }
    Err(DomainError::InvalidInput(format!(
        "{dims}-dimension embeddings have no search index; set EMBEDDING_DIMENSIONS \
         (and EMBEDDING_MODEL) to a size the schema indexes: 1024 or 384"
    )))
}

/// The key is the operator's: a rejected or rotated key is fixed by the
/// operator, so 401 and 403 stay retryable instead of failing bookmarks for
/// good. Other statuses map as for `LlmClient`.
fn status_error(status: StatusCode) -> DomainError {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            DomainError::Internal(format!("{VENDOR} API returned HTTP {status}"))
        }
        _ => http_status_error(VENDOR, status),
    }
}

/// `embed` returns no usage with an error, so tokens already billed for a
/// failed call (an unusable answer, or earlier batches) are logged: counts
/// only, never the inputs.
fn log_billed_failure(usage_tokens: u32, error: &DomainError) {
    if usage_tokens > 0 {
        tracing::warn!(
            vendor = VENDOR,
            usage_tokens,
            "billed embedding call failed: {error}"
        );
    }
}

fn reject_empty(inputs: &[String]) -> Result<(), DomainError> {
    if inputs.iter().any(|input| input.trim().is_empty()) {
        return Err(DomainError::InvalidInput(
            "embedding inputs must not be empty".into(),
        ));
    }
    Ok(())
}

/// Builds the backend named by `EMBEDDING_BACKEND` and delegates to it.
pub struct EmbedderRouter {
    inner: Arc<dyn Embedder>,
}

impl EmbedderRouter {
    /// Reads `EmbeddingSettings` from the environment. Errors on a
    /// configuration that cannot work, so a deployment fails at startup
    /// instead of on the first bookmark.
    pub fn from_env() -> Result<Self, DomainError> {
        Self::from_settings(&EmbeddingSettings::from_env()?)
    }

    pub fn from_settings(settings: &EmbeddingSettings) -> Result<Self, DomainError> {
        let inner: Arc<dyn Embedder> = match settings.backend {
            EmbeddingBackend::Disabled => Arc::new(DisabledEmbedder),
            EmbeddingBackend::OpenAi => {
                let key = settings.openai_api_key.clone().ok_or_else(|| {
                    DomainError::InvalidInput(
                        "EMBEDDING_BACKEND=openai needs OPENAI_EMBEDDING_API_KEY".into(),
                    )
                })?;
                let embedder = OpenAiEmbedder::new(key, &settings.model, settings.dimensions)?;
                check_indexed(embedder.dims())?;
                Arc::new(embedder)
            }
            #[cfg(feature = "local-embeddings")]
            EmbeddingBackend::Local => {
                // Checked before loading, which downloads the model.
                let native = FastembedEmbedder::native_dims(&settings.model);
                if settings.dimensions.is_some_and(|dims| dims != native) {
                    return Err(DomainError::InvalidInput(
                        "EMBEDDING_DIMENSIONS does not match the local model".into(),
                    ));
                }
                if native != 0 {
                    check_indexed(native)?;
                }
                Arc::new(FastembedEmbedder::new(&settings.model)?)
            }
            #[cfg(not(feature = "local-embeddings"))]
            EmbeddingBackend::Local => {
                return Err(DomainError::InvalidInput(
                    "EMBEDDING_BACKEND=local needs a build with --features local-embeddings".into(),
                ));
            }
        };
        Ok(Self { inner })
    }
}

impl Embedder for EmbedderRouter {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }

    fn dims(&self) -> usize {
        self.inner.dims()
    }

    fn enabled(&self) -> bool {
        self.inner.enabled()
    }

    fn embed(
        &self,
        inputs: Vec<String>,
        kind: EmbedKind,
    ) -> BoxFuture<'_, Result<Embeddings, DomainError>> {
        self.inner.embed(inputs, kind)
    }
}

/// `EMBEDDING_BACKEND=disabled`: full-text search only.
pub struct DisabledEmbedder;

impl Embedder for DisabledEmbedder {
    fn model_id(&self) -> &str {
        "disabled"
    }

    fn dims(&self) -> usize {
        0
    }

    fn enabled(&self) -> bool {
        false
    }

    fn embed(
        &self,
        _inputs: Vec<String>,
        _kind: EmbedKind,
    ) -> BoxFuture<'_, Result<Embeddings, DomainError>> {
        Box::pin(async { Err(DomainError::InvalidInput("embeddings are disabled".into())) })
    }
}

/// OpenAI `/v1/embeddings`. `text-embedding-3-*` models take a `dimensions`
/// parameter (shortened Matryoshka output); production stores
/// text-embedding-3-large at 1,024 dimensions.
#[derive(Clone)]
pub struct OpenAiEmbedder {
    client: reqwest::Client,
    url: String,
    api_key: String,
    model: String,
    dims: usize,
    /// Sent as `dimensions` only when it differs from the native size.
    shortened: bool,
    model_id: String,
    max_inputs: usize,
}

impl OpenAiEmbedder {
    pub fn new(api_key: String, model: &str, dims: Option<usize>) -> Result<Self, DomainError> {
        Self::with_url(OPENAI_EMBEDDINGS_URL, api_key, model, dims)
    }

    fn with_url(
        url: &str,
        api_key: String,
        model: &str,
        dims: Option<usize>,
    ) -> Result<Self, DomainError> {
        let native = match model {
            "text-embedding-3-small" | "text-embedding-ada-002" => 1536,
            "text-embedding-3-large" => 3072,
            other => {
                return Err(DomainError::InvalidInput(format!(
                    "unknown OpenAI embedding model {other}"
                )));
            }
        };
        let dims = dims.unwrap_or(native);
        if dims == 0 || dims > native || (dims != native && !model.starts_with("text-embedding-3"))
        {
            return Err(DomainError::InvalidInput(format!(
                "{model} cannot produce {dims}-dimension vectors"
            )));
        }
        Ok(Self {
            client: reqwest::Client::new(),
            url: url.to_string(),
            api_key,
            model: model.to_string(),
            dims,
            shortened: dims != native,
            model_id: format!("openai:{model}@{dims}"),
            max_inputs: OPENAI_MAX_INPUTS,
        })
    }

    /// Consecutive index ranges under both the input-count and size limits.
    fn batches(&self, inputs: &[String]) -> Vec<Range<usize>> {
        let mut ranges = Vec::new();
        let (mut start, mut bytes) = (0, 0);
        for (i, input) in inputs.iter().enumerate() {
            if i > start
                && (i - start == self.max_inputs || bytes + input.len() > OPENAI_MAX_BATCH_BYTES)
            {
                ranges.push(start..i);
                (start, bytes) = (i, 0);
            }
            bytes += input.len();
        }
        if start < inputs.len() {
            ranges.push(start..inputs.len());
        }
        ranges
    }

    async fn embed_batch(
        &self,
        batch: &[String],
        timeout: Duration,
    ) -> Result<(Vec<Vec<f32>>, u32), DomainError> {
        let mut body = json!({"model": self.model, "input": batch, "encoding_format": "float"});
        if self.shortened {
            body["dimensions"] = json!(self.dims);
        }
        let response = self
            .client
            .post(&self.url)
            .bearer_auth(&self.api_key)
            .timeout(timeout)
            .json(&body)
            .send()
            .await
            .map_err(|error| transport_error(VENDOR, &error))?;
        let status = response.status();
        if !status.is_success() {
            return Err(status_error(status));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|error| transport_error(VENDOR, &error))?;
        let payload: Value = serde_json::from_slice(&bytes).map_err(|_| {
            DomainError::Internal(format!("{VENDOR} returned a malformed response"))
        })?;
        let tokens = payload
            .pointer("/usage/total_tokens")
            .and_then(Value::as_u64)
            .map_or(0, |tokens| u32::try_from(tokens).unwrap_or(u32::MAX));
        let vectors = self
            .parse_vectors(&payload, batch.len())
            .inspect_err(|error| log_billed_failure(tokens, error))?;
        Ok((vectors, tokens))
    }

    /// One normalised vector per input in input order, or an error if the
    /// response does not cover the batch exactly with `dims`-long vectors.
    fn parse_vectors(
        &self,
        payload: &Value,
        expected: usize,
    ) -> Result<Vec<Vec<f32>>, DomainError> {
        let mismatch =
            || DomainError::Internal(format!("{VENDOR} response did not match the request"));
        let rows = payload
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(mismatch)?;
        if rows.len() != expected {
            return Err(mismatch());
        }
        let mut slots: Vec<Option<Vec<f32>>> = vec![None; expected];
        for row in rows {
            let index = row
                .get("index")
                .and_then(Value::as_u64)
                .and_then(|index| usize::try_from(index).ok())
                .filter(|index| *index < expected)
                .ok_or_else(mismatch)?;
            let vector = row
                .get("embedding")
                .and_then(Value::as_array)
                .ok_or_else(mismatch)?
                .iter()
                .map(|x| x.as_f64().map(|x| x as f32).filter(|x| x.is_finite()))
                .collect::<Option<Vec<f32>>>()
                .filter(|vector| vector.len() == self.dims)
                .ok_or_else(mismatch)?;
            let vector = l2_normalize(vector).ok_or_else(mismatch)?;
            if slots[index].replace(vector).is_some() {
                return Err(mismatch());
            }
        }
        // Every index was seen exactly once, because the counts match and
        // duplicates were rejected.
        Ok(slots.into_iter().flatten().collect())
    }
}

impl Embedder for OpenAiEmbedder {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn dims(&self) -> usize {
        self.dims
    }

    fn embed(
        &self,
        inputs: Vec<String>,
        kind: EmbedKind,
    ) -> BoxFuture<'_, Result<Embeddings, DomainError>> {
        Box::pin(async move {
            reject_empty(&inputs)?;
            let timeout = match kind {
                EmbedKind::Query => QUERY_TIMEOUT,
                EmbedKind::Document => DOCUMENT_TIMEOUT,
            };
            let mut out = Embeddings::default();
            for range in self.batches(&inputs) {
                let (vectors, tokens) = self
                    .embed_batch(&inputs[range], timeout)
                    .await
                    .inspect_err(|error| log_billed_failure(out.usage_tokens, error))?;
                out.vectors.extend(vectors);
                out.usage_tokens = out.usage_tokens.saturating_add(tokens);
            }
            Ok(out)
        })
    }
}

/// Local ONNX models through fastembed, on the blocking pool. The model files
/// are downloaded from Hugging Face on first use (cache dir:
/// `FASTEMBED_CACHE_DIR`), and ONNX Runtime is loaded from `ORT_DYLIB_PATH`.
#[cfg(feature = "local-embeddings")]
pub struct FastembedEmbedder {
    /// fastembed's `embed` takes `&mut self`.
    model: Arc<std::sync::Mutex<fastembed::TextEmbedding>>,
    model_id: String,
    dims: usize,
    query_prefix: &'static str,
}

#[cfg(feature = "local-embeddings")]
impl FastembedEmbedder {
    const BATCH_SIZE: usize = 64;

    /// fastembed lists ONNX re-exports (`Xenova/bge-small-en-v1.5`,
    /// `Qdrant/all-MiniLM-L6-v2-onnx`); match on the model name so the
    /// upstream name (`BAAI/bge-small-en-v1.5`) works too.
    fn find(model_code: &str) -> Option<fastembed::ModelInfo<fastembed::EmbeddingModel>> {
        let name = |code: &str| {
            let base = code.rsplit('/').next().unwrap_or(code).to_ascii_lowercase();
            base.trim_end_matches("-onnx").to_string()
        };
        fastembed::TextEmbedding::list_supported_models()
            .into_iter()
            .find(|info| name(&info.model_code) == name(model_code))
    }

    /// The vector length of a supported model; 0 if fastembed lacks it.
    fn native_dims(model_code: &str) -> usize {
        Self::find(model_code).map_or(0, |info| info.dim)
    }

    /// `model_code` is the Hugging Face name, e.g. `BAAI/bge-small-en-v1.5`.
    pub fn new(model_code: &str) -> Result<Self, DomainError> {
        use fastembed::{TextEmbedding, TextInitOptions};

        let info = Self::find(model_code).ok_or_else(|| {
            DomainError::InvalidInput(format!("fastembed does not support {model_code}"))
        })?;
        let mut options =
            TextInitOptions::new(info.model.clone()).with_show_download_progress(false);
        if let Ok(dir) = std::env::var("FASTEMBED_CACHE_DIR") {
            options = options.with_cache_dir(dir.into());
        }
        let model = TextEmbedding::try_new(options)
            .map_err(|e| DomainError::Internal(format!("fastembed failed to load: {e}")))?;
        let query_prefix = if model_code.to_ascii_lowercase().contains("bge") {
            "Represent this sentence for searching relevant passages: "
        } else {
            ""
        };
        Ok(Self {
            model: Arc::new(std::sync::Mutex::new(model)),
            model_id: format!("fastembed:{model_code}@{}", info.dim),
            dims: info.dim,
            query_prefix,
        })
    }
}

#[cfg(feature = "local-embeddings")]
impl Embedder for FastembedEmbedder {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn dims(&self) -> usize {
        self.dims
    }

    fn embed(
        &self,
        inputs: Vec<String>,
        kind: EmbedKind,
    ) -> BoxFuture<'_, Result<Embeddings, DomainError>> {
        let model = self.model.clone();
        let prefix = match kind {
            EmbedKind::Query => self.query_prefix,
            EmbedKind::Document => "",
        };
        Box::pin(async move {
            reject_empty(&inputs)?;
            let inputs: Vec<String> = inputs.into_iter().map(|t| format!("{prefix}{t}")).collect();
            let vectors = tokio::task::spawn_blocking(move || {
                model
                    .lock()
                    .map_err(|_| DomainError::Internal("fastembed model lock poisoned".into()))?
                    .embed(&inputs, Some(Self::BATCH_SIZE))
                    .map_err(|e| DomainError::Internal(format!("fastembed failed: {e}")))
            })
            .await
            .map_err(|_| DomainError::Internal("fastembed task panicked".into()))??;
            let vectors = vectors
                .into_iter()
                .map(l2_normalize)
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| {
                    DomainError::Internal("fastembed returned an unusable vector".into())
                })?;
            Ok(Embeddings {
                vectors,
                usage_tokens: 0,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, extract::State, http::HeaderMap, http::StatusCode, routing::post};
    use std::sync::Mutex;
    use tokio::net::TcpListener;

    const SECRET_BODY: &str = "secret detail from the vendor";

    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<Value>>>);

    /// Fake vendor: returns `[len, 1, 0, 0]` per input, where `len` is the
    /// input's length, with the rows in reverse order (the adapter must place
    /// them by `index`).
    async fn fake_embeddings(
        State(captured): State<Captured>,
        headers: HeaderMap,
        body: String,
    ) -> (StatusCode, String) {
        let request: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(headers["authorization"], "Bearer sk-test");
        captured.0.lock().unwrap().push(request.clone());
        let inputs = request["input"].as_array().unwrap();
        let mut data: Vec<Value> = inputs
            .iter()
            .enumerate()
            .map(|(i, input)| {
                let len = input.as_str().unwrap().len() as f64;
                json!({"index": i, "embedding": [len, 1.0, 0.0, 0.0]})
            })
            .collect();
        data.reverse();
        let usage = json!({"prompt_tokens": inputs.len() * 3, "total_tokens": inputs.len() * 3});
        (
            StatusCode::OK,
            json!({"data": data, "usage": usage}).to_string(),
        )
    }

    async fn server_for(router: Router) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{address}/v1/embeddings")
    }

    /// A vendor that answers every request with `status` and `body`.
    async fn fixed_vendor(status: u16, body: String) -> OpenAiEmbedder {
        let reply = move || {
            let body = body.clone();
            async move { (StatusCode::from_u16(status).unwrap(), body) }
        };
        let url = server_for(Router::new().route("/v1/embeddings", post(reply))).await;
        OpenAiEmbedder::with_url(&url, "sk-test".into(), "text-embedding-3-small", Some(4)).unwrap()
    }

    async fn fake_vendor_embedder(dims: usize) -> (OpenAiEmbedder, Captured) {
        let captured = Captured::default();
        let router = Router::new()
            .route("/v1/embeddings", post(fake_embeddings))
            .with_state(captured.clone());
        let url = server_for(router).await;
        // The fake vendor returns 4-d vectors: model them as a shortened
        // text-embedding-3-small.
        let embedder =
            OpenAiEmbedder::with_url(&url, "sk-test".into(), "text-embedding-3-small", Some(dims))
                .unwrap();
        (embedder, captured)
    }

    fn inputs(texts: &[&str]) -> Vec<String> {
        texts.iter().map(|t| t.to_string()).collect()
    }

    fn lookup(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |key| {
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value.to_string())
        }
    }

    fn assert_error(result: Result<Embeddings, DomainError>, retryable: bool, needle: &str) {
        let message = match (result, retryable) {
            (Err(DomainError::Internal(message)), true) => message,
            (Err(DomainError::InvalidInput(message)), false) => message,
            (other, _) => panic!("expected retryable={retryable}, got {other:?}"),
        };
        assert!(message.contains(needle), "{message}");
        assert!(!message.contains(SECRET_BODY));
        assert!(!message.contains("sk-test"));
    }

    #[test]
    fn settings_default_to_disabled_and_read_openai_config() {
        let none = EmbeddingSettings::from_lookup(lookup(&[])).unwrap();
        assert_eq!(none.backend, EmbeddingBackend::Disabled);

        let openai = EmbeddingSettings::from_lookup(lookup(&[
            ("EMBEDDING_BACKEND", "OpenAI"),
            ("EMBEDDING_DIMENSIONS", "512"),
            ("OPENAI_EMBEDDING_API_KEY", "sk-op"),
        ]))
        .unwrap();
        assert_eq!(openai.backend, EmbeddingBackend::OpenAi);
        assert_eq!(openai.model, "text-embedding-3-large");
        assert_eq!(openai.dimensions, Some(512));
        assert_eq!(openai.openai_api_key.as_deref(), Some("sk-op"));
        assert!(!format!("{openai:?}").contains("sk-op"));

        let openai_default =
            EmbeddingSettings::from_lookup(lookup(&[("EMBEDDING_BACKEND", "openai")])).unwrap();
        assert_eq!(openai_default.model, "text-embedding-3-large");
        assert_eq!(openai_default.dimensions, Some(1024));

        // Naming the model, even the default one, keeps the indexed size.
        for model in ["text-embedding-3-large", "text-embedding-3-small"] {
            let named = EmbeddingSettings::from_lookup(move |key| match key {
                "EMBEDDING_BACKEND" => Some("openai".into()),
                "EMBEDDING_MODEL" => Some(model.into()),
                _ => None,
            })
            .unwrap();
            assert_eq!(named.dimensions, Some(1024), "{model}");
        }
        let ada = EmbeddingSettings::from_lookup(lookup(&[
            ("EMBEDDING_BACKEND", "openai"),
            ("EMBEDDING_MODEL", "text-embedding-ada-002"),
        ]))
        .unwrap();
        assert_eq!(ada.dimensions, None);

        for name in ["local", "fastembed"] {
            let local = EmbeddingSettings::from_lookup(move |key| {
                (key == "EMBEDDING_BACKEND").then(|| name.to_string())
            })
            .unwrap();
            assert_eq!(local.backend, EmbeddingBackend::Local);
            assert_eq!(local.model, "BAAI/bge-small-en-v1.5");
        }

        assert!(
            EmbeddingSettings::from_lookup(lookup(&[("EMBEDDING_BACKEND", "voyage")])).is_err()
        );
        assert!(
            EmbeddingSettings::from_lookup(lookup(&[
                ("EMBEDDING_BACKEND", "openai"),
                ("EMBEDDING_DIMENSIONS", "lots"),
            ]))
            .is_err()
        );
    }

    #[tokio::test]
    async fn router_builds_the_configured_backend() {
        let disabled =
            EmbedderRouter::from_settings(&EmbeddingSettings::from_lookup(lookup(&[])).unwrap())
                .unwrap();
        assert!(!disabled.enabled());
        assert_eq!(disabled.dims(), 0);
        assert_eq!(disabled.model_id(), "disabled");
        assert!(matches!(
            disabled.embed(inputs(&["q"]), EmbedKind::Query).await,
            Err(DomainError::InvalidInput(_))
        ));

        let mut settings = EmbeddingSettings {
            backend: EmbeddingBackend::OpenAi,
            model: "text-embedding-3-small".into(),
            dimensions: Some(1024),
            openai_api_key: None,
        };
        assert!(
            EmbedderRouter::from_settings(&settings).is_err(),
            "key is required"
        );
        settings.openai_api_key = Some("sk-op".into());
        let openai = EmbedderRouter::from_settings(&settings).unwrap();
        assert!(openai.enabled());
        assert_eq!(openai.model_id(), "openai:text-embedding-3-small@1024");
        assert_eq!(openai.dims(), 1024);

        // Sizes the schema has no index for are refused at startup.
        for dims in [Some(512), Some(3072)] {
            settings.model = "text-embedding-3-large".into();
            settings.dimensions = dims;
            assert!(matches!(
                EmbedderRouter::from_settings(&settings),
                Err(DomainError::InvalidInput(message)) if message.contains("no search index")
            ));
        }
        settings.model = "text-embedding-ada-002".into();
        settings.dimensions = None;
        assert!(EmbedderRouter::from_settings(&settings).is_err(), "1536");

        let production = EmbedderRouter::from_settings(
            &EmbeddingSettings::from_lookup(lookup(&[
                ("EMBEDDING_BACKEND", "openai"),
                ("OPENAI_EMBEDDING_API_KEY", "sk-op"),
            ]))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(production.model_id(), "openai:text-embedding-3-large@1024");
    }

    #[cfg(not(feature = "local-embeddings"))]
    #[test]
    fn router_refuses_local_without_the_feature() {
        let settings =
            EmbeddingSettings::from_lookup(lookup(&[("EMBEDDING_BACKEND", "local")])).unwrap();
        assert!(matches!(
            EmbedderRouter::from_settings(&settings),
            Err(DomainError::InvalidInput(message)) if message.contains("local-embeddings")
        ));
    }

    #[test]
    fn openai_rejects_impossible_dimensions() {
        let key = || "sk".to_string();
        assert!(OpenAiEmbedder::new(key(), "text-embedding-3-small", Some(4096)).is_err());
        assert!(OpenAiEmbedder::new(key(), "text-embedding-3-small", Some(0)).is_err());
        assert!(OpenAiEmbedder::new(key(), "text-embedding-ada-002", Some(512)).is_err());
        assert!(OpenAiEmbedder::new(key(), "unknown-model", None).is_err());
        let native = OpenAiEmbedder::new(key(), "text-embedding-3-large", None).unwrap();
        assert_eq!(native.model_id(), "openai:text-embedding-3-large@3072");
        assert!(!native.shortened);
    }

    #[tokio::test]
    async fn openai_batches_keeps_order_sums_usage_and_sends_dimensions() {
        let (mut embedder, captured) = fake_vendor_embedder(4).await;
        embedder.max_inputs = 2;

        let out = embedder
            .embed(inputs(&["a", "bbb", "cc"]), EmbedKind::Document)
            .await
            .unwrap();

        let requests = captured.0.lock().unwrap().clone();
        assert_eq!(requests.len(), 2, "3 inputs at 2 per request");
        assert_eq!(requests[0]["input"], json!(["a", "bbb"]));
        assert_eq!(requests[1]["input"], json!(["cc"]));
        assert!(requests.iter().all(|r| r["dimensions"] == 4));
        assert!(
            requests
                .iter()
                .all(|r| r["model"] == "text-embedding-3-small")
        );
        assert_eq!(out.usage_tokens, 9);
        // Each vector is [len, 1, 0, 0] normalised: unit length, and the
        // ratio of the first two components gives back the input length.
        assert_eq!(out.vectors.len(), 3);
        for (vector, len) in out.vectors.iter().zip([1.0, 3.0, 2.0]) {
            let norm = vector.iter().map(|x| x * x).sum::<f32>();
            assert!((norm - 1.0).abs() < 1e-6);
            assert!((vector[0] / vector[1] - len).abs() < 1e-5);
        }
    }

    #[test]
    fn openai_places_rows_by_index() {
        let embedder = OpenAiEmbedder::new("sk".into(), "text-embedding-3-small", Some(2)).unwrap();
        let payload = json!({"data": [
            {"index": 1, "embedding": [0.0, 2.0]},
            {"index": 0, "embedding": [3.0, 0.0]},
        ]});
        assert_eq!(
            embedder.parse_vectors(&payload, 2).unwrap(),
            vec![vec![1.0, 0.0], vec![0.0, 1.0]]
        );
        let duplicate = json!({"data": [
            {"index": 0, "embedding": [1.0, 0.0]},
            {"index": 0, "embedding": [1.0, 0.0]},
        ]});
        assert!(embedder.parse_vectors(&duplicate, 2).is_err());
        let not_numbers = json!({"data": [{"index": 0, "embedding": [1.0, "x"]}]});
        assert!(embedder.parse_vectors(&not_numbers, 1).is_err());
        for unusable in [json!([0.0, 0.0]), json!([3.0e38, 3.0e38])] {
            let payload = json!({"data": [{"index": 0, "embedding": unusable}]});
            assert!(embedder.parse_vectors(&payload, 1).is_err(), "{unusable}");
        }
    }

    #[test]
    fn batches_split_on_size_as_well_as_count() {
        let embedder = OpenAiEmbedder::new("sk".into(), "text-embedding-3-small", None).unwrap();
        let big = "x".repeat(OPENAI_MAX_BATCH_BYTES / 2 + 1);
        let texts = vec![big.clone(), big.clone(), big];
        assert_eq!(embedder.batches(&texts), vec![0..1, 1..2, 2..3]);
        assert_eq!(embedder.batches(&inputs(&["a", "b"])), vec![0..2]);
        assert!(embedder.batches(&[]).is_empty());
    }

    #[tokio::test]
    async fn openai_maps_http_errors_by_retryability_without_leaking_bodies() {
        let secret = json!({"error": {"message": SECRET_BODY}}).to_string();
        for (status, retryable) in [
            (400, false),
            // The operator's key: a rotated key must not fail work for good.
            (401, true),
            (403, true),
            (404, false),
            (422, false),
            (429, true),
            (500, true),
            (503, true),
        ] {
            let embedder = fixed_vendor(status, secret.clone()).await;
            assert_error(
                embedder.embed(inputs(&["x"]), EmbedKind::Query).await,
                retryable,
                &status.to_string(),
            );
        }
    }

    #[tokio::test]
    async fn openai_rejects_malformed_responses_and_empty_inputs() {
        let embedder = fixed_vendor(200, format!("<html>{SECRET_BODY}")).await;
        assert_error(
            embedder.embed(inputs(&["x"]), EmbedKind::Document).await,
            true,
            "malformed",
        );
        let embedder = fixed_vendor(200, json!({"data": "nope"}).to_string()).await;
        assert_error(
            embedder.embed(inputs(&["x"]), EmbedKind::Document).await,
            true,
            "did not match",
        );
        assert_error(
            embedder.embed(inputs(&["  "]), EmbedKind::Document).await,
            false,
            "empty",
        );
        assert_eq!(
            embedder
                .embed(Vec::new(), EmbedKind::Document)
                .await
                .unwrap(),
            Embeddings::default()
        );
    }

    #[tokio::test]
    async fn openai_rejects_vectors_of_the_wrong_size() {
        // Configured for 8 dims; the fake vendor answers with 4.
        let (embedder, _) = fake_vendor_embedder(8).await;
        assert_error(
            embedder.embed(inputs(&["x"]), EmbedKind::Document).await,
            true,
            "did not match",
        );
    }

    #[tokio::test]
    async fn openai_network_failures_are_retryable() {
        let address = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap();
        let embedder = OpenAiEmbedder::with_url(
            &format!("http://{address}/v1/embeddings"),
            "sk-test".into(),
            "text-embedding-3-small",
            None,
        )
        .unwrap();
        assert_error(
            embedder.embed(inputs(&["x"]), EmbedKind::Query).await,
            true,
            "request failed",
        );
    }

    /// Downloads the model and needs ONNX Runtime (`ORT_DYLIB_PATH`):
    /// `cargo test -p boopmark-server --features local-embeddings -- --ignored fastembed`
    #[cfg(feature = "local-embeddings")]
    #[tokio::test]
    #[ignore]
    async fn fastembed_bge_small_embeds_queries_and_documents() {
        let embedder = FastembedEmbedder::new("BAAI/bge-small-en-v1.5").unwrap();
        assert_eq!(embedder.model_id(), "fastembed:BAAI/bge-small-en-v1.5@384");
        let docs = embedder
            .embed(
                inputs(&["Bees dance to share where flowers are."]),
                EmbedKind::Document,
            )
            .await
            .unwrap();
        let query = embedder
            .embed(inputs(&["how do bees communicate"]), EmbedKind::Query)
            .await
            .unwrap();
        assert_eq!(docs.vectors[0].len(), 384);
        let cosine: f32 = docs.vectors[0]
            .iter()
            .zip(&query.vectors[0])
            .map(|(a, b)| a * b)
            .sum();
        assert!(cosine > 0.5, "related texts should be close, got {cosine}");
    }

    #[cfg(feature = "local-embeddings")]
    #[test]
    fn fastembed_knows_bge_small_by_its_upstream_name() {
        assert_eq!(
            FastembedEmbedder::native_dims("BAAI/bge-small-en-v1.5"),
            384
        );
        assert_eq!(FastembedEmbedder::native_dims("no-such-model"), 0);
    }
}
