//! The scraper fallback of the article fetcher.
//!
//! `IngestWorker` asks one `ArticleFetcher` for a page. This decorator answers
//! from the guarded direct fetch, and only when that path cannot read the page
//! does it ask the `PageScraper` port (see
//! docs/superpowers/specs/2026-11-03-page-capture-design.md):
//!
//! - direct text of at least `MIN_CHUNKED_TEXT_CHARS` → the direct result;
//! - a direct member-only preview, a direct `Permanent` failure, or a shorter
//!   text → one scrape;
//! - direct `Transient` → the retry schedule already covers it, no scrape;
//! - direct `Skipped` → not an HTML page, no scrape.
//!
//! A scrape changes the outcome only when it produced text that is at least as
//! good: an exhausted budget or a rejected request leaves the original result
//! in place, so the bookmark lands in `failed`/`skipped` exactly as it would
//! without the scraper. Text the direct fetch already has is never traded for
//! an error: a teaser stays as article text (`source = fetched`) whatever the
//! scrape did — skipped, failed, paused by a 402, or locked — and only a full
//! article replaces it. `preview only (locked)` is recorded when the scraper
//! found a preview and there was no text to keep. A retryable scrape failure
//! becomes `Transient`, so the ingest schedule retries it; with text in hand,
//! that text is stored instead.
//!
//! Spend is counted the way the vendor bills: a page is reserved before the
//! call and refunded only when the vendor answered no 2xx. A 2xx that produced
//! nothing usable keeps its page, and so does a timeout, which the vendor may
//! have billed anyway.
//!
//! Scrapes are paced by `PAGE_SCRAPER_CONCURRENCY` and
//! `PAGE_SCRAPER_RATE_PER_MINUTE`: a job waits for one of that many slots and
//! for its turn in the minute's requests before it reserves a page or calls
//! the vendor, because the plan limits both the renders in flight and the
//! requests a minute. A job that cannot get its turn inside its remaining
//! budget keeps its direct result instead.
//!
//! A 429 is the account's limit, not a page's: it pauses every scrape in the
//! process for the wait the vendor named (capped), and the one job that hit
//! it retries once when that wait fits the budget. A job that arrives during
//! the pause waits it out if it fits, and otherwise goes back to the ingest
//! schedule without a request.

use crate::adapters::article::readability::HtmlExtractor;
use crate::adapters::net::validate_public_url;
use crate::config::{DEFAULT_PAGE_SCRAPER_CONCURRENCY, DEFAULT_PAGE_SCRAPER_RATE_PER_MINUTE};
use crate::domain::chunking::MIN_CHUNKED_TEXT_CHARS;
use crate::domain::ports::article_fetcher::{ArticleFetchError, ArticleFetcher, FetchedArticle};
use crate::domain::ports::page_scraper::{
    PREVIEW_LOCKED_MSG, PageScraper, ScrapeError, ScrapedPage,
};
use crate::domain::ports::scraper_usage_repo::ScraperUsageRepository;
use crate::domain::search::ContentSource;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

/// Mirrors `IngestWorker::JOB_TIMEOUT`: the direct fetch, any scrape and the
/// extraction all have to finish inside one job's budget, so a scrape starts
/// only when this much of it is still left.
const JOB_BUDGET: Duration = Duration::from_secs(120);

/// What the extraction after a scrape can take: the 20 s parse plus a 10 s
/// wait for a free parse slot (the decorator's extractor does not queue
/// longer; see `EXTRACT_SLOT_WAIT`). A scrape that starts with less than this
/// left could be billed and then abandoned by `JOB_TIMEOUT`.
const EXTRACT_RESERVE: Duration = Duration::from_secs(30);

/// How long this decorator's extraction waits for a parse slot. It runs
/// inside an ingest job, so it gives up after 10 s instead of the 60 s the
/// request paths allow: waiting longer leaves no room for the parse.
const EXTRACT_SLOT_WAIT: Duration = Duration::from_secs(10);

/// How long a 402 pauses the fallback before one cheap re-probe: a top-up is
/// noticed within the hour, without a restart.
const OUT_OF_CREDITS_COOLDOWN: Duration = Duration::from_secs(60 * 60);

/// How long a 429 pauses every scrape when the vendor named no wait the app
/// could read.
const DEFAULT_RETRY_DELAY: Duration = Duration::from_secs(2);

/// The longest pause one 429 may set. The vendor's ask is usually about a
/// minute; past this the ingest schedule is the better place to wait, since a
/// job's budget cannot hold it anyway.
const MAX_COOLDOWN: Duration = Duration::from_secs(120);

/// `PAGE_SCRAPER_RATE_PER_MINUTE`, as an interval: one vendor request per
/// `interval`, handed out in order, so the account never sees a minute with
/// more requests than the plan allows however many jobs are queued.
struct RequestPacer {
    interval: Duration,
    /// When the next request may go out. Read and written under the lock, so
    /// callers are served in the order they asked.
    next: tokio::sync::Mutex<Instant>,
}

impl RequestPacer {
    fn new(rate_per_minute: usize) -> Self {
        Self {
            // One a minute is the floor: zero would stop scraping.
            interval: Duration::from_secs_f64(60.0 / rate_per_minute.max(1) as f64),
            next: tokio::sync::Mutex::new(Instant::now()),
        }
    }

    /// The moment this request may go out, or `None` when that is past
    /// `deadline`. Nothing is reserved then, so a job that gives up does not
    /// hold a slot the next one could have used.
    async fn reserve(&self, deadline: Instant) -> Option<Instant> {
        let mut next = self.next.lock().await;
        let slot = (*next).max(Instant::now());
        if slot > deadline {
            return None;
        }
        *next = slot + self.interval;
        Some(slot)
    }
}

/// What one job's turn at the vendor produced.
enum VendorTurn {
    /// The vendor's answer, or the failure that ends this scrape's attempts.
    Answered(Result<ScrapedPage, ScrapeError>),
    /// No turn was taken and nothing was spent: pacing, a missing slot, or
    /// the month's budget left no room. The direct result stands.
    Skipped,
    /// No turn was taken: the vendor's own cooldown outlasts the job's
    /// budget, so the ingest schedule is the right place to wait.
    Throttled,
}

pub struct FallbackFetcher<F, S> {
    direct: F,
    scraper: S,
    extractor: HtmlExtractor,
    usage: Arc<dyn ScraperUsageRepository>,
    /// `PAGE_SCRAPER_MONTHLY_BUDGET_PAGES`; 0 is no cap.
    monthly_budget_pages: u64,
    /// `PAGE_SCRAPER_CONCURRENCY`: how many scrapes this process may have in
    /// flight. The vendor's plan renders only so many pages at once and
    /// answers 429 above that, which a retry meets again.
    scrape_slots: Arc<Semaphore>,
    /// `PAGE_SCRAPER_RATE_PER_MINUTE`: the other limit on the same plan.
    /// Concurrency alone does not keep the account inside it.
    pacer: RequestPacer,
    /// When the next scrape may call the vendor after a 429. Shared by every
    /// job, because the limit is the account's, not a page's.
    rate_limited_until: Mutex<Option<Instant>>,
    /// `PAGE_SCRAPER_TIMEOUT_SECONDS`: one scrape's ceiling.
    scrape_timeout: Duration,
    /// `JOB_BUDGET`, overridable so a test can shorten it.
    job_budget: Duration,
    /// One warning per process when the month's budget is gone, not one per
    /// bookmark.
    budget_notice: AtomicBool,
    /// When the fallback may ask the vendor again after a 402, in epoch
    /// milliseconds; 0 while it never happened.
    out_of_credits_resume_ms: AtomicU64,
    /// `OUT_OF_CREDITS_COOLDOWN`, overridable so a test need not wait an hour.
    out_of_credits_cooldown: Duration,
}

impl<F, S> FallbackFetcher<F, S> {
    /// `extraction_slots` bounds this decorator's blocking parses; the ingest
    /// worker passes its concurrency.
    pub fn new(
        direct: F,
        scraper: S,
        extraction_slots: usize,
        usage: Arc<dyn ScraperUsageRepository>,
        monthly_budget_pages: u64,
        scrape_timeout: Duration,
    ) -> Self {
        Self {
            direct,
            scraper,
            extractor: HtmlExtractor::with_slot_wait(extraction_slots, EXTRACT_SLOT_WAIT),
            usage,
            monthly_budget_pages,
            scrape_slots: Arc::new(Semaphore::new(DEFAULT_PAGE_SCRAPER_CONCURRENCY)),
            pacer: RequestPacer::new(DEFAULT_PAGE_SCRAPER_RATE_PER_MINUTE),
            rate_limited_until: Mutex::new(None),
            scrape_timeout,
            job_budget: JOB_BUDGET,
            budget_notice: AtomicBool::new(false),
            out_of_credits_resume_ms: AtomicU64::new(0),
            out_of_credits_cooldown: OUT_OF_CREDITS_COOLDOWN,
        }
    }

