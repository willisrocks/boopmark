use crate::domain::error::DomainError;
use std::time::Duration;
use uuid::Uuid;

/// The capture queue holds at most this many waiting payloads ...
pub const CAPTURE_QUEUE_ROWS: i64 = 64;
/// ... and at most this much gzip in total. Both bounds keep a burst of
/// captures from growing the write surface: past them the route answers
/// `503 busy` and the client may retry later, never automatically.
pub const CAPTURE_QUEUE_BYTES: i64 = 64 * 1024 * 1024;

/// A captured page the route accepted but did not parse (the `queued`
/// answer), waiting for the ingest worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCapture {
    pub bookmark_id: Uuid,
    pub user_id: Uuid,
    /// `X-Page-Url` after validation.
    pub page_url: String,
    /// The gzip body the client sent, already within the caps.
    pub html_gz: Vec<u8>,
}

/// A claimed payload: the row is gone from the queue, so a second worker
/// cannot parse it again.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct QueuedCapture {
    pub bookmark_id: Uuid,
    pub user_id: Uuid,
    pub page_url: String,
    pub html_gz: Vec<u8>,
}

/// The capture queue: HTML accepted but not yet parsed. One row per
/// bookmark, a re-capture replaces the waiting payload.
#[trait_variant::make(Send)]
pub trait CaptureQueueRepository: Send + Sync {
    /// Store (or replace) a waiting payload. Returns false when the queue is
    /// full, which the route reports as `503 busy`.
    async fn enqueue_capture(&self, capture: &NewCapture) -> Result<bool, DomainError>;

    /// Take up to `limit` waiting payloads, oldest first. Claiming removes
    /// them, so a queued payload is parsed at most once.
    async fn claim_captures(&self, limit: i64) -> Result<Vec<QueuedCapture>, DomainError>;

    /// Note on the bookmark why its captured HTML could not be parsed. The
    /// row keeps whatever state it had: the bookmark was already searchable
    /// by its metadata.
    async fn record_capture_failure(
        &self,
        bookmark_id: Uuid,
        error: &str,
    ) -> Result<(), DomainError>;

    /// Delete payloads that have waited longer than `max_age`, unparsed;
    /// returns how many were deleted.
    async fn purge_stale_captures(&self, max_age: Duration) -> Result<u64, DomainError>;
}
