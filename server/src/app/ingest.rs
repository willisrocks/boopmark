use crate::domain::error::DomainError;
use crate::domain::ports::article_fetcher::{ArticleFetchError, ArticleFetcher};
use crate::domain::ports::search_repo::SearchRepository;
use crate::domain::search::{IngestFailure, IngestJob, IngestStatus, retry_delay};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::task::JoinSet;

/// A claimed job is invisible to other claims for this long, well past
/// `JOB_TIMEOUT`.
const CLAIM_LEASE: Duration = Duration::from_secs(10 * 60);
/// One job (per-host waits, every redirect hop, extraction) gives up after
/// this long, so a hostile or stuck page cannot hold a slot.
const JOB_TIMEOUT: Duration = Duration::from_secs(2 * 60);
/// How long the worker sleeps on an empty queue, unless it is woken by a
/// `NOTIFY bookmark_ingest` or a job finishes.
const IDLE_POLL: Duration = Duration::from_secs(2);
const MAX_ERROR_CHARS: usize = 500;

fn report(result: Result<(), tokio::task::JoinError>) {
    if let Err(e) = result {
        tracing::warn!(error = %e, "article ingest task panicked");
    }
}

/// What to record for a fetch that produced no article.
fn failure_for(job: &IngestJob, err: &ArticleFetchError, now: DateTime<Utc>) -> IngestFailure {
    let error: String = err.to_string().chars().take(MAX_ERROR_CHARS).collect();
    let (status, retry_at) = match err {
        ArticleFetchError::Transient(_) => match retry_delay(job.attempts) {
            Some(delay) => (IngestStatus::Pending, Some(now + delay)),
            None => (IngestStatus::Failed, None),
        },
        ArticleFetchError::Permanent(_) => (IngestStatus::Failed, None),
        ArticleFetchError::Skipped(_) => (IngestStatus::Skipped, None),
    };
    IngestFailure {
        status,
        error,
        retry_at,
    }
}

/// Drains the article ingest queue: keeps up to `concurrency` jobs in
/// flight, each claimed as a slot frees up, and stores the extracted text or
/// the outcome of the failure. A failed fetch leaves the bookmark searchable
/// by its metadata. Per-host pacing is the fetcher's job.
pub struct IngestWorker<R, F> {
    repo: Arc<R>,
    fetcher: Arc<F>,
    concurrency: usize,
    /// Woken after each stored result, so the embedding worker picks it up
    /// at once.
    on_stored: Option<Arc<Notify>>,
}

