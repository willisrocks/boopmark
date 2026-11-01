use crate::domain::chunking::ChunkSource;
use crate::domain::error::DomainError;
use crate::domain::ports::embedder::{EmbedKind, Embedder};
use crate::domain::ports::embedding_repo::EmbeddingRepository;
use crate::domain::search::{EmbedJob, EmbeddedChunk, embed_retry_delay};
use chrono::{DateTime, Utc};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::Instant;

/// Rows claimed at a time; each is embedded with its own call.
const CLAIM_BATCH: i64 = 8;
/// A claimed row is invisible to other claims for this long, well past
/// `CLAIM_BATCH` times `EMBED_TIMEOUT`.
const CLAIM_LEASE: Duration = Duration::from_secs(30 * 60);
/// One bookmark's embedding gives up after this long. Hosted adapters time
/// out after 30 s; a local model on a slow CPU has no timeout of its own.
const EMBED_TIMEOUT: Duration = Duration::from_secs(2 * 60);
/// The rate limit: at least this long between two embedding calls, so at
/// most 5 bookmarks (about 50 chunks) a second. The production backfill of
/// a few hundred bookmarks takes a minute or two, far inside vendor limits.
const MIN_CALL_INTERVAL: Duration = Duration::from_millis(200);
/// How long the worker sleeps on an empty queue, unless the ingest worker
/// wakes it with a new result. Edits that queue a re-embed wait for this.
const IDLE_POLL: Duration = Duration::from_secs(5);
const MAX_ERROR_CHARS: usize = 500;

/// Builds and stores the chunks of every bookmark whose chunks were not made
/// by the configured embedder: new articles, the metadata chunk of failed
/// fetches, edits, and (because all of that state is in the database) the
/// backfill after migration 018 or a model change. Rate-limited, resumable
/// and idempotent: a bookmark's chunks are replaced as a whole.
pub struct EmbedWorker<R> {
    repo: Arc<R>,
    embedder: Arc<dyn Embedder>,
    min_call_interval: Duration,
}

/// What became of one claimed bookmark.
#[derive(Debug, Clone, PartialEq)]
enum Outcome {
    Embedded {
        chunks: Vec<EmbeddedChunk>,
        usage_tokens: u32,
    },
    Failed {
        error: String,
        /// `None`: final for this model.
        retry_at: Option<DateTime<Utc>>,
    },
}

