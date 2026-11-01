use crate::domain::error::DomainError;
use crate::domain::search::{EmbedJob, EmbeddedChunk};
use chrono::{DateTime, Utc};
use std::time::Duration;

/// The embed queue: bookmarks whose chunks were not built by the configured
/// embedder (new results of the ingest worker, failed fetches, edits, the
/// backfill after migration 017, and a model change). Its state lives in the
/// database, so the backfill resumes where it stopped.
#[trait_variant::make(Send)]
pub trait EmbeddingRepository: Send + Sync {
    /// Claim up to `limit` rows that are not pending and whose
    /// `embedded_model` is not `model_id`. Claimed rows are leased for
    /// `lease`, like ingest claims.
    async fn claim_embed_batch(
        &self,
        model_id: &str,
        limit: i64,
        lease: Duration,
    ) -> Result<Vec<EmbedJob>, DomainError>;

    /// Replace the bookmark's chunks with `chunks` (in `ord` order) and mark
    /// it embedded by `model_id`. Returns false, and stores nothing, when the
    /// row changed since the claim or the bookmark is gone.
    async fn store_chunks(
        &self,
        job: &EmbedJob,
        model_id: &str,
        chunks: &[EmbeddedChunk],
    ) -> Result<bool, DomainError>;

    /// Record a failed embedding. With `retry_at` the row is retried then;
    /// without it the failure is final for `model_id`: the row is marked
    /// done with no chunks, and only a model change retries it. Returns
    /// false like `store_chunks`.
    async fn record_embed_failure(
        &self,
        job: &EmbedJob,
        model_id: &str,
        error: &str,
        retry_at: Option<DateTime<Utc>>,
    ) -> Result<bool, DomainError>;
}
