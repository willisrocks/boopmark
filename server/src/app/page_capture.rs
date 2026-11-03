//! The capture queue: HTML the browser sent that the capture route accepted
//! but did not parse in the request (the `queued` answer). The ingest worker
//! drains it before claiming fetch jobs.

use crate::adapters::article::readability::HtmlExtractor;
use crate::domain::error::DomainError;
use crate::domain::ports::page_capture_repo::{CaptureQueueRepository, NewCapture, QueuedCapture};
use crate::domain::ports::search_repo::SearchRepository;
use crate::domain::search::ContentSource;
use flate2::bufread::GzDecoder;
use sha2::{Digest, Sha256};
use std::future::Future;
use std::io::Read;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Notify;

/// One drain parses at most this many payloads, however many slots are free.
const DRAIN_BATCH: usize = 32;
/// A payload that has waited this long is deleted unparsed: whatever made
/// the request time out will not get better by waiting a day.
const MAX_QUEUE_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// The stale sweep runs at most this often; the queue is usually empty.
const PURGE_INTERVAL: Duration = Duration::from_secs(60 * 60);
const MAX_ERROR_CHARS: usize = 500;

/// Why a captured payload was refused before any parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GzipError {
    /// Over the decompressed cap: a gzip bomb, or a page too large for
    /// either cap.
    TooLarge,
    /// Not gzip, truncated, or empty.
    Corrupt,
}

/// Inflate a gzip body, never producing more than `cap` bytes: the output is
/// bounded while inflating, so a bomb costs milliseconds, not memory. A
/// second gzip member or bytes after the compressed stream are `Corrupt`:
/// the extension sends one gzip stream of one document.
pub fn gunzip_capped(gz: &[u8], cap: usize) -> Result<Vec<u8>, GzipError> {
    let mut out = Vec::new();
    let mut decoder = GzDecoder::new(gz);
    // One byte more than the cap tells "exactly at the cap" from "over it".
    let limit = (cap as u64).saturating_add(1);
    decoder
        .by_ref()
        .take(limit)
        .read_to_end(&mut out)
        .map_err(|_| GzipError::Corrupt)?;
    if out.len() > cap {
        return Err(GzipError::TooLarge);
    }
    if out.is_empty() {
        return Err(GzipError::Corrupt);
    }
    // `bufread::GzDecoder` stops at the end of its one member, so leftover
    // input is trailing garbage.
    if !decoder.get_ref().is_empty() {
        return Err(GzipError::Corrupt);
    }
    Ok(out)
}

/// The capture write path: text stored now, or a payload queued for the
/// ingest worker (the route answers `202 indexed` or `202 queued`).
pub struct CaptureService<R> {
    repo: Arc<R>,
}

impl<R> CaptureService<R>
where
    R: SearchRepository + CaptureQueueRepository,
{
    pub fn new(repo: Arc<R>) -> Self {
        Self { repo }
    }

    /// Store captured text as `source = 'captured'`. False when the bookmark
    /// was deleted or is not the user's.
    pub async fn store_capture(
        &self,
        bookmark_id: uuid::Uuid,
        user_id: uuid::Uuid,
        text: &str,
    ) -> Result<bool, DomainError> {
        let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
        self.repo
            .store_capture(bookmark_id, user_id, text, &hash)
            .await
    }

    /// Queue a validated payload; false when the capture queue is full.
    pub async fn queue_capture(&self, capture: &NewCapture) -> Result<bool, DomainError> {
        self.repo.enqueue_capture(capture).await
    }
}

/// What the ingest worker calls to drain the capture queue, without knowing
/// the repository's other bounds.
pub trait CaptureDrain: Send + Sync {
    /// Parse up to `limit` waiting payloads, store what parses, and return
    /// how many payloads were taken.
    fn drain(
        &self,
        limit: usize,
    ) -> Pin<Box<dyn Future<Output = Result<usize, DomainError>> + Send + '_>>;
}

/// Parses the capture queue through the same `HtmlExtractor` as a fetch.
pub struct CaptureWorker<R> {
    repo: Arc<R>,
    extractor: Arc<HtmlExtractor>,
    /// The decompressed cap, from `PAGE_CAPTURE_MAX_BYTES`.
    max_bytes: usize,
    /// Woken after each stored capture, so the embed worker picks it up.
    on_stored: Option<Arc<Notify>>,
    /// When the stale sweep last ran.
    last_purge: std::sync::Mutex<Option<Instant>>,
}

