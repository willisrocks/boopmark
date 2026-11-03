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
//! without the scraper. Two answers are recorded instead, and neither is
//! retried: a member-only preview (`preview only (locked)`, from either path)
//! and a retryable scrape failure, which becomes `Transient` so the ingest
//! schedule retries it.
//!
//! Spend is counted the way the vendor bills: a page is reserved before the
//! call and is refunded only when no 2xx came back. A 2xx that produced
//! nothing usable still cost a page and is counted as a failure.

use crate::adapters::article::readability::HtmlExtractor;
use crate::adapters::net::validate_public_url;
use crate::domain::chunking::MIN_CHUNKED_TEXT_CHARS;
use crate::domain::ports::article_fetcher::{ArticleFetchError, ArticleFetcher, FetchedArticle};
use crate::domain::ports::page_scraper::{PREVIEW_LOCKED_MSG, PageScraper, ScrapeError};
use crate::domain::ports::scraper_usage_repo::ScraperUsageRepository;
use crate::domain::search::ContentSource;
use chrono::Datelike;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;
use tokio::time::Instant;

/// Mirrors `IngestWorker::JOB_TIMEOUT`: the direct fetch, any scrape and the
/// extraction all have to finish inside one job's budget, so a scrape starts
/// only when this much of it is still left.
const JOB_BUDGET: Duration = Duration::from_secs(120);

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
    /// The month (`YYYYMM`) whose credits ran out, 0 while they have not: the
    /// fallback then calls no vendor at all until the month rolls.
    out_of_credits_month: AtomicU32,
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
            extractor: HtmlExtractor::new(extraction_slots),
            usage,
            monthly_budget_pages,
            scrape_timeout,
            job_budget: JOB_BUDGET,
            budget_notice: AtomicBool::new(false),
            out_of_credits_month: AtomicU32::new(0),
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

    /// The current month, as `YYYYMM`.
    fn month_key() -> u32 {
        let now = chrono::Utc::now();
        now.year() as u32 * 100 + now.month()
    }

    /// True while this month's credits are known to be gone.
    fn out_of_credits(&self) -> bool {
        let month = self.out_of_credits_month.load(Ordering::Relaxed);
        month != 0 && month == Self::month_key()
    }

    /// Switch the fallback off for the rest of the month, warning once.
    fn note_out_of_credits(&self) {
        let month = Self::month_key();
        if self.out_of_credits_month.swap(month, Ordering::Relaxed) != month {
            tracing::warn!(
                "page scraper switched off: the account is out of credits until next month"
            );
        }
    }

    /// One scrape, or the original result when it cannot help.
    async fn with_scrape(
        &self,
        url: &str,
        original: Result<FetchedArticle, ArticleFetchError>,
        started: Instant,
    ) -> Result<FetchedArticle, ArticleFetchError> {
        if !self.scraper.enabled() {
            return original;
        }
        // Past 402, every later call would hit the same wall: stop asking.
        if self.out_of_credits() {
            tracing::debug!(%url, "page scraper skipped: the account is out of credits");
            return original;
        }
        // A URL the app may not fetch, the vendor must not be asked to fetch
        // either: the vendor would otherwise be an SSRF proxy.
        if let Err(refused) = validate_public_url(url) {
            tracing::debug!(%url, reason = %refused, "page scraper skipped: URL is not public");
            return original;
        }
        // A scrape that cannot finish inside the job's budget would be billed
        // and then abandoned, and the retry would pay again.
        if self.job_budget.saturating_sub(started.elapsed()) < self.scrape_timeout {
            tracing::debug!(%url, "page scraper skipped: no job budget left for a scrape");
            return original;
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
                return original;
            }
            Err(error) => {
                // Spending without accounting is worse than not spending.
                tracing::warn!(
                    error = %error,
                    "page scraper skipped: the monthly budget could not be checked"
                );
                return original;
            }
        }

        // The adapter bounds itself too; this is the job's hard stop.
        let page = match tokio::time::timeout(self.scrape_timeout, self.scraper.scrape(url)).await {
            Err(_elapsed) => {
                // No answer came back, so nothing was billed.
                self.note_failure(false).await;
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
                        original
                    }
                    // The vendor said "not now": the ingest retry schedule is
                    // the right place to wait.
                    ScrapeError::Retryable { message, .. } => {
                        Err(ArticleFetchError::Transient(message))
                    }
                    // The same request will fail the same way; the direct
                    // result is the honest outcome.
                    ScrapeError::Final { .. } => original,
                };
            }
        };

        // From here the page was paid for, whatever comes of it.
        if page.locked {
            self.note_failure(true).await;
            tracing::debug!(%url, "page scraper got a preview only");
            return Err(ArticleFetchError::Permanent(PREVIEW_LOCKED_MSG.to_string()));
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
                return original;
            }
        };
        if extracted.preview {
            self.note_failure(true).await;
            tracing::debug!(%url, "page scraper got a member-only preview");
            return Err(ArticleFetchError::Permanent(PREVIEW_LOCKED_MSG.to_string()));
        }
        if !scrape_improves(&original, &extracted.text) {
            self.note_failure(true).await;
            tracing::debug!(%url, "the scraped text does not beat the direct result");
            return original;
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
            let outcome = match self.direct.fetch(&url).await {
                // The retry schedule (Transient) or the non-HTML verdict
                // (Skipped) already decides these; a scrape would not.
                Err(ArticleFetchError::Transient(message)) => {
                    Err(ArticleFetchError::Transient(message))
                }
                Err(ArticleFetchError::Skipped(message)) => {
                    Err(ArticleFetchError::Skipped(message))
                }
                Err(permanent) => self.with_scrape(&url, Err(permanent), started).await,
                // Checked before the length rule: a 1,900-character teaser is
                // not an article, and only the scrape (or a capture) can say
                // whether the full one is reachable.
                Ok(article) if article.preview => {
                    self.with_scrape(&url, Ok(article), started).await
                }
                Ok(article) if is_article(&article.text) => Ok(article),
                Ok(short) => self.with_scrape(&url, Ok(short), started).await,
            };
            // Whatever produced it, a preview is never stored as a full fetch:
            // it stays metadata-only until a capture replaces it.
            match outcome {
                Ok(article) if article.preview => {
                    Err(ArticleFetchError::Permanent(PREVIEW_LOCKED_MSG.to_string()))
                }
                other => other,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error::DomainError;
    use crate::domain::ports::page_scraper::ScrapedPage;
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

    /// One prepared scrape answer, handed out once.
    enum Answer {
        Page(ScrapedPage),
        Fail(ScrapeError),
        Slow(Duration, ScrapedPage),
    }

    struct FakeScraper {
        enabled: bool,
        calls: AtomicUsize,
        answer: Mutex<Option<Answer>>,
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

        fn slow(page: ScrapedPage) -> Arc<Self> {
            Self::answering(Answer::Slow(Duration::from_secs(5), page))
        }

        fn answering(answer: Answer) -> Arc<Self> {
            Arc::new(Self {
                enabled: true,
                calls: AtomicUsize::new(0),
                answer: Mutex::new(Some(answer)),
            })
        }

        fn disabled() -> Arc<Self> {
            Arc::new(Self {
                enabled: false,
                calls: AtomicUsize::new(0),
                answer: Mutex::new(None),
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
            let answer = self
                .answer
                .lock()
                .unwrap()
                .take()
                .expect("the fake scraper answers once");
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

    /// Out of credits is not per-page: the next bookmark would burn another
    /// request for the same answer, so the fallback stops asking until the
    /// month rolls.
    #[tokio::test]
    async fn out_of_credits_switches_the_fallback_off_for_the_month() {
        let scraper = FakeScraper::failing(ScrapeError::OutOfCredits);
        let usage = FakeUsage::allowing();
        let direct = FakeDirect {
            result: Err(ArticleFetchError::Permanent("HTTP 403".into())),
        };
        let fetcher = fetcher(direct, scraper.clone(), usage.clone(), 200);
        for _ in 0..3 {
            let result = fetcher.fetch(PUBLIC_URL).await.unwrap_err();
            assert_eq!(result, ArticleFetchError::Permanent("HTTP 403".into()));
        }
        assert_eq!(scraper.calls(), 1, "the breaker stops later calls");
        assert_eq!(usage.reserves(), 1, "and reserves no more pages");
        assert_eq!(usage.failures(), vec![false]);
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
    async fn a_direct_preview_is_never_stored_as_full_text() {
        let scraper = FakeScraper::disabled();
        let usage = FakeUsage::allowing();
        let direct = FakeDirect {
            result: Ok(preview_article(&"x".repeat(1_900))),
        };
        let result = fetcher(direct, scraper, usage.clone(), 0)
            .fetch(PUBLIC_URL)
            .await
            .unwrap_err();
        assert_eq!(
            result,
            ArticleFetchError::Permanent(PREVIEW_LOCKED_MSG.to_string())
        );
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
    /// timeout. Nothing came back, so nothing is billed.
    #[tokio::test]
    async fn a_slow_scrape_becomes_a_retryable_timeout_and_is_refunded() {
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
        assert_eq!(usage.failures(), vec![false]);
    }
}