impl<R: EmbeddingRepository + 'static> EmbedWorker<R> {
    pub fn new(repo: Arc<R>, embedder: Arc<dyn Embedder>) -> Self {
        Self {
            repo,
            embedder,
            min_call_interval: MIN_CALL_INTERVAL,
        }
    }

    /// Run forever on a background task; `wake` cuts the idle sleep short.
    /// With embeddings disabled there is nothing to do and nothing starts.
    pub fn spawn(self, wake: Arc<Notify>) {
        if !self.embedder.enabled() {
            return;
        }
        tokio::spawn(async move { self.run(wake).await });
    }

    async fn run(self, wake: Arc<Notify>) {
        let model_id = self.embedder.model_id().to_string();
        tracing::info!(model = %model_id, "embedding worker started");
        let mut next_call = Instant::now();
        let mut total_tokens: u64 = 0;
        loop {
            let jobs = self
                .repo
                .claim_embed_batch(&model_id, CLAIM_BATCH, CLAIM_LEASE)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "embedding: claiming jobs failed");
                    Vec::new()
                });
            if jobs.is_empty() {
                tokio::select! {
                    _ = wake.notified() => {}
                    _ = tokio::time::sleep(IDLE_POLL) => {}
                }
                continue;
            }
            for job in jobs {
                tokio::time::sleep_until(next_call).await;
                next_call = Instant::now() + self.min_call_interval;
                let outcome = self.embed(&job).await;
                if let Outcome::Embedded {
                    chunks,
                    usage_tokens,
                } = &outcome
                {
                    // Spend, in tokens only; never the text.
                    total_tokens += u64::from(*usage_tokens);
                    tracing::info!(
                        bookmark_id = %job.bookmark_id,
                        model = %model_id,
                        chunks = chunks.len(),
                        usage_tokens,
                        total_usage_tokens = total_tokens,
                        "embedded bookmark"
                    );
                }
                self.record(&job, &model_id, &outcome).await;
            }
        }
    }

    /// Chunk the bookmark and embed all of its chunks in one call.
    async fn embed(&self, job: &EmbedJob) -> Outcome {
        let chunks = ChunkSource {
            title: job.title.as_deref(),
            description: job.description.as_deref(),
            url: &job.url,
            tags: &job.tags,
            text: job.text.as_deref(),
        }
        .chunks();
        let inputs = chunks.iter().map(|c| c.input.clone()).collect();
        let embedded = tokio::time::timeout(
            EMBED_TIMEOUT,
            self.embedder.embed(inputs, EmbedKind::Document),
        )
        .await
        .unwrap_or_else(|_| Err(DomainError::Internal("embedding timed out".into())))
        .and_then(|embeddings| {
            if embeddings.vectors.len() != chunks.len() {
                return Err(DomainError::Internal(
                    "the embedder returned the wrong number of vectors".into(),
                ));
            }
            Ok(embeddings)
        });
        match embedded {
            Ok(embeddings) => Outcome::Embedded {
                chunks: chunks
                    .into_iter()
                    .zip(embeddings.vectors)
                    .map(|(chunk, embedding)| EmbeddedChunk {
                        text: chunk.text,
                        embedding,
                    })
                    .collect(),
                usage_tokens: embeddings.usage_tokens,
            },
            Err(error) => Outcome::Failed {
                retry_at: match error {
                    // The vendor will never accept it.
                    DomainError::InvalidInput(_) => None,
                    _ => Some(Utc::now() + embed_retry_delay(job.embed_attempts)),
                },
                error: error.to_string().chars().take(MAX_ERROR_CHARS).collect(),
            },
        }
    }

    async fn record(&self, job: &EmbedJob, model_id: &str, outcome: &Outcome) {
        let stored = match outcome {
            Outcome::Embedded { chunks, .. } => self.repo.store_chunks(job, model_id, chunks).await,
            Outcome::Failed { error, retry_at } => {
                tracing::warn!(
                    bookmark_id = %job.bookmark_id,
                    attempts = job.embed_attempts,
                    retry_at = ?retry_at,
                    "embedding failed: {error}"
                );
                self.repo
                    .record_embed_failure(job, model_id, error, *retry_at)
                    .await
            }
        };
        match stored {
            Ok(true) => {}
            // Edited, refetched or deleted since the claim; the next claim
            // embeds the new version.
            Ok(false) => {
                tracing::debug!(bookmark_id = %job.bookmark_id, "embedding result dropped")
            }
            Err(e) => {
                tracing::warn!(bookmark_id = %job.bookmark_id, error = %e, "storing embeddings failed")
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::domain::chunking::MIN_CHUNKED_TEXT_CHARS;
    use crate::domain::ports::embedder::{BoxFuture, Embeddings};
    use std::sync::Mutex;
    use uuid::Uuid;

    /// A deterministic embedder for tests: each input's tokens are hashed
    /// into a fixed-size vector, so texts that share words point the same
    /// way. Inputs starting with `fail:` or `reject:` fail transiently or for
    /// good.
    pub(crate) struct StubEmbedder {
        pub dims: usize,
        pub model_id: String,
        pub calls: Mutex<Vec<(Vec<String>, EmbedKind, Instant)>>,
        /// Each call waits this long first.
        pub delay: Duration,
    }

    impl StubEmbedder {
        pub(crate) fn new(dims: usize) -> Self {
            Self {
                dims,
                model_id: format!("stub:hash@{dims}"),
                calls: Mutex::new(Vec::new()),
                delay: Duration::ZERO,
            }
        }

        pub(crate) fn vector(&self, text: &str) -> Vec<f32> {
            let mut v = vec![0f32; self.dims];
            for word in text
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
            {
                let hash = word
                    .to_lowercase()
                    .bytes()
                    .fold(0xcbf29ce484222325u64, |h, b| {
                        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
                    });
                v[(hash % self.dims as u64) as usize] += 1.0;
            }
            let norm = v
                .iter()
                .map(|x| x * x)
                .sum::<f32>()
                .sqrt()
                .max(f32::MIN_POSITIVE);
            v.iter_mut().for_each(|x| *x /= norm);
            v
        }
    }

    impl Embedder for StubEmbedder {
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
                self.calls
                    .lock()
                    .unwrap()
                    .push((inputs.clone(), kind, Instant::now()));
                tokio::time::sleep(self.delay).await;
                if inputs.iter().any(|i| i.contains("fail:")) {
                    return Err(DomainError::Internal("HTTP 503".into()));
                }
                if inputs.iter().any(|i| i.contains("reject:")) {
                    return Err(DomainError::InvalidInput("HTTP 400".into()));
                }
                Ok(Embeddings {
                    vectors: inputs.iter().map(|i| self.vector(i)).collect(),
                    usage_tokens: inputs.len() as u32 * 7,
                })
            })
        }
    }

    pub(crate) struct DisabledStub;

    impl Embedder for DisabledStub {
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
            unreachable!("a disabled embedder is never called")
        }
    }

    type Failure = (Uuid, String, Option<DateTime<Utc>>);

    #[derive(Default)]
    struct FakeRepo {
        queue: Mutex<Vec<EmbedJob>>,
        claims: Mutex<Vec<String>>,
        stored: Mutex<Vec<(Uuid, String, Vec<EmbeddedChunk>)>>,
        failures: Mutex<Vec<Failure>>,
    }

    impl EmbeddingRepository for FakeRepo {
        async fn claim_embed_batch(
            &self,
            model_id: &str,
            limit: i64,
            _lease: Duration,
        ) -> Result<Vec<EmbedJob>, DomainError> {
            self.claims.lock().unwrap().push(model_id.to_string());
            let mut queue = self.queue.lock().unwrap();
            let n = queue.len().min(limit as usize);
            Ok(queue.drain(..n).collect())
        }

        async fn store_chunks(
            &self,
            job: &EmbedJob,
            model_id: &str,
            chunks: &[EmbeddedChunk],
        ) -> Result<bool, DomainError> {
            self.stored.lock().unwrap().push((
                job.bookmark_id,
                model_id.to_string(),
                chunks.to_vec(),
            ));
            Ok(true)
        }

        async fn record_embed_failure(
            &self,
            job: &EmbedJob,
            _model_id: &str,
            error: &str,
            retry_at: Option<DateTime<Utc>>,
        ) -> Result<bool, DomainError> {
            self.failures
                .lock()
                .unwrap()
                .push((job.bookmark_id, error.to_string(), retry_at));
            Ok(true)
        }
    }

    fn job(title: &str, text: Option<String>, attempts: i32) -> EmbedJob {
        EmbedJob {
            bookmark_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            url: format!("https://example.com/{}", title.replace(' ', "-")),
            title: Some(title.to_string()),
            description: Some("a description".into()),
            tags: vec!["tag".into()],
            text,
            embed_attempts: attempts,
            version: Utc::now(),
        }
    }

    fn article(words: usize) -> String {
        (0..words)
            .map(|i| format!("word{i}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn start(jobs: Vec<EmbedJob>, embedder: Arc<StubEmbedder>) -> Arc<FakeRepo> {
        let repo = Arc::new(FakeRepo::default());
        *repo.queue.lock().unwrap() = jobs;
        EmbedWorker::new(repo.clone(), embedder).spawn(Arc::new(Notify::new()));
        repo
    }

    #[tokio::test(start_paused = true)]
    async fn embeds_articles_as_chunks_and_failed_fetches_as_metadata() {
        let embedder = Arc::new(StubEmbedder::new(8));
        let long = job("Long read", Some(article(400)), 1);
        let failed = job("Unfetchable page", None, 1);
        let repo = start(vec![long.clone(), failed.clone()], embedder.clone());
        tokio::time::sleep(Duration::from_secs(5)).await;

        let stored = repo.stored.lock().unwrap();
        assert_eq!(stored.len(), 2);
        let (id, model, chunks) = &stored[0];
        assert_eq!((*id, model.as_str()), (long.bookmark_id, "stub:hash@8"));
        assert_eq!(chunks.len(), 3);
        assert!(
            chunks[0].text.starts_with("word0 "),
            "no header in the text"
        );
        assert_eq!(
            chunks[0].embedding,
            embedder.vector(&format!("Long read\na description\n{}", chunks[0].text))
        );
        let (_, _, meta) = &stored[1];
        assert_eq!(meta.len(), 1);
        assert_eq!(
            meta[0].text,
            "Unfetchable page\na description\nhttps://example.com/Unfetchable-page\ntag"
        );

        // One call per bookmark, as documents.
        let calls = embedder.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0.len(), 3);
        assert!(calls.iter().all(|c| c.1 == EmbedKind::Document));
        assert!(
            (MIN_CHUNKED_TEXT_CHARS..).contains(&long.text.as_ref().unwrap().len()),
            "the article is long enough to chunk"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn transient_errors_retry_on_a_schedule_and_rejections_are_final() {
        let embedder = Arc::new(StubEmbedder::new(8));
        let flaky = job("fail: flaky vendor", None, 2);
        let rejected = job("reject: never", None, 1);
        let before = Utc::now();
        let repo = start(vec![flaky.clone(), rejected.clone()], embedder);
        tokio::time::sleep(Duration::from_secs(5)).await;

        assert!(repo.stored.lock().unwrap().is_empty());
        let failures = repo.failures.lock().unwrap();
        let (id, error, retry_at) = &failures[0];
        assert_eq!(*id, flaky.bookmark_id);
        assert_eq!(error, "internal error: HTTP 503");
        let retry_at = retry_at.expect("transient errors are retried");
        assert!(retry_at >= before + chrono::Duration::minutes(10));
        assert!(retry_at <= Utc::now() + chrono::Duration::minutes(10));
        assert_eq!(failures[1].0, rejected.bookmark_id);
        assert_eq!(failures[1].2, None, "a rejection is final for this model");
    }

    #[tokio::test(start_paused = true)]
    async fn calls_are_rate_limited_and_the_queue_resumes_after_idle() {
        let embedder = Arc::new(StubEmbedder::new(8));
        let jobs: Vec<EmbedJob> = (0..4).map(|i| job(&format!("page {i}"), None, 1)).collect();
        let repo = start(jobs, embedder.clone());
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(repo.stored.lock().unwrap().len(), 4);
        {
            let calls = embedder.calls.lock().unwrap();
            for pair in calls.windows(2) {
                assert!(pair[1].2 - pair[0].2 >= MIN_CALL_INTERVAL);
            }
        }

        // A job queued later is picked up by the next poll.
        repo.queue.lock().unwrap().push(job("later", None, 1));
        tokio::time::sleep(IDLE_POLL + Duration::from_secs(1)).await;
        assert_eq!(repo.stored.lock().unwrap().len(), 5);
        assert!(
            repo.claims
                .lock()
                .unwrap()
                .iter()
                .all(|m| m == "stub:hash@8")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_hanging_embedder_times_out_as_transient() {
        let embedder = Arc::new(StubEmbedder {
            delay: Duration::from_secs(3600),
            ..StubEmbedder::new(8)
        });
        let repo = start(vec![job("slow", None, 1)], embedder);
        tokio::time::sleep(EMBED_TIMEOUT + Duration::from_secs(1)).await;
        let failures = repo.failures.lock().unwrap();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].1, "internal error: embedding timed out");
        assert!(failures[0].2.is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn a_disabled_embedder_starts_nothing() {
        let repo = Arc::new(FakeRepo::default());
        repo.queue.lock().unwrap().push(job("anything", None, 1));
        EmbedWorker::new(repo.clone(), Arc::new(DisabledStub)).spawn(Arc::new(Notify::new()));
        tokio::time::sleep(Duration::from_secs(30)).await;
        assert!(repo.claims.lock().unwrap().is_empty());
        assert_eq!(repo.queue.lock().unwrap().len(), 1);
    }
}
