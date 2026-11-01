#![allow(dead_code)]
//! Vendor adapters for the `Embedder` port, plus the router that builds the
//! one selected by configuration:
//!
//! ```text
//! EMBEDDING_BACKEND=disabled | openai | fastembed     (default: disabled)
//! EMBEDDING_MODEL=text-embedding-3-large              (fastembed default: BAAI/bge-small-en-v1.5)
//! EMBEDDING_DIMENSIONS=1024                           (openai only; default: 1024 for the default
//!                                                     model, else the model's native size)
//! OPENAI_EMBEDDING_API_KEY=sk-...                     (operator key, not a user's BYO key)
//! ```
//!
//! `fastembed` needs the `local-embeddings` cargo feature, so the default
//! build carries no ONNX Runtime.

use crate::domain::error::DomainError;
use crate::domain::ports::embedder::{BoxFuture, EmbedKind, Embedder, Embeddings};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

const OPENAI_EMBEDDINGS_URL: &str = "https://api.openai.com/v1/embeddings";
/// The API accepts 2,048 inputs and 300k tokens per request; stay well below.
const OPENAI_MAX_INPUTS: usize = 256;
const OPENAI_MAX_BATCH_CHARS: usize = 400_000;
/// A query embedding is on the search request path: past this, search runs
/// the lexical legs alone.
const QUERY_TIMEOUT: Duration = Duration::from_millis(1500);
const DOCUMENT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddingBackend {
    Disabled,
    OpenAi,
    Fastembed,
}

#[derive(Debug, Clone)]
pub struct EmbeddingSettings {
    pub backend: EmbeddingBackend,
    pub model: String,
    pub dimensions: Option<usize>,
    pub openai_api_key: Option<String>,
}

impl EmbeddingSettings {
    pub fn from_env() -> Self {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let backend = match get("EMBEDDING_BACKEND").as_deref().map(str::trim) {
            Some("openai") => EmbeddingBackend::OpenAi,
            Some("fastembed") => EmbeddingBackend::Fastembed,
            _ => EmbeddingBackend::Disabled,
        };
        let model = get("EMBEDDING_MODEL").filter(|m| !m.trim().is_empty());
        // The measured production choice: text-embedding-3-large shortened
        // to 1,024 dimensions, which matches a partial HNSW index.
        let default_dims = (backend == EmbeddingBackend::OpenAi && model.is_none()).then_some(1024);
        let default_model = match backend {
            EmbeddingBackend::Fastembed => "BAAI/bge-small-en-v1.5",
            _ => "text-embedding-3-large",
        };
        Self {
            backend,
            model: model.unwrap_or_else(|| default_model.to_string()),
            dimensions: get("EMBEDDING_DIMENSIONS")
                .and_then(|d| d.trim().parse().ok())
                .or(default_dims),
            openai_api_key: get("OPENAI_EMBEDDING_API_KEY").filter(|k| !k.trim().is_empty()),
        }
    }
}