impl<R> CaptureWorker<R>
where
    R: SearchRepository + CaptureQueueRepository + 'static,
{
    pub fn new(repo: Arc<R>, extractor: Arc<HtmlExtractor>, max_bytes: usize) -> Self {
        Self {
            repo,
            extractor,
            max_bytes,
            on_stored: None,
            last_purge: std::sync::Mutex::new(None),
        }
    }

    /// Notify `notify` whenever a capture is stored.
    pub fn notify_on_stored(mut self, notify: Arc<Notify>) -> Self {
        self.on_stored = Some(notify);
        self
    }

    /// Sweep expired payloads, then claim and parse up to `limit`. Returns
    /// how many payloads were claimed.
    pub async fn drain_once(&self, limit: usize) -> Result<usize, DomainError> {
        self.purge_stale().await?;
        let limit = limit.clamp(1, DRAIN_BATCH);
        let captures = self.repo.claim_captures(limit as i64).await?;
        let taken = captures.len();
        let mut tasks = tokio::task::JoinSet::new();
        for capture in captures {
            let worker = CaptureTask {
                repo: self.repo.clone(),
                extractor: self.extractor.clone(),
                max_bytes: self.max_bytes,
                on_stored: self.on_stored.clone(),
            };
            tasks.spawn(async move { worker.process(capture).await });
        }
        while let Some(result) = tasks.join_next().await {
            if let Err(e) = result {
                tracing::warn!(error = %e, "captured page task panicked");
            }
        }
        Ok(taken)
    }

    /// Delete payloads nobody parsed in a day, and say so.
    async fn purge_stale(&self) -> Result<(), DomainError> {
        {
            let mut last = self.last_purge.lock().unwrap();
            if last.is_some_and(|at| at.elapsed() < PURGE_INTERVAL) {
                return Ok(());
            }
            *last = Some(Instant::now());
        }
        let purged = self.repo.purge_stale_captures(MAX_QUEUE_AGE).await?;
        if purged > 0 {
            tracing::warn!(purged, "deleted captured pages nobody parsed in 24 h");
        }
        Ok(())
    }
}

impl<R> CaptureDrain for CaptureWorker<R>
where
    R: SearchRepository + CaptureQueueRepository + 'static,
{
    fn drain(
        &self,
        limit: usize,
    ) -> Pin<Box<dyn Future<Output = Result<usize, DomainError>> + Send + '_>> {
        Box::pin(async move { self.drain_once(limit).await })
    }
}

/// One claimed payload and what it needs to become stored text.
struct CaptureTask<R> {
    repo: Arc<R>,
    extractor: Arc<HtmlExtractor>,
    max_bytes: usize,
    on_stored: Option<Arc<Notify>>,
}