    /// The job budget to respect; a test shortens it instead of waiting two
    /// minutes. Production uses `JOB_BUDGET`, which mirrors the ingest
    /// worker's own timeout.
    #[cfg(test)]
    pub fn with_job_budget(mut self, job_budget: Duration) -> Self {
        self.job_budget = job_budget;
        self
    }

    /// `PAGE_SCRAPER_CONCURRENCY`: how many scrapes this process may have in
    /// flight. Zero would stop scraping, so it is raised to one.
    pub fn with_scrape_concurrency(mut self, concurrency: usize) -> Self {
        self.scrape_slots = Arc::new(Semaphore::new(concurrency.max(1)));
        self
    }

    /// `PAGE_SCRAPER_RATE_PER_MINUTE`: how many vendor requests one minute
    /// may carry. One a minute is the floor, because zero would stop
    /// scraping altogether.
    pub fn with_scrape_rate_per_minute(mut self, rate_per_minute: usize) -> Self {
        self.pacer = RequestPacer::new(rate_per_minute);
        self
    }

    /// The 402 cooldown; a test shortens it instead of waiting an hour.
    #[cfg(test)]
    pub fn with_out_of_credits_cooldown(mut self, cooldown: Duration) -> Self {
        self.out_of_credits_cooldown = cooldown;
        self
    }
}

impl<F, S> FallbackFetcher<F, S>
where
    F: ArticleFetcher,
    S: PageScraper,
{
    /// Note a scraped page that produced no stored text. `billed` keeps the
    /// spend right: a 2xx cost a page, a request that never got one did not.
    async fn note_failure(&self, billed: bool) {
        if let Err(error) = self.usage.record_failure(billed).await {
            tracing::warn!(error = %error, "recording a scraper failure failed");
        }
    }

    /// Epoch milliseconds, for the breaker's deadline.
    fn now_millis() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0)
    }

    /// True while a scrape, with the extraction that follows it, still fits
    /// in what is left of the job's budget.
    fn room_for_a_scrape(&self, started: Instant) -> bool {
        self.job_budget.saturating_sub(started.elapsed())
            >= self.scrape_timeout.saturating_add(EXTRACT_RESERVE)
    }

    /// True while a 402's cooldown runs: one re-probe an hour notices a
    /// top-up without a restart, and without a request per bookmark.
    fn out_of_credits(&self) -> bool {
        let resume = self.out_of_credits_resume_ms.load(Ordering::Relaxed);
        resume != 0 && Self::now_millis() < resume
    }

    /// Pause the fallback for the cooldown, warning once per pause.
    fn note_out_of_credits(&self) {
        let cooldown = u64::try_from(self.out_of_credits_cooldown.as_millis()).unwrap_or(u64::MAX);
        self.out_of_credits_resume_ms.store(
            Self::now_millis().saturating_add(cooldown),
            Ordering::Relaxed,
        );
        tracing::warn!(
            retry_in_seconds = self.out_of_credits_cooldown.as_secs(),
            "page scraper paused: the account is out of credits"
        );
    }

    /// A preview is a definite answer: only a browser capture does better.
    /// With no direct text to keep, the job records why.
    async fn note_preview(&self) -> Result<FetchedArticle, ArticleFetchError> {
        self.note_failure(true).await;
        Err(ArticleFetchError::Permanent(PREVIEW_LOCKED_MSG.to_string()))
    }

    /// The last moment a scrape may start and still finish, with its
    /// extraction, inside the job's budget.
    fn scrape_deadline(&self, started: Instant) -> Instant {
        started
            + self
                .job_budget
                .saturating_sub(self.scrape_timeout.saturating_add(EXTRACT_RESERVE))
    }

    /// When no scrape may call the vendor before, after a 429.
    fn rate_limited_until(&self) -> Option<Instant> {
        *self
            .rate_limited_until
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A 429 pauses every scrape in the process for the wait the vendor
    /// named, capped so one account-wide ask does not freeze the month's
    /// queue. Returns the wait, for the caller's log line.
    fn note_rate_limit(&self, retry_after: Option<Duration>) -> Duration {
        let wait = retry_after.unwrap_or(DEFAULT_RETRY_DELAY).min(MAX_COOLDOWN);
        let until = Instant::now() + wait;
        let mut cooldown = self
            .rate_limited_until
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !matches!(*cooldown, Some(current) if current >= until) {
            *cooldown = Some(until);
        }
        wait
    }

    /// Wait out a cooldown a 429 set, or say it outlasts the job.
    async fn wait_out_cooldown(&self, url: &str, deadline: Instant) -> Result<(), VendorTurn> {
        let Some(until) = self.rate_limited_until() else {
            return Ok(());
        };
        if until > deadline {
            tracing::info!(
                host = %log_host(url),
                wait_seconds = until.saturating_duration_since(Instant::now()).as_secs(),
                "page scraper skipped: the vendor's cooldown outlasts the job budget"
            );
            return Err(VendorTurn::Throttled);
        }
        tokio::time::sleep_until(until).await;
        Ok(())
    }

    /// One of `PAGE_SCRAPER_CONCURRENCY` slots, if one comes free before the
    /// job's budget runs out. `None` keeps the direct result.
    async fn take_slot(&self, url: &str, deadline: Instant) -> Option<OwnedSemaphorePermit> {
        match tokio::time::timeout_at(deadline, self.scrape_slots.clone().acquire_owned()).await {
            Ok(Ok(permit)) => Some(permit),
            // No slot came free in time; the semaphore is never closed.
            _ => {
                tracing::info!(
                    host = %log_host(url),
                    "page scraper skipped: no scrape slot before the job budget ran out"
                );
                None
            }
        }
    }

    /// One job's turn at the vendor: the cooldown, the pace, a slot and a
    /// reservation, then one request. Every wait comes before the reservation
    /// and is bounded by the job's budget, because a wait is not spend.
    async fn take_turn(&self, url: &str, started: Instant) -> VendorTurn {
        let deadline = self.scrape_deadline(started);
        if let Err(turn) = self.wait_out_cooldown(url, deadline).await {
            return turn;
        }
        let Some(slot) = self.pacer.reserve(deadline).await else {
            tracing::info!(
                host = %log_host(url),
                "page scraper skipped: the scrape rate leaves no room in the job budget"
            );
            return VendorTurn::Skipped;
        };
        tokio::time::sleep_until(slot).await;
        // A 429 from another job may have landed while this one waited.
        if let Err(turn) = self.wait_out_cooldown(url, deadline).await {
            return turn;
        }
        let Some(permit) = self.take_slot(url, deadline).await else {
            return VendorTurn::Skipped;
        };
        // The waits may have used the room the scrape itself needs.
        if !self.room_for_a_scrape(started) {
            tracing::info!(
                host = %log_host(url),
                "page scraper skipped: the waits used the job budget"
            );
            return VendorTurn::Skipped;
        }
        match self.usage.reserve_page(self.monthly_budget_pages).await {
            Ok(true) => {}
            Ok(false) => {
                if !self.budget_notice.swap(true, Ordering::Relaxed) {
                    tracing::warn!(
                        budget = self.monthly_budget_pages,
                        "page scraper skipped: this month's page budget is used up"
                    );
                }
                return VendorTurn::Skipped;
            }
            Err(error) => {
                // Spending without accounting is worse than not spending.
                tracing::warn!(
                    error = %error,
                    "page scraper skipped: the monthly budget could not be checked"
                );
                return VendorTurn::Skipped;
            }
        }
        self.request_with_one_retry(url, permit, deadline).await
    }

    /// One request under a slot, and one more when a 429's ask fits what the
    /// job has left. The slot is released before any wait, and the page stays
    /// reserved across both requests: it is the same page either way.
    async fn request_with_one_retry(
        &self,
        url: &str,
        permit: OwnedSemaphorePermit,
        deadline: Instant,
    ) -> VendorTurn {
        let error = match self.request(url).await {
            Ok(page) => return VendorTurn::Answered(Ok(page)),
            Err(error) => error,
        };
        // Only a 429 earns the single in-job retry; every other answer is
        // this scrape's.
        let retry_after = match error {
            ScrapeError::RateLimited { retry_after } => retry_after,
            other => return VendorTurn::Answered(Err(other)),
        };
        let wait = self.note_rate_limit(retry_after);
        let refused = ScrapeError::RateLimited { retry_after };
        let ask = retry_after.map_or_else(|| "none".to_string(), |ask| ask.as_secs().to_string());
        // The request is over; the slot goes back while this job waits.
        drop(permit);
        // The vendor's ask must fit the job's budget beside the scrape and
        // its extraction; past that the ingest schedule waits instead.
        if Instant::now() + wait > deadline {
            tracing::info!(
                host = %log_host(url),
                retry_after_seconds = %ask,
                "the vendor rate-limited the scraper: the ask outlasts the job budget"
            );
            return VendorTurn::Answered(Err(refused));
        }
        tracing::info!(
            host = %log_host(url),
            retry_after_seconds = %ask,
            "the vendor rate-limited the scraper: retrying once inside the job"
        );
        if self.wait_out_cooldown(url, deadline).await.is_err() {
            return VendorTurn::Answered(Err(refused));
        }
        let Some(slot) = self.pacer.reserve(deadline).await else {
            tracing::info!(
                host = %log_host(url),
                "page scraper skipped: the scrape rate leaves no room in the job budget"
            );
            return VendorTurn::Answered(Err(refused));
        };
        tokio::time::sleep_until(slot).await;
        let Some(_permit) = self.take_slot(url, deadline).await else {
            return VendorTurn::Answered(Err(refused));
        };
        match self.request(url).await {
            Ok(page) => VendorTurn::Answered(Ok(page)),
            Err(ScrapeError::RateLimited { retry_after }) => {
                // Still throttled: the newer ask replaces the cooldown, and
                // the ingest schedule is the place to wait now.
                self.note_rate_limit(retry_after);
                tracing::info!(
                    host = %log_host(url),
                    retry_after_seconds = %retry_after
                        .map_or_else(|| "none".to_string(), |ask| ask.as_secs().to_string()),
                    "the vendor rate-limited the scraper again: the ingest schedule waits"
                );
                VendorTurn::Answered(Err(ScrapeError::RateLimited { retry_after }))
            }
            Err(error) => VendorTurn::Answered(Err(error)),
        }
    }

    /// One request to the vendor, bounded by the scrape timeout. The adapter
    /// bounds itself too; this is the job's hard stop.
    async fn request(&self, url: &str) -> Result<ScrapedPage, ScrapeError> {
        match tokio::time::timeout(self.scrape_timeout, self.scraper.scrape(url)).await {
            Ok(result) => result,
            // No answer was seen, but a timeout usually means the vendor kept
            // working: assume the page was billed rather than give the
            // month's budget away.
            Err(_elapsed) => Err(ScrapeError::Retryable {
                message: "page scraper timed out".to_string(),
                billed: true,
            }),
        }
    }

    /// One scrape, or the original result when it cannot help. Text that is
    /// already stored is never traded for an error: only a scrape that finds
    /// better text replaces it.
    async fn with_scrape(
        &self,
        url: &str,
        original: Result<FetchedArticle, ArticleFetchError>,
        started: Instant,
    ) -> Result<FetchedArticle, ArticleFetchError> {
        let outcome = self.scrape_once(url, &original, started).await;
        match (original, outcome) {
            (Ok(article), Err(_)) => Ok(article),
            (_, outcome) => outcome,
        }
    }

    /// The scrape's own verdict. `with_scrape` decides what a failure means
    /// when the direct fetch already has text to store.
    async fn scrape_once(
        &self,
        url: &str,
        original: &Result<FetchedArticle, ArticleFetchError>,
        started: Instant,
    ) -> Result<FetchedArticle, ArticleFetchError> {
        if !self.scraper.enabled() {
            return original.clone();
        }
        // Past 402, every later call would hit the same wall: stop asking.
        if self.out_of_credits() {
            tracing::debug!(%url, "page scraper skipped: the account is out of credits");
            return original.clone();
        }
        // A URL the app may not fetch, the vendor must not be asked to fetch
        // either: the vendor would otherwise be an SSRF proxy.
        if let Err(refused) = validate_public_url(url) {
            tracing::debug!(%url, reason = %refused, "page scraper skipped: URL is not public");
            return original.clone();
        }
        // A scrape that cannot finish, with its extraction, inside the job's
        // budget would be billed and then abandoned, and the retry would pay
        // again.
        if !self.room_for_a_scrape(started) {
            tracing::info!(
                host = %log_host(url),
                "page scraper skipped: no job budget left for a scrape"
            );
            return original.clone();
        }
        // Pacing, the vendor's cooldown, the slot and the page reservation all
        // happen inside the turn, and none of the waits spends anything.
        let page = match self.take_turn(url, started).await {
            VendorTurn::Answered(Ok(page)) => page,
            VendorTurn::Answered(Err(error)) => {
                self.note_failure(error.billed()).await;
                // Info, with the host and the error class only: a failed
                // scrape that prod cannot see is a queue that stalls quietly.
                tracing::info!(
                    host = %log_host(url),
                    error_class = scrape_error_class(&error),
                    billed = error.billed(),
                    error = %error,
                    "page scrape failed"
                );
                let message = error.message().to_string();
                return match error {
                    ScrapeError::OutOfCredits => {
                        self.note_out_of_credits();
                        original.clone()
                    }
                    // The vendor said "not now": the ingest retry schedule is
                    // the right place to wait.
                    ScrapeError::Retryable { .. } | ScrapeError::RateLimited { .. } => {
                        Err(ArticleFetchError::Transient(message))
                    }
                    // The same request will fail the same way; the direct
                    // result is the honest outcome.
                    ScrapeError::Final { .. } => original.clone(),
                };
            }
            // A wait is not spend, and a job that never reached the vendor
            // keeps the direct result.
            VendorTurn::Skipped => return original.clone(),
            // The vendor's cooldown outlasts the job: no request went out and
            // no page was reserved, so the schedule is the place to wait.
            VendorTurn::Throttled => {
                tracing::info!(
                    host = %log_host(url),
                    "page scraper skipped: the vendor's rate limit outlasts the job budget"
                );
                return Err(ArticleFetchError::Transient(
                    "page scraper is waiting out the vendor's rate limit".to_string(),
                ));
            }
        };

        // From here the page was paid for, whatever comes of it.
        if page.locked {
            tracing::debug!(%url, "page scraper got a preview only");
            return self.note_preview().await;
        }
        // The app's own Readability pass on the vendor's HTML, so a scrape
        // and a capture produce the same text for the same page. It also
        // classifies a member-only preview that the vendor did not flag.
        let extracted = match self
            .extractor
            .extract_fetched(page.html.into_bytes(), Some("text/html"), &page.final_url)
            .await
        {
            Ok(extracted) => extracted,
            Err(error) => {
                self.note_failure(true).await;
                tracing::info!(
                    host = %log_host(url),
                    error = %error,
                    "the scraped page has no article"
                );
                return original.clone();
            }
        };
        if extracted.preview {
            tracing::debug!(%url, "page scraper got a member-only preview");
            return self.note_preview().await;
        }
        if !scrape_improves(original, &extracted.text) {
            self.note_failure(true).await;
            tracing::debug!(%url, "the scraped text does not beat the direct result");
            return original.clone();
        }
        tracing::debug!(
            %url,
            chars = extracted.text.chars().count(),
            "article stored from the page scraper"
        );
        Ok(FetchedArticle {
            text: extracted.text,
            final_url: page.final_url,
            html_bytes: page.html_bytes,
            source: ContentSource::Scraped,
            preview: false,
        })
    }
}