fn l2_normalize(mut v: Vec<f32>) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
    v
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
    pub fn from_settings(settings: &EmbeddingSettings) -> Result<Self, DomainError> {
        let inner: Arc<dyn Embedder> = match settings.backend {
            EmbeddingBackend::Disabled => Arc::new(DisabledEmbedder),
            EmbeddingBackend::OpenAi => {
                let key = settings.openai_api_key.clone().ok_or_else(|| {
                    DomainError::InvalidInput(
                        "EMBEDDING_BACKEND=openai needs OPENAI_EMBEDDING_API_KEY".into(),
                    )
                })?;
                Arc::new(OpenAiEmbedder::new(
                    key,
                    &settings.model,
                    settings.dimensions,
                )?)
            }
            #[cfg(feature = "local-embeddings")]
            EmbeddingBackend::Fastembed => Arc::new(FastembedEmbedder::new(&settings.model)?),
            #[cfg(not(feature = "local-embeddings"))]
            EmbeddingBackend::Fastembed => {
                return Err(DomainError::InvalidInput(
                    "EMBEDDING_BACKEND=fastembed needs a build with --features local-embeddings"
                        .into(),
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
/// text-embedding-3-large at 1,024 dimensions as `halfvec`.
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
    fn batches(&self, inputs: &[String]) -> Vec<std::ops::Range<usize>> {
        let mut ranges = Vec::new();
        let (mut start, mut chars) = (0, 0);
        for (i, input) in inputs.iter().enumerate() {
            if i > start
                && (i - start == self.max_inputs || chars + input.len() > OPENAI_MAX_BATCH_CHARS)
            {
                ranges.push(start..i);
                (start, chars) = (i, 0);
            }
            chars += input.len();
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
            .map_err(|_| DomainError::Internal("OpenAI embeddings request failed".into()))?;
        let status = response.status();
        if status == reqwest::StatusCode::BAD_REQUEST {
            return Err(DomainError::InvalidInput(
                "OpenAI rejected the embedding request (HTTP 400)".into(),
            ));
        }
        if !status.is_success() {
            return Err(DomainError::Internal(format!(
                "OpenAI embeddings returned HTTP {status}"
            )));
        }
        let payload: Value = response
            .json()
            .await
            .map_err(|_| DomainError::Internal("OpenAI embeddings response parse failed".into()))?;
        let mut rows: Vec<(u64, Vec<f32>)> = payload["data"]
            .as_array()
            .map(|data| {
                data.iter()
                    .map(|row| {
                        let vector = row["embedding"]
                            .as_array()
                            .map(|v| {
                                v.iter()
                                    .filter_map(Value::as_f64)
                                    .map(|x| x as f32)
                                    .collect()
                            })
                            .unwrap_or_default();
                        (row["index"].as_u64().unwrap_or(u64::MAX), vector)
                    })
                    .collect()
            })
            .unwrap_or_default();
        rows.sort_by_key(|(index, _)| *index);
        let in_order = rows
            .iter()
            .enumerate()
            .all(|(i, (index, _))| *index == i as u64);
        if rows.len() != batch.len() || !in_order || rows.iter().any(|(_, v)| v.len() != self.dims)
        {
            return Err(DomainError::Internal(
                "OpenAI embeddings response did not match the request".into(),
            ));
        }
        let tokens = payload["usage"]["total_tokens"].as_u64().unwrap_or(0) as u32;
        Ok((
            rows.into_iter().map(|(_, v)| l2_normalize(v)).collect(),
            tokens,
        ))
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
                let (vectors, tokens) = self.embed_batch(&inputs[range], timeout).await?;
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
    model: Arc<std::sync::Mutex<fastembed::TextEmbedding>>,
    model_id: String,
    dims: usize,
    query_prefix: &'static str,
}

#[cfg(feature = "local-embeddings")]
impl FastembedEmbedder {
    /// `model_code` is the Hugging Face name, e.g. `BAAI/bge-small-en-v1.5`.
    pub fn new(model_code: &str) -> Result<Self, DomainError> {
        use fastembed::{TextEmbedding, TextInitOptions};

        // fastembed lists ONNX re-exports (`Xenova/bge-small-en-v1.5`,
        // `Qdrant/all-MiniLM-L6-v2-onnx`); match on the model name so the
        // upstream name (`BAAI/bge-small-en-v1.5`) works too.
        let name = |code: &str| {
            let base = code.rsplit('/').next().unwrap_or(code).to_ascii_lowercase();
            base.trim_end_matches("-onnx").to_string()
        };
        let info = TextEmbedding::list_supported_models()
            .into_iter()
            .find(|info| name(&info.model_code) == name(model_code))
            .ok_or_else(|| {
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
                    .embed(&inputs, Some(64))
                    .map_err(|e| DomainError::Internal(format!("fastembed failed: {e}")))
            })
            .await
            .map_err(|_| DomainError::Internal("fastembed task panicked".into()))??;
            Ok(Embeddings {
                vectors: vectors.into_iter().map(l2_normalize).collect(),
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

    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<Value>>>);

    /// Fake vendor: returns a 4-d vector per input whose first component is
    /// the input's length, with the rows in reverse order (the adapter must
    /// sort by `index`).
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
                json!({"index": i, "embedding": [len, 0.0, 0.0, 0.0]})
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

    fn inputs(texts: &[&str]) -> Vec<String> {
        texts.iter().map(|t| t.to_string()).collect()
    }

    #[test]
    fn settings_default_to_disabled_and_read_openai_config() {
        let none = EmbeddingSettings::from_lookup(|_| None);
        assert_eq!(none.backend, EmbeddingBackend::Disabled);

        let openai = EmbeddingSettings::from_lookup(|key| match key {
            "EMBEDDING_BACKEND" => Some("openai".into()),
            "EMBEDDING_DIMENSIONS" => Some("512".into()),
            "OPENAI_EMBEDDING_API_KEY" => Some("sk-op".into()),
            _ => None,
        });
        assert_eq!(openai.backend, EmbeddingBackend::OpenAi);
        assert_eq!(openai.model, "text-embedding-3-large");
        assert_eq!(openai.dimensions, Some(512));

        let openai_default = EmbeddingSettings::from_lookup(|key| {
            (key == "EMBEDDING_BACKEND").then(|| "openai".to_string())
        });
        assert_eq!(openai_default.model, "text-embedding-3-large");
        assert_eq!(openai_default.dimensions, Some(1024));

        let small_native = EmbeddingSettings::from_lookup(|key| match key {
            "EMBEDDING_BACKEND" => Some("openai".into()),
            "EMBEDDING_MODEL" => Some("text-embedding-3-small".into()),
            _ => None,
        });
        assert_eq!(small_native.dimensions, None);

        let local = EmbeddingSettings::from_lookup(|key| {
            (key == "EMBEDDING_BACKEND").then(|| "fastembed".to_string())
        });
        assert_eq!(local.model, "BAAI/bge-small-en-v1.5");
    }

    #[tokio::test]
    async fn router_builds_the_configured_backend() {
        let disabled =
            EmbedderRouter::from_settings(&EmbeddingSettings::from_lookup(|_| None)).unwrap();
        assert!(!disabled.enabled());
        assert_eq!(disabled.dims(), 0);
        assert!(matches!(
            disabled.embed(inputs(&["q"]), EmbedKind::Query).await,
            Err(DomainError::InvalidInput(_))
        ));

        let mut settings = EmbeddingSettings {
            backend: EmbeddingBackend::OpenAi,
            model: "text-embedding-3-small".into(),
            dimensions: Some(512),
            openai_api_key: None,
        };
        assert!(
            EmbedderRouter::from_settings(&settings).is_err(),
            "key is required"
        );
        settings.openai_api_key = Some("sk-op".into());
        let openai = EmbedderRouter::from_settings(&settings).unwrap();
        assert!(openai.enabled());
        assert_eq!(openai.model_id(), "openai:text-embedding-3-small@512");
        assert_eq!(openai.dims(), 512);
    }

    #[test]
    fn openai_rejects_impossible_dimensions() {
        let key = || "sk".to_string();
        assert!(OpenAiEmbedder::new(key(), "text-embedding-3-small", Some(4096)).is_err());
        assert!(OpenAiEmbedder::new(key(), "text-embedding-ada-002", Some(512)).is_err());
        assert!(OpenAiEmbedder::new(key(), "unknown-model", None).is_err());
        let native = OpenAiEmbedder::new(key(), "text-embedding-3-large", None).unwrap();
        assert_eq!(native.model_id(), "openai:text-embedding-3-large@3072");
    }

    #[tokio::test]
    async fn openai_batches_keeps_order_sums_usage_and_sends_dimensions() {
        let captured = Captured::default();
        let router = Router::new()
            .route("/v1/embeddings", post(fake_embeddings))
            .with_state(captured.clone());
        let url = server_for(router).await;
        // The fake vendor returns 4-d vectors: model them as a shortened
        // text-embedding-3-small.
        let mut embedder =
            OpenAiEmbedder::with_url(&url, "sk-test".into(), "text-embedding-3-small", Some(4))
                .unwrap();
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
        assert_eq!(out.usage_tokens, 9);
        // Normalised, so each vector is [1, 0, 0, 0]; order is checked
        // through the batches above.
        assert_eq!(out.vectors.len(), 3);
        assert!(out.vectors.iter().all(|v| v == &vec![1.0, 0.0, 0.0, 0.0]));
    }

    #[tokio::test]
    async fn openai_maps_errors_without_leaking_bodies() {
        async fn unauthorized() -> (StatusCode, &'static str) {
            (StatusCode::UNAUTHORIZED, "{\"error\":\"secret detail\"}")
        }
        async fn bad_request() -> (StatusCode, &'static str) {
            (StatusCode::BAD_REQUEST, "{\"error\":\"too long\"}")
        }
        let url = server_for(Router::new().route("/v1/embeddings", post(unauthorized))).await;
        let embedder =
            OpenAiEmbedder::with_url(&url, "sk-test".into(), "text-embedding-3-small", None)
                .unwrap();
        match embedder.embed(inputs(&["x"]), EmbedKind::Query).await {
            Err(DomainError::Internal(message)) => {
                assert!(message.contains("401"));
                assert!(!message.contains("secret"));
            }
            other => panic!("expected Internal, got {other:?}"),
        }

        let url = server_for(Router::new().route("/v1/embeddings", post(bad_request))).await;
        let embedder =
            OpenAiEmbedder::with_url(&url, "sk-test".into(), "text-embedding-3-small", None)
                .unwrap();
        assert!(matches!(
            embedder.embed(inputs(&["x"]), EmbedKind::Document).await,
            Err(DomainError::InvalidInput(_))
        ));
        assert!(matches!(
            embedder.embed(inputs(&["  "]), EmbedKind::Document).await,
            Err(DomainError::InvalidInput(_))
        ));
    }

    #[tokio::test]
    async fn openai_rejects_vectors_of_the_wrong_size() {
        let captured = Captured::default();
        let router = Router::new()
            .route("/v1/embeddings", post(fake_embeddings))
            .with_state(captured);
        let url = server_for(router).await;
        // Configured for 8 dims; the fake vendor answers with 4.
        let embedder =
            OpenAiEmbedder::with_url(&url, "sk-test".into(), "text-embedding-3-small", Some(8))
                .unwrap();
        assert!(matches!(
            embedder.embed(inputs(&["x"]), EmbedKind::Document).await,
            Err(DomainError::Internal(_))
        ));
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
}