impl<R> CaptureTask<R>
where
    R: SearchRepository + CaptureQueueRepository + 'static,
{
    async fn process(&self, capture: QueuedCapture) {
        let bookmark_id = capture.bookmark_id;
        let html = match gunzip_capped(&capture.html_gz, self.max_bytes) {
            Ok(html) => html,
            Err(e) => {
                // The route validated the same bytes; this is a bug or a
                // hand-written row. Keep no payload either way.
                tracing::warn!(%bookmark_id, error = ?e, "queued capture is not usable gzip");
                self.fail(bookmark_id, "captured page could not be decoded")
                    .await;
                return;
            }
        };
        let text = match self
            .extractor
            .extract(html, Some("text/html; charset=utf-8"), &capture.page_url)
            .await
        {
            Ok(text) => text,
            Err(err) => {
                tracing::debug!(%bookmark_id, error = %err, "queued capture has no article");
                self.fail(bookmark_id, "no readable article").await;
                return;
            }
        };
        let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
        match self
            .repo
            .store_capture(bookmark_id, capture.user_id, &text, &hash)
            .await
        {
            Ok(true) => {
                tracing::debug!(%bookmark_id, source = ContentSource::Captured.as_str(), "stored captured page");
                if let Some(notify) = &self.on_stored {
                    notify.notify_one();
                }
            }
            Ok(false) => {
                tracing::debug!(%bookmark_id, "capture result dropped: bookmark gone")
            }
            Err(e) => tracing::warn!(%bookmark_id, error = %e, "storing a capture failed"),
        }
    }

    async fn fail(&self, bookmark_id: uuid::Uuid, error: &str) {
        let error: String = error.chars().take(MAX_ERROR_CHARS).collect();
        if let Err(e) = self.repo.record_capture_failure(bookmark_id, &error).await {
            tracing::warn!(%bookmark_id, error = %e, "recording a capture failure failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ports::page_capture_repo::NewCapture;
    use crate::domain::search::{IngestFailure, IngestJob, SearchHit, SearchQuery};
    use std::io::Write;
    use std::sync::Mutex;
    use uuid::Uuid;

    const ARTICLE: &str = r#"<!doctype html><html><head><title>Borrowing in Rust</title></head>
        <body><article><h1>Borrowing in Rust</h1>
        <p>The borrow checker enforces that every reference is valid for as long as it is used,
           and that mutable references are never aliased, which a readability heuristic can see.</p>
        <p>Lifetimes are the names the compiler gives to those regions of validity.</p>
        </article></body></html>"#;

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[derive(Default)]
    struct FakeRepo {
        queued: Mutex<Vec<QueuedCapture>>,
        claimed: Mutex<Vec<i64>>,
        stored: Mutex<Vec<(Uuid, Uuid, String)>>,
        failures: Mutex<Vec<(Uuid, String)>>,
        purges: Mutex<Vec<Duration>>,
    }

    impl CaptureQueueRepository for FakeRepo {
        async fn enqueue_capture(&self, _capture: &NewCapture) -> Result<bool, DomainError> {
            unreachable!("the drain never enqueues")
        }
        async fn claim_captures(&self, limit: i64) -> Result<Vec<QueuedCapture>, DomainError> {
            self.claimed.lock().unwrap().push(limit);
            let mut queued = self.queued.lock().unwrap();
            let n = queued.len().min(limit as usize);
            Ok(queued.drain(..n).collect())
        }
        async fn record_capture_failure(
            &self,
            bookmark_id: Uuid,
            error: &str,
        ) -> Result<(), DomainError> {
            self.failures
                .lock()
                .unwrap()
                .push((bookmark_id, error.to_string()));
            Ok(())
        }
        async fn purge_stale_captures(&self, max_age: Duration) -> Result<u64, DomainError> {
            self.purges.lock().unwrap().push(max_age);
            Ok(0)
        }
    }

    impl SearchRepository for FakeRepo {
        async fn search(
            &self,
            _user_id: Uuid,
            _query: &SearchQuery,
        ) -> Result<Vec<SearchHit>, DomainError> {
            unreachable!("the drain never searches")
        }
        async fn claim_ingest_batch(
            &self,
            _limit: i64,
            _lease: Duration,
        ) -> Result<Vec<IngestJob>, DomainError> {
            unreachable!("the drain never fetches")
        }
        async fn store_article(
            &self,
            _job: &IngestJob,
            _text: &str,
            _content_hash: &str,
            _source: ContentSource,
        ) -> Result<bool, DomainError> {
            unreachable!("the drain never fetches")
        }
        async fn record_ingest_failure(
            &self,
            _job: &IngestJob,
            _failure: &IngestFailure,
        ) -> Result<bool, DomainError> {
            unreachable!("the drain never fetches")
        }
        async fn store_capture(
            &self,
            bookmark_id: Uuid,
            user_id: Uuid,
            text: &str,
            _content_hash: &str,
        ) -> Result<bool, DomainError> {
            self.stored
                .lock()
                .unwrap()
                .push((bookmark_id, user_id, text.to_string()));
            Ok(true)
        }
    }

    fn capture(html: &[u8]) -> QueuedCapture {
        QueuedCapture {
            bookmark_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            page_url: "https://example.com/article".into(),
            html_gz: gzip(html),
        }
    }

    fn worker(repo: &Arc<FakeRepo>, cap: usize) -> CaptureWorker<FakeRepo> {
        CaptureWorker::new(repo.clone(), Arc::new(HtmlExtractor::new(1)), cap)
    }

    #[tokio::test]
    async fn the_drain_parses_stores_and_notifies() {
        let repo = Arc::new(FakeRepo::default());
        let first = capture(ARTICLE.as_bytes());
        let second = capture(ARTICLE.as_bytes());
        *repo.queued.lock().unwrap() = vec![first.clone(), second.clone()];
        let notify = Arc::new(Notify::new());
        let woke = tokio::spawn({
            let notify = notify.clone();
            async move { notify.notified().await }
        });
        let worker = worker(&repo, 5 * 1024 * 1024).notify_on_stored(notify.clone());

        assert_eq!(worker.drain_once(8).await.unwrap(), 2);

        {
            let stored = repo.stored.lock().unwrap();
            assert_eq!(stored.len(), 2);
            assert_eq!(stored[0].0, first.bookmark_id);
            assert_eq!(stored[0].1, first.user_id);
            assert!(stored[0].2.contains("The borrow checker enforces"));
            assert!(repo.failures.lock().unwrap().is_empty());
        }
        // The embed worker is woken for the stored capture.
        tokio::time::timeout(Duration::from_secs(1), woke)
            .await
            .expect("the embed worker is woken")
            .unwrap();
    }

    #[tokio::test]
    async fn an_unparsable_capture_records_the_reason_and_stores_nothing() {
        let repo = Arc::new(FakeRepo::default());
        let page = capture(b"<html><body></body></html>");
        *repo.queued.lock().unwrap() = vec![page.clone()];
        assert_eq!(
            worker(&repo, 5 * 1024 * 1024).drain_once(4).await.unwrap(),
            1
        );
        assert!(repo.stored.lock().unwrap().is_empty());
        assert_eq!(
            repo.failures.lock().unwrap().as_slice(),
            [(page.bookmark_id, "no readable article".to_string())]
        );
    }

    #[tokio::test]
    async fn an_oversized_payload_is_dropped_as_a_failure() {
        let repo = Arc::new(FakeRepo::default());
        let page =
            capture(format!("<html><body><p>{}</p></body></html>", "x".repeat(4096)).as_bytes());
        *repo.queued.lock().unwrap() = vec![page.clone()];
        assert_eq!(worker(&repo, 1024).drain_once(4).await.unwrap(), 1);
        assert!(repo.stored.lock().unwrap().is_empty());
        assert_eq!(repo.failures.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn the_drain_batch_and_the_stale_sweep_are_bounded() {
        let repo = Arc::new(FakeRepo::default());
        let worker = worker(&repo, 1024 * 1024);
        worker.drain_once(1000).await.unwrap();
        worker.drain_once(2).await.unwrap();
        assert_eq!(*repo.claimed.lock().unwrap(), [DRAIN_BATCH as i64, 2]);
        // The sweep runs once, then waits an hour.
        assert_eq!(repo.purges.lock().unwrap().as_slice(), [MAX_QUEUE_AGE]);
    }

    #[test]
    fn gzip_caps_are_exact_and_bombs_are_rejected() {
        let payload = vec![b'a'; 1000];
        let gz = gzip(&payload);
        assert_eq!(
            gunzip_capped(&gz, 1000).unwrap(),
            payload,
            "exactly at the cap"
        );
        assert_eq!(gunzip_capped(&gz, 999), Err(GzipError::TooLarge));
        assert_eq!(gunzip_capped(&gz, 5000).unwrap().len(), 1000);

        // A 100:1 bomb is refused at the output cap, not inflated whole.
        let bomb = gzip(&vec![0u8; 5 * 1024 * 1024]);
        assert_eq!(gunzip_capped(&bomb, 1024 * 1024), Err(GzipError::TooLarge));

        assert_eq!(gunzip_capped(b"not gzip", 100), Err(GzipError::Corrupt));
        assert_eq!(gunzip_capped(&[], 100), Err(GzipError::Corrupt));
        // Truncated inside the CRC trailer, with room for all the output.
        assert_eq!(
            gunzip_capped(&gz[..gz.len() - 3], 5000),
            Err(GzipError::Corrupt)
        );
        assert_eq!(gunzip_capped(&gzip(b""), 100), Err(GzipError::Corrupt));

        let mut trailing = gz.clone();
        trailing.extend_from_slice(b"garbage");
        assert_eq!(gunzip_capped(&trailing, 1000), Err(GzipError::Corrupt));
        // The extension sends one stream of one document: a second gzip
        // member is trailing input, and `Corrupt`.
        let mut two_members = gz.clone();
        two_members.extend_from_slice(&gzip(b"second"));
        assert_eq!(gunzip_capped(&two_members, 5000), Err(GzipError::Corrupt));
    }
}