/// A page counts as an article past the chunking threshold, which is also
/// where a short fetch stops being worth a scrape.
fn is_article(text: &str) -> bool {
    text.chars().count() >= MIN_CHUNKED_TEXT_CHARS
}

/// The page's host, for a log line: a hostname says which site failed without
/// copying the article's own path into a log. Shared with the ingest worker's
/// failure line, which names the same host.
pub(crate) fn log_host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_ascii_lowercase))
        .unwrap_or_else(|| "unknown".to_string())
}

/// The class a scrape failed in, as a log field: the retry decision is made
/// per class, so a stuck queue is read from one line.
fn scrape_error_class(error: &ScrapeError) -> &'static str {
    match error {
        ScrapeError::Retryable { .. } => "retryable",
        ScrapeError::Final { .. } => "final",
        ScrapeError::OutOfCredits => "out_of_credits",
        ScrapeError::RateLimited { .. } => "rate_limited",
    }
}

/// True when the scraped text is worth storing over the direct result: any
/// text at all after a failure or after a teaser, and strictly more text than
/// a short success. Length is the wrong measure for a teaser — it is padded
/// with the preview — so a preview is replaced by any full text.
fn scrape_improves(original: &Result<FetchedArticle, ArticleFetchError>, text: &str) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    match original {
        Err(_) => true,
        Ok(article) if article.preview => true,
        Ok(article) => text.chars().count() > article.text.chars().count(),
    }
}

