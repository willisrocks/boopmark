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

use crate::adapters::article::readability::HtmlExtractor;
use crate::adapters::net::validate_public_url;
use crate::domain::chunking::MIN_CHUNKED_TEXT_CHARS;
use crate::domain::ports::article_fetcher::{ArticleFetchError, ArticleFetcher, FetchedArticle};
use crate::domain::ports::page_scraper::{PREVIEW_LOCKED_MSG, PageScraper, ScrapeError};
use crate::domain::ports::scraper_usage_repo::ScraperUsageRepository;
use crate::domain::search::ContentSource;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
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

pub struct FallbackFetcher<F, S> {
    direct: F,
    scraper: S,
    extractor: HtmlExtractor,
    usage: Arc<dyn ScraperUsageRepository>,
    /// `PAGE_SCRAPER_MONTHLY_BUDGET_PAGES`; 0 is no cap.
    monthly_budget_pages: u64,
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
        if self.job_budget.saturating_sub(started.elapsed())
            < self.scrape_timeout.saturating_add(EXTRACT_RESERVE)
        {
            tracing::debug!(%url, "page scraper skipped: no job budget left for a scrape");
            return original.clone();
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
                return original.clone();
            }
            Err(error) => {
                // Spending without accounting is worse than not spending.
                tracing::warn!(
                    error = %error,
                    "page scraper skipped: the monthly budget could not be checked"
                );
                return original.clone();
            }
        }

        // The adapter bounds itself too; this is the job's hard stop.
        let page = match tokio::time::timeout(self.scrape_timeout, self.scraper.scrape(url)).await {
            Err(_elapsed) => {
                // No answer was seen, but a timeout usually means the vendor
                // kept working: assume the page was billed rather than give
                // the month's budget away.
                self.note_failure(true).await;
                tracing::debug!(%url, "page scrape timed out");
                return Err(ArticleFetchError::Transient(
                    "page scraper timed out".to_string(),
                ));
            }
            Ok(Ok(page)) => page,
            Ok(Err(error)) => {
                self.note_failure(error.billed()).await;
                tracing::debug!(
                    %url,
                    error = %error,
                    billed = error.billed(),
                    "page scrape failed"
                );
                return match error {
                    ScrapeError::OutOfCredits => {
                        self.note_out_of_credits();
                        original.clone()
                    }
                    // The vendor said "not now": the ingest retry schedule is
                    // the right place to wait.
                    ScrapeError::Retryable { message, .. } => {
                        Err(ArticleFetchError::Transient(message))
                    }
                    // The same request will fail the same way; the direct
                    // result is the honest outcome.
                    ScrapeError::Final { .. } => original.clone(),
                };
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
                tracing::debug!(%url, error = %error, "the scraped page has no article");
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
            .with_out_of_credits_cooldown(Duration::from_millis(50));

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
}
