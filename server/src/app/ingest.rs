use crate::domain::ports::article_fetcher::{ArticleFetchError, ArticleFetcher};
use crate::domain::ports::search_repo::SearchRepository;
use crate::domain::search::{IngestFailure, IngestJob, IngestStatus, retry_delay};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::task::JoinSet;
use tokio::time::Instant;

/// A claimed job is invisible to other claims for this long. It covers the
/// per-host wait plus the 15 s fetch, with a wide margin.
const CLAIM_LEASE: Duration = Duration::from_secs(10 * 60);
/// How long the worker sleeps on an empty queue, unless it is woken by a
/// `NOTIFY bookmark_ingest`.
const IDLE_POLL: Duration = Duration::from_secs(2);
/// At most one request per host per interval: imports are often many links
/// to one site.
const PER_HOST_INTERVAL: Duration = Duration::from_secs(1);
const MAX_ERROR_CHARS: usize = 500;

/// Hands out request slots per host, `interval` apart.
pub struct HostThrottle {
    interval: Duration,
    next_free: Mutex<HashMap<String, Instant>>,
}

impl HostThrottle {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            next_free: Mutex::new(HashMap::new()),
        }
    }

    /// Reserve the next slot for `host` and return when it starts.
    pub fn reserve(&self, host: &str) -> Instant {
        let now = Instant::now();
        let mut next_free = self.next_free.lock().unwrap();
        next_free.retain(|_, free_at| *free_at > now);
        let slot = next_free.get(host).copied().unwrap_or(now).max(now);
        next_free.insert(host.to_string(), slot + self.interval);
        slot
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

fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
        .unwrap_or_default()
}

/// Drains the article ingest queue: claims due bookmarks, fetches and
/// extracts their articles, and stores the text or the outcome of the
/// failure. A failed fetch leaves the bookmark searchable by its metadata.
pub struct IngestWorker<R, F> {
    repo: Arc<R>,
    fetcher: Arc<F>,
    throttle: HostThrottle,
    concurrency: usize,
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
            throttle: HostThrottle::new(PER_HOST_INTERVAL),
            concurrency: concurrency.max(1),
        }
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
        loop {
            match self.clone().run_once().await {
                Ok(0) => {
                    tokio::select! {
                        _ = wake.notified() => {}
                        _ = tokio::time::sleep(IDLE_POLL) => {}
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "article ingest: claiming jobs failed");
                    tokio::time::sleep(IDLE_POLL).await;
                }
            }
        }
    }

    /// Claim one batch and process it to the end. Returns the batch size.
    pub async fn run_once(self: Arc<Self>) -> Result<usize, crate::domain::error::DomainError> {
        let jobs = self
            .repo
            .claim_ingest_batch(self.concurrency as i64, CLAIM_LEASE)
            .await?;
        let claimed = jobs.len();
        let mut tasks = JoinSet::new();
        for job in jobs {
            let worker = self.clone();
            tasks.spawn(async move { worker.process(job).await });
        }
        while let Some(result) = tasks.join_next().await {
            if let Err(e) = result {
                tracing::warn!(error = %e, "article ingest task panicked");
            }
        }
        Ok(claimed)
    }

    async fn process(&self, job: IngestJob) {
        tokio::time::sleep_until(self.throttle.reserve(&host_of(&job.url))).await;
        let stored = match self.fetcher.fetch(&job.url).await {
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
            Ok(true) => {}
            // Deleted, or its URL changed and it was queued again.
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
    use crate::domain::error::DomainError;
    use crate::domain::ports::article_fetcher::FetchedArticle;
    use crate::domain::search::{SearchHit, SearchQuery};
    use std::future::Future;
    use std::pin::Pin;
    use uuid::Uuid;

    #[derive(Default)]
    struct FakeRepo {
        queue: Mutex<Vec<IngestJob>>,
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

    /// Answers by URL path, and records when each request started.
    #[derive(Default)]
    struct FakeFetcher {
        started: Mutex<Vec<(String, Instant)>>,
    }

    impl ArticleFetcher for FakeFetcher {
        fn fetch(
            &self,
            url: &str,
        ) -> Pin<Box<dyn Future<Output = Result<FetchedArticle, ArticleFetchError>> + Send + '_>>
        {
            self.started
                .lock()
                .unwrap()
                .push((url.to_string(), Instant::now()));
            let result = match url.rsplit('/').next().unwrap() {
                "ok" => Ok(FetchedArticle {
                    text: "article body".into(),
                    final_url: url.to_string(),
                    html_bytes: 100,
                }),
                "flaky" => Err(ArticleFetchError::Transient("HTTP 503".into())),
                "gone" => Err(ArticleFetchError::Permanent("HTTP 404".into())),
                _ => Err(ArticleFetchError::Skipped(
                    "not an HTML page: application/pdf".into(),
                )),
            };
            Box::pin(async move { result })
        }
    }

    fn job(url: &str, attempts: i32) -> IngestJob {
        IngestJob {
            bookmark_id: Uuid::new_v4(),
            url: url.to_string(),
            attempts,
        }
    }

    fn worker(
        jobs: Vec<IngestJob>,
        concurrency: usize,
    ) -> Arc<IngestWorker<FakeRepo, FakeFetcher>> {
        let repo = FakeRepo::default();
        *repo.queue.lock().unwrap() = jobs;
        Arc::new(IngestWorker::new(
            Arc::new(repo),
            Arc::new(FakeFetcher::default()),
            concurrency,
        ))
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
    async fn throttle_spaces_requests_per_host() {
        let throttle = HostThrottle::new(Duration::from_secs(1));
        let t0 = Instant::now();
        assert_eq!(throttle.reserve("a.test"), t0);
        assert_eq!(throttle.reserve("a.test"), t0 + Duration::from_secs(1));
        assert_eq!(throttle.reserve("b.test"), t0);
        assert_eq!(throttle.reserve("a.test"), t0 + Duration::from_secs(2));
        tokio::time::advance(Duration::from_secs(10)).await;
        assert_eq!(throttle.reserve("a.test"), Instant::now());
    }

    #[tokio::test(start_paused = true)]
    async fn run_once_stores_articles_and_records_failures() {
        let jobs = vec![
            job("https://a.test/ok", 1),
            job("https://b.test/flaky", 2),
            job("https://c.test/gone", 1),
            job("https://d.test/report.pdf", 1),
            job("https://e.test/ok", 1),
        ];
        let w = worker(jobs.clone(), 4);
        assert_eq!(w.clone().run_once().await.unwrap(), 4);
        assert_eq!(w.clone().run_once().await.unwrap(), 1);
        assert_eq!(w.clone().run_once().await.unwrap(), 0);

        let stored = w.repo.stored.lock().unwrap();
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0].1, "article body");
        assert_eq!(
            stored[0].2,
            format!("{:x}", Sha256::digest("article body".as_bytes()))
        );

        let failures = w.repo.failures.lock().unwrap();
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
    async fn one_batch_on_one_host_is_spread_one_second_apart() {
        let jobs = (0..3)
            .map(|i| job(&format!("https://same.test/{i}/ok"), 1))
            .collect();
        let w = worker(jobs, 4);
        let t0 = Instant::now();
        w.clone().run_once().await.unwrap();
        let mut starts: Vec<Duration> = w
            .fetcher
            .started
            .lock()
            .unwrap()
            .iter()
            .map(|(_, at)| *at - t0)
            .collect();
        starts.sort();
        assert_eq!(
            starts,
            [0, 1, 2].map(Duration::from_secs).to_vec(),
            "requests to one host start a second apart"
        );
    }
}
