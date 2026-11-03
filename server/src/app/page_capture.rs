//! The capture queue: HTML the browser sent that the capture route accepted
//! but did not parse in the request (the `queued` answer). A dedicated loop
//! drains it, so it runs whether or not the article fetcher does.

use crate::adapters::article::readability::{CaptureExtractError, HtmlExtractor};
use crate::domain::error::DomainError;
use crate::domain::ports::page_capture_repo::{CaptureQueueRepository, NewCapture, QueuedCapture};
use crate::domain::ports::search_repo::SearchRepository;
use crate::domain::search::ContentSource;
use crate::domain::url_match::same_page;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Notify;

/// One drain parses at most this many payloads.
const DRAIN_BATCH: usize = 32;
/// How long the drain sleeps when nothing arrives; it also carries the
/// hourly sweep of expired payloads.
const IDLE_POLL: Duration = Duration::from_secs(60);
/// A payload that has waited this long is deleted unparsed: whatever made
/// the request time out will not get better by waiting a day.
const MAX_QUEUE_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// The stale sweep runs at most this often; the queue is usually empty.
const PURGE_INTERVAL: Duration = Duration::from_secs(60 * 60);
const MAX_ERROR_CHARS: usize = 500;

/// The capture write path: text stored now, or a payload queued for the
/// drain (the route answers `202 indexed` or `202 queued`).
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

    /// Store captured text as `source = 'captured'`, for the bookmark URL the
    /// request was checked against. False when the bookmark was deleted, is
    /// not the user's, or no longer has that URL.
    pub async fn store_capture(
        &self,
        bookmark_id: uuid::Uuid,
        user_id: uuid::Uuid,
        url: &str,
        text: &str,
    ) -> Result<bool, DomainError> {
        let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
        self.repo
            .store_capture(bookmark_id, user_id, url, text, &hash)
            .await
    }

    /// Queue a validated payload; false when the capture queue is full.
    pub async fn queue_capture(&self, capture: &NewCapture) -> Result<bool, DomainError> {
        self.repo.enqueue_capture(capture).await
    }
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

    /// Run forever on a background task. `wake` is the route's "a capture is
    /// waiting"; the idle poll covers the expired-payload sweep. This is not
    /// part of the ingest worker, so captures drain with
    /// `ARTICLE_INGEST_ENABLED=0` too.
    pub fn spawn(self, wake: Arc<Notify>) {
        tokio::spawn(async move {
            tracing::info!("capture drain started");
            loop {
                loop {
                    match self.drain_once(DRAIN_BATCH).await {
                        Ok(0) => break,
                        Ok(captures) => tracing::debug!(captures, "parsed captured pages"),
                        Err(e) => {
                            tracing::warn!(error = %e, "draining the capture queue failed");
                            break;
                        }
                    }
                }
                tokio::select! {
                    _ = wake.notified() => {}
                    _ = tokio::time::sleep(IDLE_POLL) => {}
                }
            }
        });
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
        // The bookmark may have been pointed at another page since the
        // capture was queued: the old page's text must not be indexed
        // against the new URL.
        if !same_page(&capture.bookmark_url, &capture.page_url) {
            tracing::debug!(
                %bookmark_id,
                page_url = %capture.page_url,
                bookmark_url = %capture.bookmark_url,
                "queued capture dropped: the bookmark now has another URL"
            );
            return;
        }
        let text = match self
            .extractor
            .extract_captured(
                Arc::new(capture.html_gz),
                self.max_bytes,
                Some("text/html; charset=utf-8"),
                &capture.page_url,
            )
            .await
        {
            Ok(text) => text,
            // The route validated the same bytes; this is a bug or a
            // hand-written row. Keep no payload either way.
            Err(CaptureExtractError::Gzip(err)) => {
                tracing::warn!(%bookmark_id, error = ?err, "queued capture is not usable gzip");
                self.fail(bookmark_id, "captured page could not be decoded")
                    .await;
                return;
            }
            Err(CaptureExtractError::NoArticle(err)) => {
                tracing::debug!(%bookmark_id, error = %err, "queued capture has no article");
                self.fail(bookmark_id, "no readable article").await;
                return;
            }
            Err(CaptureExtractError::Unfinished(err)) => {
                // Do not report a timeout as "no readable article": the page
                // may well have one.
                tracing::warn!(%bookmark_id, error = %err, "queued capture was not parsed");
                self.fail(bookmark_id, &format!("parse failed: {err}"))
                    .await;
                return;
            }
        };
        let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
        match self
            .repo
            .store_capture(
                bookmark_id,
                capture.user_id,
                &capture.bookmark_url,
                &text,
                &hash,
            )
            .await
        {
            Ok(true) => {
                tracing::debug!(%bookmark_id, source = ContentSource::Captured.as_str(), "stored captured page");
                if let Some(notify) = &self.on_stored {
                    notify.notify_one();
                }
            }
            Ok(false) => {
                tracing::debug!(%bookmark_id, "capture result dropped: bookmark gone or moved")
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
        stored: Mutex<Vec<(Uuid, Uuid, String, String)>>,
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
            url: &str,
            text: &str,
            _content_hash: &str,
        ) -> Result<bool, DomainError> {
            self.stored.lock().unwrap().push((
                bookmark_id,
                user_id,
                url.to_string(),
                text.to_string(),
            ));
            Ok(true)
        }
    }

    fn capture(html: &[u8]) -> QueuedCapture {
        let page_url = "https://example.com/article".to_string();
        QueuedCapture {
            bookmark_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            bookmark_url: page_url.clone(),
            page_url,
            html_gz: gzip(html),
        }
    }

    fn worker(repo: &Arc<FakeRepo>, cap: usize) -> CaptureWorker<FakeRepo> {
        CaptureWorker::new(repo.clone(), Arc::new(HtmlExtractor::new(1)), cap)
    }

    /// Wait, without sleeping, until the drain has claimed `n` times.
    async fn claimed_at_least(repo: &FakeRepo, n: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while repo.claimed.lock().unwrap().len() < n {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the drain loop claimed");
    }

    /// Wait, without sleeping, until the drain has stored `n` captures.
    async fn stored_at_least(repo: &FakeRepo, n: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while repo.stored.lock().unwrap().len() < n {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the drain stored the capture");
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
            assert_eq!(
                stored[0].2, first.bookmark_url,
                "stored for the checked URL"
            );
            assert!(stored[0].3.contains("The borrow checker enforces"));
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
        let id = page.bookmark_id;
        *repo.queued.lock().unwrap() = vec![page];
        assert_eq!(worker(&repo, 1024).drain_once(4).await.unwrap(), 1);
        assert!(repo.stored.lock().unwrap().is_empty());
        assert_eq!(
            repo.failures.lock().unwrap().as_slice(),
            [(id, "captured page could not be decoded".to_string())]
        );
    }

    /// A bookmark pointed at another page since the capture was queued keeps
    /// its text alone; the payload is dropped silently.
    #[tokio::test]
    async fn a_capture_of_another_page_is_dropped() {
        let repo = Arc::new(FakeRepo::default());
        let mut page = capture(ARTICLE.as_bytes());
        page.bookmark_url = "https://example.com/another-page".into();
        *repo.queued.lock().unwrap() = vec![page];
        assert_eq!(
            worker(&repo, 5 * 1024 * 1024).drain_once(4).await.unwrap(),
            1
        );
        assert!(repo.stored.lock().unwrap().is_empty());
        assert!(
            repo.failures.lock().unwrap().is_empty(),
            "a moved bookmark is not a parse failure"
        );
    }

    /// The drain is its own loop: it runs at spawn, and the route's wake
    /// makes it look again without waiting for the idle poll.
    #[tokio::test]
    async fn the_drain_loop_runs_and_is_woken_by_a_new_capture() {
        let repo = Arc::new(FakeRepo::default());
        let wake = Arc::new(Notify::new());
        worker(&repo, 5 * 1024 * 1024).spawn(wake.clone());
        claimed_at_least(&repo, 1).await;

        *repo.queued.lock().unwrap() = vec![capture(ARTICLE.as_bytes())];
        wake.notify_one();
        claimed_at_least(&repo, 2).await;
        stored_at_least(&repo, 1).await;
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
}