impl<F, S> ArticleFetcher for FallbackFetcher<F, S>
where
    F: ArticleFetcher,
    S: PageScraper,
{
    fn fetch(
        &self,
        url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<FetchedArticle, ArticleFetchError>> + Send + '_>> {
        let url = url.to_string();
        Box::pin(async move {
            let started = Instant::now();
            match self.direct.fetch(&url).await {
                // The retry schedule (Transient) or the non-HTML verdict
                // (Skipped) already decides these; a scrape would not.
                Err(ArticleFetchError::Transient(message)) => {
                    Err(ArticleFetchError::Transient(message))
                }
                Err(ArticleFetchError::Skipped(message)) => {
                    Err(ArticleFetchError::Skipped(message))
                }
                Err(permanent) => self.with_scrape(&url, Err(permanent), started).await,
                // A preview is checked before the length rule: a
                // 1,900-character teaser is not an article, so the scraper
                // gets a chance at the full one. Without a scraper this is the
                // direct result, teaser and all, exactly as before.
                Ok(article) if article.preview => {
                    self.with_scrape(&url, Ok(article), started).await
                }
                Ok(article) if is_article(&article.text) => Ok(article),
                Ok(short) => self.with_scrape(&url, Ok(short), started).await,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error::DomainError;
    use crate::domain::ports::page_scraper::ScrapedPage;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    /// The fixture article the extractor can read, long enough to be stored
    /// and chunked.
    const ARTICLE_HTML: &str = r#"<!doctype html><html><head><title>Borrowing in Rust</title></head>
        <body>
          <nav><a href="/">Home</a> <a href="/about">About</a></nav>
          <article>
            <h1>Borrowing in Rust</h1>
            <p>The borrow checker enforces that every reference is valid for as long as it is
               used, and that mutable references are never aliased. This paragraph is long enough
               for a readability heuristic to pick it as the main content of the page.</p>
            <p>Lifetimes are the names the compiler gives to those regions of validity. Most of the
               time they are inferred, and you only write them when a function returns a
               reference derived from one of several arguments.</p>
            <p>Interior mutability, through Cell and RefCell, moves the aliasing check from compile
               time to run time, which is occasionally exactly what a data structure needs.</p>
          </article>
          <footer>Copyright footer text that should not matter</footer>
        </body></html>"#;

    /// A Medium page whose own entry is locked: the extractor must classify it
    /// as a preview even though the vendor did not say so.
    fn locked_article_html() -> String {
        ARTICLE_HTML.replace(
            "<h1>Borrowing in Rust</h1>",
            r#"<h1>Borrowing in Rust</h1><script>window.__APOLLO_STATE__={"Post:x":{"canonicalUrl":"https://medium.com/@writer/a-post","isLocked":true}}</script>"#,
        )
    }

    const PUBLIC_URL: &str = "https://medium.com/@writer/a-post";

    fn article(text: &str) -> FetchedArticle {
        FetchedArticle {
            text: text.to_string(),
            final_url: PUBLIC_URL.to_string(),
            html_bytes: 1234,
            source: ContentSource::Fetched,
            preview: false,
        }
    }

    fn preview_article(text: &str) -> FetchedArticle {
        FetchedArticle {
            preview: true,
            ..article(text)
        }
    }

    /// Answers every fetch with one prepared result.
    struct FakeDirect {
        result: Result<FetchedArticle, ArticleFetchError>,
    }

    impl ArticleFetcher for FakeDirect {
        fn fetch(
            &self,
            _url: &str,
        ) -> Pin<Box<dyn Future<Output = Result<FetchedArticle, ArticleFetchError>> + Send + '_>>
        {
            let result = self.result.clone();
            Box::pin(async move { result })
        }
    }

    /// One prepared scrape answer, handed out in order.
    #[derive(Clone)]
    enum Answer {
        Page(ScrapedPage),
        Fail(ScrapeError),
        Slow(Duration, ScrapedPage),
    }

    struct FakeScraper {
        enabled: bool,
        calls: AtomicUsize,
        answers: Mutex<VecDeque<Answer>>,
    }

    impl FakeScraper {
        fn page(html: &str) -> Arc<Self> {
            Self::answering(Answer::Page(ScrapedPage {
                html: html.to_string(),
                final_url: PUBLIC_URL.to_string(),
                html_bytes: html.len(),
                locked: false,
            }))
        }

        fn locked_page() -> Arc<Self> {
            Self::answering(Answer::Page(ScrapedPage {
                html: "<html><p>teaser</p></html>".to_string(),
                final_url: PUBLIC_URL.to_string(),
                html_bytes: 24,
                locked: true,
            }))
        }

        fn failing(error: ScrapeError) -> Arc<Self> {
            Self::answering(Answer::Fail(error))
        }

        /// Answers each call in turn; the last one repeats, so a breaker test
        /// can trip once and probe again.
        fn answering_all(answers: Vec<Answer>) -> Arc<Self> {
            Arc::new(Self {
                enabled: true,
                calls: AtomicUsize::new(0),
                answers: Mutex::new(answers.into()),
            })
        }

        fn slow(page: ScrapedPage) -> Arc<Self> {
            Self::answering(Answer::Slow(Duration::from_secs(5), page))
        }

        fn answering(answer: Answer) -> Arc<Self> {
            Self::answering_all(vec![answer])
        }

        fn disabled() -> Arc<Self> {
            Arc::new(Self {
                enabled: false,
                calls: AtomicUsize::new(0),
                answers: Mutex::new(VecDeque::new()),
            })
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
    }

    impl PageScraper for FakeScraper {
        fn enabled(&self) -> bool {
            self.enabled
        }

        fn scrape(
            &self,
            _url: &str,
        ) -> crate::domain::ports::page_scraper::BoxFuture<'_, Result<ScrapedPage, ScrapeError>>
        {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let answer = {
                let mut answers = self.answers.lock().unwrap();
                if answers.len() > 1 {
                    answers.pop_front()
                } else {
                    // The last answer repeats, so a test can drive two calls.
                    answers.front().cloned()
                }
                .expect("the fake scraper was given an answer")
            };
            Box::pin(async move {
                match answer {
                    Answer::Page(page) => Ok(page),
                    Answer::Fail(error) => Err(error),
                    Answer::Slow(delay, page) => {
                        tokio::time::sleep(delay).await;
                        Ok(page)
                    }
                }
            })
        }
    }

    /// The budget and the failure counter the decorator writes through. A
    /// `billed` failure keeps its page; an unbilled one is refunded.
    struct FakeUsage {
        can_reserve: bool,
        broken: bool,
        reserves: AtomicUsize,
        recorded: Mutex<Vec<bool>>,
        budgets: Mutex<Vec<u64>>,
    }

    impl FakeUsage {
        fn allowing() -> Arc<Self> {
            Self::new(true, false)
        }

        fn exhausted() -> Arc<Self> {
            Self::new(false, false)
        }

        fn broken() -> Arc<Self> {
            Self::new(true, true)
        }

        fn new(can_reserve: bool, broken: bool) -> Arc<Self> {
            Arc::new(Self {
                can_reserve,
                broken,
                reserves: AtomicUsize::new(0),
                recorded: Mutex::new(Vec::new()),
                budgets: Mutex::new(Vec::new()),
            })
        }

        fn reserves(&self) -> usize {
            self.reserves.load(Ordering::Relaxed)
        }

        /// The recorded failures, each with the billing flag it was given.
        fn failures(&self) -> Vec<bool> {
            self.recorded.lock().unwrap().clone()
        }
    }

    impl ScraperUsageRepository for FakeUsage {
        fn reserve_page(
            &self,
            budget: u64,
        ) -> crate::domain::ports::scraper_usage_repo::BoxFuture<'_, Result<bool, DomainError>>
        {
            self.budgets.lock().unwrap().push(budget);
            let broken = self.broken;
            let can_reserve = self.can_reserve;
            if !broken {
                self.reserves.fetch_add(1, Ordering::Relaxed);
            }
            Box::pin(async move {
                if broken {
                    Err(DomainError::Internal("budget database unreachable".into()))
                } else {
                    Ok(can_reserve)
                }
            })
        }

        fn record_failure(
            &self,
            billed: bool,
        ) -> crate::domain::ports::scraper_usage_repo::BoxFuture<'_, Result<(), DomainError>>
        {
            self.recorded.lock().unwrap().push(billed);
            Box::pin(std::future::ready(Ok(())))
        }
    }

    fn fetcher<S>(
        direct: FakeDirect,
        scraper: S,
        usage: Arc<FakeUsage>,
        budget: u64,
    ) -> FallbackFetcher<FakeDirect, S> {
        FallbackFetcher::new(direct, scraper, 1, usage, budget, Duration::from_secs(30))
    }

    #[tokio::test]
    async fn a_long_direct_result_is_stored_without_a_scrape() {
        let scraper = FakeScraper::page(ARTICLE_HTML);
        let usage = FakeUsage::allowing();
        let direct = FakeDirect {
            result: Ok(article(&"x".repeat(MIN_CHUNKED_TEXT_CHARS))),
        };
        let result = fetcher(direct, scraper.clone(), usage.clone(), 0)
            .fetch(PUBLIC_URL)
            .await
            .unwrap();
        assert_eq!(result.source, ContentSource::Fetched);
        assert_eq!(scraper.calls(), 0);
        assert_eq!(usage.reserves(), 0, "no budget is touched");
    }

    /// One character under the chunking threshold is a short page, and the
    /// scraper gets its chance.
    #[tokio::test]
    async fn a_text_just_under_the_threshold_is_scraped() {
        let scraper = FakeScraper::page(ARTICLE_HTML);
        let usage = FakeUsage::allowing();
        let direct = FakeDirect {
            result: Ok(article(&"x".repeat(MIN_CHUNKED_TEXT_CHARS - 1))),
        };
        let result = fetcher(direct, scraper.clone(), usage.clone(), 100)
            .fetch(PUBLIC_URL)
            .await
            .unwrap();
        assert_eq!(scraper.calls(), 1);
        assert_eq!(result.source, ContentSource::Scraped);
        assert!(!result.preview);
        assert!(result.text.contains("borrow checker"));
        assert_eq!(
            usage.budgets.lock().unwrap().as_slice(),
            [100],
            "the configured budget reaches the reservation"
        );
    }

    #[tokio::test]
    async fn a_permanent_failure_is_retried_through_the_scraper() {
        let scraper = FakeScraper::page(ARTICLE_HTML);
        let usage = FakeUsage::allowing();
        let direct = FakeDirect {
            result: Err(ArticleFetchError::Permanent("HTTP 403".into())),
        };
        let result = fetcher(direct, scraper.clone(), usage.clone(), 0)
            .fetch(PUBLIC_URL)
            .await
            .unwrap();
        assert_eq!(scraper.calls(), 1);
        assert_eq!(result.source, ContentSource::Scraped);
        assert_eq!(result.final_url, PUBLIC_URL);
        assert!(
            usage.failures().is_empty(),
            "a stored article is not a failure"
        );
    }

    /// Transient results have their own retry schedule, and a non-HTML page
    /// stays non-HTML whatever renders it.
    #[tokio::test]
    async fn transient_and_skipped_results_never_reach_the_scraper() {
        for error in [
            ArticleFetchError::Transient("HTTP 503".into()),
            ArticleFetchError::Skipped("not an HTML page: application/pdf".into()),
        ] {
            let scraper = FakeScraper::page(ARTICLE_HTML);
            let usage = FakeUsage::allowing();
            let direct = FakeDirect {
                result: Err(error.clone()),
            };
            let result = fetcher(direct, scraper.clone(), usage.clone(), 0)
                .fetch(PUBLIC_URL)
                .await
                .unwrap_err();
            assert_eq!(result, error);
            assert_eq!(scraper.calls(), 0);
        }
    }

    #[tokio::test]
    async fn a_disabled_scraper_changes_nothing() {
        let scraper = FakeScraper::disabled();
        let usage = FakeUsage::allowing();
        let direct = FakeDirect {
            result: Err(ArticleFetchError::Permanent("HTTP 403".into())),
        };
        let result = fetcher(direct, scraper.clone(), usage.clone(), 0)
            .fetch(PUBLIC_URL)
            .await
            .unwrap_err();
        assert_eq!(result, ArticleFetchError::Permanent("HTTP 403".into()));
        assert_eq!(scraper.calls(), 0);
        assert_eq!(usage.reserves(), 0);
    }

    /// The vendor never fetches what the app itself refuses: a loopback or
    /// private URL would make it an SSRF proxy.
    #[tokio::test]
    async fn a_non_public_url_is_never_sent_to_the_vendor() {
        for url in [
            "http://127.0.0.1:8080/admin",
            "http://localhost/a",
            "http://169.254.169.254/latest/meta-data",
            "ftp://example.com/a",
            "not a url",
        ] {
            let scraper = FakeScraper::page(ARTICLE_HTML);
            let usage = FakeUsage::allowing();
            let direct = FakeDirect {
                result: Err(ArticleFetchError::Permanent("HTTP 403".into())),
            };
            let result = fetcher(direct, scraper.clone(), usage.clone(), 0)
                .fetch(url)
                .await
                .unwrap_err();
            assert_eq!(
                result,
                ArticleFetchError::Permanent("HTTP 403".into()),
                "{url}"
            );
            assert_eq!(scraper.calls(), 0, "{url}");
            assert_eq!(usage.reserves(), 0, "{url}");
        }
    }

    #[tokio::test]
    async fn an_exhausted_budget_skips_the_scrape() {
        let scraper = FakeScraper::page(ARTICLE_HTML);
        let usage = FakeUsage::exhausted();
        let direct = FakeDirect {
            result: Err(ArticleFetchError::Permanent("HTTP 403".into())),
        };
        let fetcher = fetcher(direct, scraper.clone(), usage.clone(), 200);
        // Twice: the "log once" path must not change the outcome either way.
        for _ in 0..2 {
            let result = fetcher.fetch(PUBLIC_URL).await.unwrap_err();
            assert_eq!(result, ArticleFetchError::Permanent("HTTP 403".into()));
        }
        assert_eq!(scraper.calls(), 0);
        assert_eq!(
            usage.reserves(),
            2,
            "the budget is consulted, the vendor is never called"
        );
    }

    /// Spending without accounting is worse than not spending: a broken
    /// budget table skips the scrape instead of calling the vendor.
    #[tokio::test]
    async fn an_unreadable_budget_skips_the_scrape() {
        let scraper = FakeScraper::page(ARTICLE_HTML);
        let usage = FakeUsage::broken();
        let direct = FakeDirect {
            result: Err(ArticleFetchError::Permanent("HTTP 403".into())),
        };
        let result = fetcher(direct, scraper.clone(), usage.clone(), 200)
            .fetch(PUBLIC_URL)
            .await
            .unwrap_err();
        assert_eq!(result, ArticleFetchError::Permanent("HTTP 403".into()));
        assert_eq!(scraper.calls(), 0);
        assert!(usage.failures().is_empty());
    }

    /// A 429 or a timeout is "come back later", not "this page is hopeless":
    /// the ingest retry schedule owns the wait, and no page was billed.
    #[tokio::test]
    async fn a_retryable_scrape_error_becomes_transient_and_is_refunded() {
        let scraper = FakeScraper::failing(ScrapeError::Retryable {
            message: "Firecrawl API returned HTTP 429 Too Many Requests".into(),
            billed: false,
        });
        let usage = FakeUsage::allowing();
        let direct = FakeDirect {
            result: Err(ArticleFetchError::Permanent("HTTP 403".into())),
        };
        let result = fetcher(direct, scraper.clone(), usage.clone(), 0)
            .fetch(PUBLIC_URL)
            .await
            .unwrap_err();
        match result {
            ArticleFetchError::Transient(message) => assert!(message.contains("429"), "{message}"),
            other => panic!("expected a transient error, got {other:?}"),
        }
        assert_eq!(
            usage.failures(),
            vec![false],
            "an unbilled request is refunded"
        );
    }

    #[tokio::test]
    async fn a_final_scrape_error_keeps_the_direct_result_and_is_refunded() {
        let scraper = FakeScraper::failing(ScrapeError::Final {
            message: "Firecrawl API returned HTTP 400 Bad Request".into(),
            billed: false,
        });
        let usage = FakeUsage::allowing();
        let direct = FakeDirect {
            result: Err(ArticleFetchError::Permanent("HTTP 403".into())),
        };
        let result = fetcher(direct, scraper.clone(), usage.clone(), 0)
            .fetch(PUBLIC_URL)
            .await
            .unwrap_err();
        assert_eq!(result, ArticleFetchError::Permanent("HTTP 403".into()));
        assert_eq!(usage.failures(), vec![false]);
    }

    /// Text in hand is never traded for an error: a retryable scrape failure
    /// on a page that already has a short article stores the article.
    #[tokio::test]
    async fn a_retryable_scrape_error_keeps_a_short_direct_result() {
        let scraper = FakeScraper::failing(ScrapeError::Retryable {
            message: "Firecrawl API returned HTTP 503 Service Unavailable".into(),
            billed: false,
        });
        let usage = FakeUsage::allowing();
        let short = article("A page with a little text");
        let direct = FakeDirect {
            result: Ok(short.clone()),
        };
        let result = fetcher(direct, scraper.clone(), usage.clone(), 0)
            .fetch(PUBLIC_URL)
            .await
            .unwrap();
        assert_eq!(result, short);
        assert_eq!(usage.failures(), vec![false], "the request was refunded");
    }

    /// ... and neither is a teaser, whatever a paused 402 or a failed scrape
    /// has to say.
    #[tokio::test]
    async fn a_paused_402_keeps_the_direct_teaser() {
        let scraper = FakeScraper::failing(ScrapeError::OutOfCredits);
        let usage = FakeUsage::allowing();
        let teaser = preview_article(&"x".repeat(1_900));
        let direct = FakeDirect {
            result: Ok(preview_article(&"x".repeat(1_900))),
        };
        let fetcher = fetcher(direct, scraper.clone(), usage.clone(), 200);
        for _ in 0..2 {
            let result = fetcher.fetch(PUBLIC_URL).await.unwrap();
            assert_eq!(result, teaser);
            assert_eq!(result.source, ContentSource::Fetched);
        }
        assert_eq!(scraper.calls(), 1, "the second call is paused");
        assert_eq!(usage.failures(), vec![false], "the 402 was not billed");
    }

    /// The default configuration still scrapes after a slow direct fetch: the
    /// extraction reserve leaves room for a 30 s scrape once 12 s are gone.
    #[tokio::test]
    async fn the_default_config_scrapes_after_a_twelve_second_direct_fetch() {
        struct SlowDirect(Duration);
        impl ArticleFetcher for SlowDirect {
            fn fetch(
                &self,
                _url: &str,
            ) -> Pin<Box<dyn Future<Output = Result<FetchedArticle, ArticleFetchError>> + Send + '_>>
            {
                let delay = self.0;
                Box::pin(async move {
                    tokio::time::sleep(delay).await;
                    Err(ArticleFetchError::Permanent("HTTP 403".into()))
                })
            }
        }

        let scraper = FakeScraper::page(ARTICLE_HTML);
        let usage = FakeUsage::allowing();
        let scraper_timeout =
            Duration::from_secs(crate::config::DEFAULT_PAGE_SCRAPER_TIMEOUT_SECONDS);
        let result = FallbackFetcher::new(
            SlowDirect(Duration::from_secs(12)),
            scraper.clone(),
            1,
            usage.clone(),
            200,
            scraper_timeout,
        )
        .fetch(PUBLIC_URL)
        .await
        .unwrap();
        assert_eq!(scraper.calls(), 1, "12 s in, there is still time to scrape");
        assert_eq!(result.source, ContentSource::Scraped);
    }

    /// Out of credits is not per-page: the next bookmark would burn another
    /// request for the same answer. The pause is a cooldown, not the rest of
    /// the month, so one re-probe an hour notices a top-up without a restart.
    #[tokio::test]
    async fn out_of_credits_pauses_the_fallback_then_reprobes_after_the_cooldown() {
        let scraper = FakeScraper::answering_all(vec![
            Answer::Fail(ScrapeError::OutOfCredits),
            Answer::Fail(ScrapeError::OutOfCredits),
            Answer::Page(ScrapedPage {
                html: ARTICLE_HTML.to_string(),
                final_url: PUBLIC_URL.to_string(),
                html_bytes: ARTICLE_HTML.len(),
                locked: false,
            }),
        ]);
        let usage = FakeUsage::allowing();
        let direct = FakeDirect {
            result: Err(ArticleFetchError::Permanent("HTTP 403".into())),
        };
        let fetcher = fetcher(direct, scraper.clone(), usage.clone(), 200)
            .with_out_of_credits_cooldown(Duration::from_millis(50))
            // The 402 pause is the cap under test, not the pace.
            .with_scrape_rate_per_minute(600);

        // First 402 trips the pause; the next calls do not reach the vendor.
        for _ in 0..2 {
            let result = fetcher.fetch(PUBLIC_URL).await.unwrap_err();
            assert_eq!(result, ArticleFetchError::Permanent("HTTP 403".into()));
        }
        assert_eq!(scraper.calls(), 1, "the pause stops later calls");
        assert_eq!(usage.reserves(), 1, "and reserves no more pages");
        assert_eq!(usage.failures(), vec![false], "no page was billed");

        // Past the cooldown one probe goes out; still 402, so it re-arms.
        tokio::time::sleep(Duration::from_millis(80)).await;
        let result = fetcher.fetch(PUBLIC_URL).await.unwrap_err();
        assert_eq!(result, ArticleFetchError::Permanent("HTTP 403".into()));
        assert_eq!(scraper.calls(), 2, "the probe after the cooldown went out");

        // A top-up is noticed on the next probe, and the scrape is stored.
        tokio::time::sleep(Duration::from_millis(80)).await;
        let result = fetcher.fetch(PUBLIC_URL).await.unwrap();
        assert_eq!(result.source, ContentSource::Scraped);
        assert_eq!(scraper.calls(), 3);
    }

    /// A member-only preview is a definite answer: the error says so and the
    /// bookmark is not retried, because only a browser capture can do better.
    /// The vendor charged for the page, so it is not refunded.
    #[tokio::test]
    async fn a_locked_preview_is_recorded_and_not_retried() {
        let scraper = FakeScraper::locked_page();
        let usage = FakeUsage::allowing();
        let direct = FakeDirect {
            result: Err(ArticleFetchError::Permanent("HTTP 403".into())),
        };
        let result = fetcher(direct, scraper.clone(), usage.clone(), 0)
            .fetch(PUBLIC_URL)
            .await
            .unwrap_err();
        assert_eq!(
            result,
            ArticleFetchError::Permanent(PREVIEW_LOCKED_MSG.to_string())
        );
        assert_eq!(scraper.calls(), 1);
        assert_eq!(
            usage.failures(),
            vec![true],
            "a billed page is not refunded"
        );
    }

    /// A teaser the direct fetch already has is better than nothing when the
    /// scraper confirms the page is member-only: it is stored, and the billed
    /// page is counted.
    #[tokio::test]
    async fn a_scraped_preview_keeps_the_direct_teaser() {
        let scraper = FakeScraper::locked_page();
        let usage = FakeUsage::allowing();
        let teaser = preview_article(&"x".repeat(1_900));
        let direct = FakeDirect {
            result: Ok(teaser.clone()),
        };
        let result = fetcher(direct, scraper.clone(), usage.clone(), 0)
            .fetch(PUBLIC_URL)
            .await
            .unwrap();
        assert_eq!(result, teaser);
        assert_eq!(result.source, ContentSource::Fetched);
        assert_eq!(scraper.calls(), 1);
        assert_eq!(usage.failures(), vec![true], "the scrape was billed");
    }

    /// The vendor did not flag the page, but its HTML says it is member-only:
    /// the extractor's verdict wins the same way.
    #[tokio::test]
    async fn a_scraped_member_only_page_is_a_preview_too() {
        let scraper = FakeScraper::page(&locked_article_html());
        let usage = FakeUsage::allowing();
        let direct = FakeDirect {
            result: Err(ArticleFetchError::Permanent("HTTP 403".into())),
        };
        let result = fetcher(direct, scraper.clone(), usage.clone(), 0)
            .fetch(PUBLIC_URL)
            .await
            .unwrap_err();
        assert_eq!(
            result,
            ArticleFetchError::Permanent(PREVIEW_LOCKED_MSG.to_string())
        );
        assert_eq!(usage.failures(), vec![true]);
    }

    /// A page whose parse finds no article leaves the original outcome in
    /// place, so the bookmark stays in the state it was in.
    #[tokio::test]
    async fn a_scraped_page_without_an_article_returns_the_original() {
        let scraper = FakeScraper::page("<html><body></body></html>");
        let usage = FakeUsage::allowing();
        let direct = FakeDirect {
            result: Err(ArticleFetchError::Permanent("HTTP 403".into())),
        };
        let result = fetcher(direct, scraper.clone(), usage.clone(), 0)
            .fetch(PUBLIC_URL)
            .await
            .unwrap_err();
        assert_eq!(result, ArticleFetchError::Permanent("HTTP 403".into()));
        assert_eq!(usage.failures(), vec![true], "the page was billed");
    }

    /// A scrape that finds less text than the direct fetch does not replace
    /// it: the stored text must never get worse.
    #[tokio::test]
    async fn a_shorter_scrape_leaves_a_short_direct_result_alone() {
        let scraper =
            FakeScraper::page("<html><body><article><p>tiny teaser</p></article></body></html>");
        let usage = FakeUsage::allowing();
        let short = article("A page with a little text");
        let direct = FakeDirect {
            result: Ok(short.clone()),
        };
        let result = fetcher(direct, scraper.clone(), usage.clone(), 0)
            .fetch(PUBLIC_URL)
            .await
            .unwrap();
        assert_eq!(result, short);
        assert_eq!(result.source, ContentSource::Fetched);
        assert_eq!(scraper.calls(), 1);
        assert_eq!(usage.failures(), vec![true], "the page was billed");
    }

    /// A scrape that finds more text replaces a short direct result.
    #[tokio::test]
    async fn a_longer_scrape_replaces_a_short_direct_result() {
        let scraper = FakeScraper::page(ARTICLE_HTML);
        let usage = FakeUsage::allowing();
        let direct = FakeDirect {
            result: Ok(article("A page with a little text")),
        };
        let result = fetcher(direct, scraper.clone(), usage.clone(), 0)
            .fetch(PUBLIC_URL)
            .await
            .unwrap();
        assert_eq!(result.source, ContentSource::Scraped);
        assert!(result.text.chars().count() > "A page with a little text".chars().count());
    }

    /// A 1,900-character Medium teaser clears the 300-character rule but is
    /// not the article: with no scraper it is metadata-only, never a fetched
    /// full text.
    #[tokio::test]
    async fn a_direct_preview_is_stored_unchanged_when_the_scraper_is_disabled() {
        let scraper = FakeScraper::disabled();
        let usage = FakeUsage::allowing();
        let teaser = preview_article(&"x".repeat(1_900));
        let direct = FakeDirect {
            result: Ok(teaser.clone()),
        };
        let result = fetcher(direct, scraper.clone(), usage.clone(), 0)
            .fetch(PUBLIC_URL)
            .await
            .unwrap();
        assert_eq!(result, teaser, "a disabled scraper changes nothing");
        assert_eq!(result.source, ContentSource::Fetched);
        assert!(result.preview);
        assert_eq!(scraper.calls(), 0);
        assert_eq!(usage.reserves(), 0);
        assert!(usage.failures().is_empty());
    }

    /// ... and a scraper that does reach the full article replaces the teaser.
    #[tokio::test]
    async fn a_direct_preview_is_replaced_by_a_full_scrape() {
        let scraper = FakeScraper::page(ARTICLE_HTML);
        let usage = FakeUsage::allowing();
        let direct = FakeDirect {
            result: Ok(preview_article(&"x".repeat(1_900))),
        };
        let result = fetcher(direct, scraper.clone(), usage.clone(), 0)
            .fetch(PUBLIC_URL)
            .await
            .unwrap();
        assert_eq!(scraper.calls(), 1);
        assert_eq!(result.source, ContentSource::Scraped);
        assert!(!result.preview);
        assert!(result.text.contains("borrow checker"));
    }

    /// A scrape that cannot finish before the job's budget runs out is not
    /// started at all: it would be billed and then abandoned, and the retry
    /// would pay again.
    #[tokio::test]
    async fn a_scrape_is_skipped_when_the_job_budget_is_short() {
        let scraper = FakeScraper::page(ARTICLE_HTML);
        let usage = FakeUsage::allowing();
        let direct = FakeDirect {
            result: Err(ArticleFetchError::Permanent("HTTP 403".into())),
        };
        let result = fetcher(direct, scraper.clone(), usage.clone(), 200)
            .with_job_budget(Duration::from_secs(1))
            .fetch(PUBLIC_URL)
            .await
            .unwrap_err();
        assert_eq!(result, ArticleFetchError::Permanent("HTTP 403".into()));
        assert_eq!(scraper.calls(), 0);
        assert_eq!(
            usage.reserves(),
            0,
            "no page is reserved for a scrape that cannot run"
        );
    }

    /// The decorator's own ceiling stops a vendor that ignores its client
    /// timeout. The job retries, and the page stays counted: a timed-out
    /// scrape is usually being worked on vendor-side.
    #[tokio::test]
    async fn a_slow_scrape_becomes_a_retryable_timeout_and_keeps_its_page() {
        let slow = ScrapedPage {
            html: ARTICLE_HTML.to_string(),
            final_url: PUBLIC_URL.to_string(),
            html_bytes: ARTICLE_HTML.len(),
            locked: false,
        };
        let scraper = FakeScraper::slow(slow);
        let usage = FakeUsage::allowing();
        let direct = FakeDirect {
            result: Err(ArticleFetchError::Permanent("HTTP 403".into())),
        };
        let result = FallbackFetcher::new(
            direct,
            scraper.clone(),
            1,
            usage.clone(),
            200,
            Duration::from_millis(50),
        )
        .fetch(PUBLIC_URL)
        .await
        .unwrap_err();
        match result {
            ArticleFetchError::Transient(message) => {
                assert!(message.contains("timed out"), "{message}")
            }
            other => panic!("expected a transient error, got {other:?}"),
        }
        assert_eq!(
            usage.failures(),
            vec![true],
            "the vendor may have billed the page a timed-out scrape was working on"
        );
    }

    /// A scraper that records how many calls are in flight at once, and holds
    /// each long enough for the others to arrive.
    struct PeakScraper {
        in_flight: AtomicUsize,
        peak: AtomicUsize,
        calls: AtomicUsize,
    }

    impl PeakScraper {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                in_flight: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
                calls: AtomicUsize::new(0),
            })
        }
    }

    impl PageScraper for PeakScraper {
        fn scrape(
            &self,
            _url: &str,
        ) -> crate::domain::ports::page_scraper::BoxFuture<'_, Result<ScrapedPage, ScrapeError>>
        {
            Box::pin(async move {
                let in_flight = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak.fetch_max(in_flight, Ordering::SeqCst);
                self.calls.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(300)).await;
                self.in_flight.fetch_sub(1, Ordering::SeqCst);
                Ok(ScrapedPage {
                    html: ARTICLE_HTML.to_string(),
                    final_url: PUBLIC_URL.to_string(),
                    html_bytes: ARTICLE_HTML.len(),
                    locked: false,
                })
            })
        }
    }

    /// Six permanent failures and a concurrency of 2: every job is scraped,
    /// and the vendor never sees more than 2 at once, which is what its plan
    /// answers 429 for. The rate is wide, so the cap under test is the slots.
    #[tokio::test(start_paused = true)]
    async fn scrapes_in_flight_are_capped_at_the_configured_concurrency() {
        let scraper = PeakScraper::new();
        let fetcher = Arc::new(
            fetcher(
                FakeDirect {
                    result: Err(ArticleFetchError::Permanent("HTTP 403".into())),
                },
                scraper.clone(),
                FakeUsage::allowing(),
                200,
            )
            .with_scrape_concurrency(2)
            .with_scrape_rate_per_minute(600),
        );
        let mut jobs = Vec::new();
        for _ in 0..6 {
            let fetcher = fetcher.clone();
            jobs.push(tokio::spawn(async move {
                fetcher.fetch(PUBLIC_URL).await.unwrap().source
            }));
        }
        for job in jobs {
            assert_eq!(job.await.unwrap(), ContentSource::Scraped);
        }
        assert_eq!(
            scraper.calls.load(Ordering::SeqCst),
            6,
            "waiting for a slot is not a failure"
        );
        assert_eq!(
            scraper.peak.load(Ordering::SeqCst),
            2,
            "the vendor saw at most 2 scrapes at a time"
        );
    }

    /// A job that cannot get a slot before its budget runs out keeps the
    /// direct result and reserves no page: a wait is not spend, and the
    /// retry schedule would only meet the same queue.
    #[tokio::test(start_paused = true)]
    async fn a_job_that_gets_no_scrape_slot_keeps_the_direct_result() {
        let slow = ScrapedPage {
            html: ARTICLE_HTML.to_string(),
            final_url: PUBLIC_URL.to_string(),
            html_bytes: ARTICLE_HTML.len(),
            locked: false,
        };
        let scraper = FakeScraper::slow(slow);
        let usage = FakeUsage::allowing();
        let fetcher = Arc::new(
            fetcher(
                FakeDirect {
                    result: Err(ArticleFetchError::Permanent("HTTP 403".into())),
                },
                scraper.clone(),
                usage.clone(),
                200,
            )
            // Room for one scrape (30 s) plus its extraction (30 s), and one
            // second of waiting on top of that.
            .with_job_budget(Duration::from_secs(61))
            .with_scrape_concurrency(1)
            // The slots are the cap under test, not the pace.
            .with_scrape_rate_per_minute(600),
        );

        let first = {
            let fetcher = fetcher.clone();
            tokio::spawn(async move { fetcher.fetch(PUBLIC_URL).await })
        };
        // The first job takes the only slot.
        tokio::time::sleep(Duration::from_millis(10)).await;

        let second = fetcher.fetch(PUBLIC_URL).await.unwrap_err();
        assert_eq!(second, ArticleFetchError::Permanent("HTTP 403".into()));
        assert_eq!(
            scraper.calls(),
            1,
            "the second job never reached the vendor"
        );
        assert_eq!(usage.reserves(), 1, "waiting reserves no page");

        assert_eq!(first.await.unwrap().unwrap().source, ContentSource::Scraped);
    }

    /// The page every scrape in these tests finds.
    fn scraped(html: &str) -> ScrapedPage {
        ScrapedPage {
            html: html.to_string(),
            final_url: PUBLIC_URL.to_string(),
            html_bytes: html.len(),
            locked: false,
        }
    }

    /// A scraper that answers each URL from its own script and records when
    /// every call went out on the (paused) clock.
    struct TimedScraper {
        /// One script per URL: its answers in order, the last one repeating.
        /// An empty script means "not under test" and answers with the page.
        scripts: Mutex<Vec<(String, VecDeque<Answer>)>>,
        calls: Mutex<Vec<(String, Instant)>>,
    }

    impl TimedScraper {
        /// `url` answers its script in order; the others answer with the page
        /// whenever they are asked.
        fn scripted(url: &'static str, answers: Vec<Answer>, others: &[&'static str]) -> Arc<Self> {
            let mut scripts = vec![(url.to_string(), VecDeque::from(answers))];
            scripts.extend(
                others
                    .iter()
                    .map(|other| (other.to_string(), VecDeque::new())),
            );
            Arc::new(Self {
                scripts: Mutex::new(scripts),
                calls: Mutex::new(Vec::new()),
            })
        }

        /// Every URL answers with the page, however often it is asked.
        fn pages(urls: &[&'static str]) -> Arc<Self> {
            Self::scripted(
                urls[0],
                vec![Answer::Page(scraped(ARTICLE_HTML))],
                &urls[1..],
            )
        }

        /// When the calls to one URL went out, in order.
        fn calls_to(&self, url: &str) -> Vec<Instant> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(called, _)| called == url)
                .map(|(_, at)| *at)
                .collect()
        }
    }

    impl PageScraper for TimedScraper {
        fn scrape(
            &self,
            url: &str,
        ) -> crate::domain::ports::page_scraper::BoxFuture<'_, Result<ScrapedPage, ScrapeError>>
        {
            self.calls
                .lock()
                .unwrap()
                .push((url.to_string(), Instant::now()));
            let answer = {
                let mut scripts = self.scripts.lock().unwrap();
                let (_, script) = scripts
                    .iter_mut()
                    .find(|(known, _)| known == url)
                    .expect("the fake scraper was asked about a known URL");
                if script.len() > 1 {
                    script.pop_front().expect("the script was just checked")
                } else {
                    script
                        .front()
                        .cloned()
                        .unwrap_or(Answer::Page(scraped(ARTICLE_HTML)))
                }
            };
            Box::pin(async move {
                match answer {
                    Answer::Page(page) => Ok(page),
                    Answer::Fail(error) => Err(error),
                    Answer::Slow(delay, page) => {
                        tokio::time::sleep(delay).await;
                        Ok(page)
                    }
                }
            })
        }
    }

    /// The plan's other limit: at ten a minute, twenty queued jobs make at
    /// most ten calls in any minute, and every one still gets its page when
    /// the job budget is wide enough to wait.
    #[tokio::test(start_paused = true)]
    async fn the_scrape_rate_caps_the_vendors_requests_per_minute() {
        const RATE: usize = 10;
        let scraper = TimedScraper::pages(&[PUBLIC_URL]);
        let fetcher = Arc::new(
            fetcher(
                FakeDirect {
                    result: Err(ArticleFetchError::Permanent("HTTP 403".into())),
                },
                scraper.clone(),
                FakeUsage::allowing(),
                0,
            )
            // Wide enough for all twenty turns at ten a minute: the pacing is
            // the cap under test, not the budget or the slots.
            .with_job_budget(Duration::from_secs(600))
            .with_scrape_concurrency(20)
            .with_scrape_rate_per_minute(RATE),
        );
        let mut jobs = Vec::new();
        for _ in 0..20 {
            let fetcher = fetcher.clone();
            jobs.push(tokio::spawn(async move {
                fetcher.fetch(PUBLIC_URL).await.unwrap().source
            }));
        }
        for job in jobs {
            assert_eq!(job.await.unwrap(), ContentSource::Scraped);
        }
        let calls = scraper.calls_to(PUBLIC_URL);
        assert_eq!(calls.len(), 20, "every job reached the vendor once");
        // Eleven calls span a minute or more: no minute carries more than the
        // configured ten.
        assert!(
            calls
                .windows(RATE + 1)
                .all(|window| window[RATE] - window[0] >= Duration::from_secs(60)),
            "{calls:?}"
        );
    }

    /// A 429 is the account's limit, not a page's: the pause holds every
    /// scrape, and the job that hit it retries once when the ask fits.
    #[tokio::test(start_paused = true)]
    async fn a_rate_limit_pauses_every_scraper_until_the_vendors_wait_ends() {
        const OTHER_URL: &str = "https://example.com/other-post";
        const ASK: Duration = Duration::from_secs(30);
        let scraper = TimedScraper::scripted(
            PUBLIC_URL,
            vec![
                Answer::Fail(ScrapeError::RateLimited {
                    retry_after: Some(ASK),
                }),
                Answer::Page(scraped(ARTICLE_HTML)),
            ],
            &[OTHER_URL],
        );
        let fetcher = Arc::new(fetcher(
            FakeDirect {
                result: Err(ArticleFetchError::Permanent("HTTP 403".into())),
            },
            scraper.clone(),
            FakeUsage::allowing(),
            0,
        ));

        let first = {
            let fetcher = fetcher.clone();
            tokio::spawn(async move { fetcher.fetch(PUBLIC_URL).await.unwrap().source })
        };
        // Let the first job take its 429 and start waiting.
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(5)).await;

        let second = {
            let fetcher = fetcher.clone();
            tokio::spawn(async move { fetcher.fetch(OTHER_URL).await.unwrap().source })
        };
        tokio::task::yield_now().await;
        assert_eq!(
            scraper.calls_to(OTHER_URL).len(),
            0,
            "the pause holds the second job too"
        );

        assert_eq!(first.await.unwrap(), ContentSource::Scraped);
        assert_eq!(second.await.unwrap(), ContentSource::Scraped);
        let asked = scraper.calls_to(PUBLIC_URL);
        assert_eq!(asked.len(), 2, "one 429 and one retry");
        assert!(asked[1] - asked[0] >= ASK, "{asked:?}");
        let other = scraper.calls_to(OTHER_URL);
        assert_eq!(other.len(), 1);
        assert!(
            other[0] - asked[0] >= ASK,
            "no scrape called the vendor inside the pause: {other:?} vs {asked:?}"
        );
    }

    /// A 429 whose ask outlasts the job's budget is not waited out inside the
    /// job, and a job that starts during the pause asks for nothing.
    #[tokio::test(start_paused = true)]
    async fn a_cooldown_that_outlasts_the_job_budget_returns_transient_without_a_request() {
        const OTHER_URL: &str = "https://example.com/other-post";
        let scraper = TimedScraper::scripted(
            PUBLIC_URL,
            vec![Answer::Fail(ScrapeError::RateLimited {
                // Past the 120 s cap, and far past one job's budget.
                retry_after: Some(Duration::from_secs(200)),
            })],
            &[OTHER_URL],
        );
        let usage = FakeUsage::allowing();
        let fetcher = Arc::new(fetcher(
            FakeDirect {
                result: Err(ArticleFetchError::Permanent("HTTP 403".into())),
            },
            scraper.clone(),
            usage.clone(),
            0,
        ));

        let first = {
            let fetcher = fetcher.clone();
            tokio::spawn(async move { fetcher.fetch(PUBLIC_URL).await })
        };
        tokio::task::yield_now().await;
        match first.await.unwrap().unwrap_err() {
            ArticleFetchError::Transient(message) => {
                assert!(message.contains("rate-limited"), "{message}")
            }
            other => panic!("expected a transient error, got {other:?}"),
        }
        assert_eq!(usage.reserves(), 1);
        assert_eq!(
            usage.failures(),
            vec![false],
            "the 429 is unbilled and refunded"
        );

        // Five seconds into a two-minute pause: this job has no budget for
        // it, so it goes back to the schedule without a request.
        tokio::time::advance(Duration::from_secs(5)).await;
        match fetcher.fetch(OTHER_URL).await.unwrap_err() {
            ArticleFetchError::Transient(message) => {
                assert!(message.contains("rate limit"), "{message}")
            }
            other => panic!("expected a transient error, got {other:?}"),
        }
        assert_eq!(
            scraper.calls_to(OTHER_URL).len(),
            0,
            "no request went out during the pause"
        );
        assert_eq!(
            usage.reserves(),
            1,
            "a scrape that never started reserves no page"
        );
        assert_eq!(
            usage.failures(),
            vec![false],
            "and nothing is refunded twice"
        );
    }
}