impl<R, F> IngestWorker<R, F>
where
    R: SearchRepository + 'static,
    F: ArticleFetcher + 'static,
{
    pub fn new(repo: Arc<R>, fetcher: Arc<F>, concurrency: usize) -> Self {
        Self {
            repo,
            fetcher,
            concurrency: concurrency.max(1),
            on_stored: None,
        }
    }

    /// Notify `notify` whenever a result (article or failure) is stored.
    pub fn notify_on_stored(mut self, notify: Arc<Notify>) -> Self {
        self.on_stored = Some(notify);
        self
    }

    /// Run forever on a background task. `wake` cuts the idle sleep short.
    pub fn spawn(self, wake: Arc<Notify>) {
        let worker = Arc::new(self);
        tokio::spawn(async move { worker.run(wake).await });
    }

    async fn run(self: Arc<Self>, wake: Arc<Notify>) {
        tracing::info!(
            concurrency = self.concurrency,
            "article ingest worker started"
        );
        let mut in_flight = JoinSet::new();
        loop {
            if let Err(e) = self.clone().fill(&mut in_flight).await {
                tracing::warn!(error = %e, "article ingest: claiming jobs failed");
            }
            // Wait for a free slot (a job finishing), new work, or the poll.
            tokio::select! {
                Some(result) = in_flight.join_next() => {
                    report(result);
                    // Refill every slot that has freed up in one claim.
                    while let Some(result) = in_flight.try_join_next() {
                        report(result);
                    }
                }
                _ = wake.notified() => {}
                _ = tokio::time::sleep(IDLE_POLL) => {}
            }
        }
    }

    /// Claim jobs for the free slots and start them.
    async fn fill(self: Arc<Self>, in_flight: &mut JoinSet<()>) -> Result<(), DomainError> {
        let free = self.concurrency.saturating_sub(in_flight.len());
        if free == 0 {
            return Ok(());
        }
        let jobs = self
            .repo
            .claim_ingest_batch(free as i64, CLAIM_LEASE)
            .await?;
        for job in jobs {
            let worker = self.clone();
            in_flight.spawn(async move { worker.process(job).await });
        }
        Ok(())
    }

    async fn process(&self, job: IngestJob) {
        let fetched = tokio::time::timeout(JOB_TIMEOUT, self.fetcher.fetch(&job.url))
            .await
            .unwrap_or_else(|_| Err(ArticleFetchError::Transient("timed out".to_string())));
        let stored = match fetched {
            Ok(article) => {
                let hash = format!("{:x}", Sha256::digest(article.text.as_bytes()));
                self.repo.store_article(&job, &article.text, &hash).await
            }
            Err(err) => {
                let failure = failure_for(&job, &err, Utc::now());
                tracing::debug!(
                    bookmark_id = %job.bookmark_id,
                    attempts = job.attempts,
                    status = failure.status.as_str(),
                    error = %failure.error,
                    "article fetch failed"
                );
                self.repo.record_ingest_failure(&job, &failure).await
            }
        };
        match stored {
            Ok(true) => {
                if let Some(notify) = &self.on_stored {
                    notify.notify_one();
                }
            }
            // Deleted, its URL changed, or another claim took it over.
            Ok(false) => tracing::debug!(bookmark_id = %job.bookmark_id, "ingest result dropped"),
            Err(e) => {
                tracing::warn!(bookmark_id = %job.bookmark_id, error = %e, "storing ingest result failed")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ports::article_fetcher::FetchedArticle;
    use crate::domain::search::{SearchHit, SearchQuery};
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;
    use uuid::Uuid;

    #[derive(Default)]
    struct FakeRepo {
        queue: Mutex<Vec<IngestJob>>,
        claims: Mutex<Vec<i64>>,
        stored: Mutex<Vec<(IngestJob, String, String)>>,
        failures: Mutex<Vec<(IngestJob, IngestFailure)>>,
    }

    impl SearchRepository for FakeRepo {
        async fn search(
            &self,
            _user_id: Uuid,
            _query: &SearchQuery,
        ) -> Result<Vec<SearchHit>, DomainError> {
            unreachable!("the worker never searches")
        }

        async fn claim_ingest_batch(
            &self,
            limit: i64,
            _lease: Duration,
        ) -> Result<Vec<IngestJob>, DomainError> {
            self.claims.lock().unwrap().push(limit);
            let mut queue = self.queue.lock().unwrap();
            let n = queue.len().min(limit as usize);
            Ok(queue.drain(..n).collect())
        }

        async fn store_article(
            &self,
            job: &IngestJob,
            text: &str,
            content_hash: &str,
        ) -> Result<bool, DomainError> {
            self.stored.lock().unwrap().push((
                job.clone(),
                text.to_string(),
                content_hash.to_string(),
            ));
            Ok(true)
        }

        async fn record_ingest_failure(
            &self,
            job: &IngestJob,
            failure: &IngestFailure,
        ) -> Result<bool, DomainError> {
            self.failures
                .lock()
                .unwrap()
                .push((job.clone(), failure.clone()));
            Ok(true)
        }
    }

    /// Answers by the last URL path segment; `slow` takes 10 minutes and
    /// `hang` never answers.
    struct FakeFetcher;

    impl ArticleFetcher for FakeFetcher {
        fn fetch(
            &self,
            url: &str,
        ) -> Pin<Box<dyn Future<Output = Result<FetchedArticle, ArticleFetchError>> + Send + '_>>
        {
            let url = url.to_string();
            Box::pin(async move {
                match url.rsplit('/').next().unwrap() {
                    "ok" => Ok(FetchedArticle {
                        text: "article body".into(),
                        final_url: url,
                        html_bytes: 100,
                    }),
                    "slow" => {
                        tokio::time::sleep(Duration::from_secs(600)).await;
                        Err(ArticleFetchError::Permanent("too slow to matter".into()))
                    }
                    "hang" => std::future::pending().await,
                    "flaky" => Err(ArticleFetchError::Transient("HTTP 503".into())),
                    "gone" => Err(ArticleFetchError::Permanent("HTTP 404".into())),
                    _ => Err(ArticleFetchError::Skipped(
                        "not an HTML page: application/pdf".into(),
                    )),
                }
            })
        }
    }

    fn job(url: &str, attempts: i32) -> IngestJob {
        IngestJob {
            bookmark_id: Uuid::new_v4(),
            url: url.to_string(),
            attempts,
            claim_token: Uuid::new_v4(),
        }
    }

    fn worker(jobs: Vec<IngestJob>, concurrency: usize) -> (Arc<FakeRepo>, Arc<Notify>) {
        let repo = Arc::new(FakeRepo::default());
        *repo.queue.lock().unwrap() = jobs;
        let wake = Arc::new(Notify::new());
        IngestWorker::new(repo.clone(), Arc::new(FakeFetcher), concurrency).spawn(wake.clone());
        (repo, wake)
    }

    #[test]
    fn transient_failures_follow_the_retry_schedule_then_fail() {
        let now = Utc::now();
        let flaky = ArticleFetchError::Transient("HTTP 503".into());
        let first = failure_for(&job("https://a.test/x", 1), &flaky, now);
        assert_eq!(first.status, IngestStatus::Pending);
        assert_eq!(first.retry_at, Some(now + chrono::Duration::minutes(1)));
        let third = failure_for(&job("https://a.test/x", 3), &flaky, now);
        assert_eq!(third.retry_at, Some(now + chrono::Duration::hours(24)));
        let last = failure_for(&job("https://a.test/x", 4), &flaky, now);
        assert_eq!((last.status, last.retry_at), (IngestStatus::Failed, None));
        assert_eq!(last.error, "HTTP 503");
    }

    #[test]
    fn permanent_and_skipped_failures_are_final() {
        let now = Utc::now();
        let gone = failure_for(
            &job("https://a.test/x", 1),
            &ArticleFetchError::Permanent("HTTP 404".into()),
            now,
        );
        assert_eq!((gone.status, gone.retry_at), (IngestStatus::Failed, None));
        let pdf = failure_for(
            &job("https://a.test/x", 1),
            &ArticleFetchError::Skipped("not HTML".into()),
            now,
        );
        assert_eq!((pdf.status, pdf.retry_at), (IngestStatus::Skipped, None));
        let long = failure_for(
            &job("https://a.test/x", 9),
            &ArticleFetchError::Permanent("x".repeat(2000)),
            now,
        );
        assert_eq!(long.error.chars().count(), MAX_ERROR_CHARS);
    }

    #[tokio::test(start_paused = true)]
    async fn stores_articles_and_records_failures() {
        let (repo, _wake) = worker(
            vec![
                job("https://a.test/ok", 1),
                job("https://b.test/flaky", 2),
                job("https://c.test/gone", 1),
                job("https://d.test/report.pdf", 1),
                job("https://e.test/ok", 1),
            ],
            4,
        );
        tokio::time::sleep(Duration::from_secs(10)).await;

        let stored = repo.stored.lock().unwrap();
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0].1, "article body");
        assert_eq!(
            stored[0].2,
            format!("{:x}", Sha256::digest("article body".as_bytes()))
        );

        let failures = repo.failures.lock().unwrap();
        let by_url = |url: &str| {
            failures
                .iter()
                .find(|(j, _)| j.url == url)
                .map(|(_, f)| f.clone())
                .unwrap()
        };
        let flaky = by_url("https://b.test/flaky");
        assert_eq!(flaky.status, IngestStatus::Pending);
        assert!(flaky.retry_at.is_some());
        assert_eq!(by_url("https://c.test/gone").status, IngestStatus::Failed);
        assert_eq!(
            by_url("https://d.test/report.pdf").status,
            IngestStatus::Skipped
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_job_does_not_hold_up_the_others() {
        let mut jobs = vec![job("https://slow.test/slow", 1)];
        jobs.extend((0..6).map(|i| job(&format!("https://fast{i}.test/ok"), 1)));
        let (repo, _wake) = worker(jobs, 2);
        tokio::time::sleep(Duration::from_secs(30)).await;
        // One slot is held by the slow job; the other drained the rest.
        assert_eq!(repo.stored.lock().unwrap().len(), 6);
        assert!(repo.failures.lock().unwrap().is_empty());
        // Claims ask only for the free slots.
        assert!(repo.claims.lock().unwrap().iter().all(|n| *n <= 2));
    }

    #[tokio::test(start_paused = true)]
    async fn a_hanging_job_times_out_as_transient() {
        let (repo, _wake) = worker(vec![job("https://hang.test/hang", 1)], 1);
        tokio::time::sleep(JOB_TIMEOUT + Duration::from_secs(5)).await;
        let failures = repo.failures.lock().unwrap();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].1.status, IngestStatus::Pending);
        assert_eq!(failures[0].1.error, "timed out");
    }
}
