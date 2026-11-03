//! The scraper fallback of the article fetcher.
//!
//! `IngestWorker` asks one `ArticleFetcher` for a page. This decorator answers
//! from the guarded direct fetch, and only when that path cannot read the page
//! does it ask the `PageScraper` port (see
//! docs/superpowers/specs/2026-11-03-page-capture-design.md):
//!
//! - direct text of at least `MIN_CHUNKED_TEXT_CHARS` → the direct result;
//! - direct `Permanent` failure, or a shorter text → one scrape;
//! - direct `Transient` → the retry schedule already covers it, no scrape;
//! - direct `Skipped` → not an HTML page, no scrape.
//!
//! A scrape never replaces a better result: any failure, an exhausted budget,
//! or text no longer than what the direct fetch got leaves the original
//! result in place, so the bookmark lands in `failed`/`skipped` exactly as it
//! would without the scraper. The one exception is a member-only preview,
//! which is recorded as `preview only (locked)` rather than retried.

use crate::adapters::article::readability::HtmlExtractor;
use crate::adapters::net::validate_public_url;
use crate::domain::chunking::MIN_CHUNKED_TEXT_CHARS;
use crate::domain::ports::article_fetcher::{ArticleFetchError, ArticleFetcher, FetchedArticle};
use crate::domain::ports::page_scraper::{PREVIEW_LOCKED_MSG, PageScraper, ScrapedPage};
use crate::domain::ports::scraper_usage_repo::ScraperUsageRepository;
use crate::domain::search::ContentSource;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct FallbackFetcher<F, S> {
    direct: F,
    scraper: S,
    extractor: HtmlExtractor,
    usage: Arc<dyn ScraperUsageRepository>,
    /// `PAGE_SCRAPER_MONTHLY_BUDGET_PAGES`; 0 is no cap.
    monthly_budget_pages: u64,
    /// One warning per process when the month's budget is gone, not one per
    /// bookmark.
    budget_notice: AtomicBool,
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
    ) -> Self {
        Self {
            direct,
            scraper,
            extractor: HtmlExtractor::new(extraction_slots),
            usage,
            monthly_budget_pages,
            budget_notice: AtomicBool::new(false),
        }
    }
}

impl<F, S> FallbackFetcher<F, S>
where
    F: ArticleFetcher,
    S: PageScraper,
{
    /// Note a page that spent budget without producing text.
    async fn note_failure(&self) {
        if let Err(error) = self.usage.record_failure().await {
            tracing::warn!(error = %error, "recording a scraper failure failed");
        }
    }

    /// One scrape, or the original result when it cannot help.
    async fn with_scrape(
        &self,
        url: &str,
        original: Result<FetchedArticle, ArticleFetchError>,
    ) -> Result<FetchedArticle, ArticleFetchError> {
        if !self.scraper.enabled() {
            return original;
        }
        // A URL the app may not fetch, the vendor must not be asked to fetch
        // either: the vendor would otherwise be an SSRF proxy.
        if let Err(refused) = validate_public_url(url) {
            tracing::debug!(%url, reason = %refused, "page scraper skipped: URL is not public");
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

        let page = match self.scraper.scrape(url).await {
            Ok(page) => page,
            Err(error) => {
                self.note_failure().await;
                tracing::debug!(%url, error = %error, "page scrape failed");
                return original;
            }
        };
        // A preview is a definite answer, not a failure to try again later:
        // only the browser capture gets a member-only story's full text.
        if page.locked {
            self.note_failure().await;
            tracing::debug!(%url, "page scraper got a preview only");
            return Err(ArticleFetchError::Permanent(PREVIEW_LOCKED_MSG.to_string()));
        }
        let ScrapedPage {
            html,
            final_url,
            html_bytes,
            locked: _,
        } = page;
        // The app's own Readability pass on the vendor's HTML, so a scrape
        // and a capture produce the same text for the same page.
        let text = match self
            .extractor
            .extract_fetched(html.into_bytes(), Some("text/html"), &final_url)
            .await
        {
            Ok(text) => text,
            Err(error) => {
                self.note_failure().await;
                tracing::debug!(%url, error = %error, "the scraped page has no article");
                return original;
            }
        };
        if !scrape_improves(&original, &text) {
            self.note_failure().await;
            tracing::debug!(%url, "the scraped text does not beat the direct result");
            return original;
        }
        tracing::debug!(%url, chars = text.chars().count(), "article stored from the page scraper");
        Ok(FetchedArticle {
            text,
            final_url,
            html_bytes,
            source: ContentSource::Scraped,
        })
    }
}

/// A page counts as an article past the chunking threshold, which is also
/// where a capture stops being worth a scrape.
fn is_article(text: &str) -> bool {
    text.chars().count() >= MIN_CHUNKED_TEXT_CHARS
}

/// True when the scraped text is worth storing over the direct result: any
/// text at all after a failure, and strictly more text than a short success.
fn scrape_improves(original: &Result<FetchedArticle, ArticleFetchError>, text: &str) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    match original {
        Err(_) => true,
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
            match self.direct.fetch(&url).await {
                // The retry schedule (Transient) or the non-HTML verdict
                // (Skipped) already decides these; a scrape would not.
                Err(ArticleFetchError::Transient(message)) => {
                    Err(ArticleFetchError::Transient(message))
                }
                Err(ArticleFetchError::Skipped(message)) => {
                    Err(ArticleFetchError::Skipped(message))
                }
                Err(permanent) => self.with_scrape(&url, Err(permanent)).await,
                Ok(article) if is_article(&article.text) => Ok(article),
                Ok(short) => self.with_scrape(&url, Ok(short)).await,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error::DomainError;
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

    const PUBLIC_URL: &str = "https://medium.com/@writer/a-post";

    fn article(text: &str) -> FetchedArticle {
        FetchedArticle {
            text: text.to_string(),
            final_url: PUBLIC_URL.to_string(),
            html_bytes: 1234,
            source: ContentSource::Fetched,
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

    struct FakeScraper {
        enabled: bool,
        calls: AtomicUsize,
        answer: Mutex<Option<Result<ScrapedPage, DomainError>>>,
    }

    impl FakeScraper {
        fn page(html: &str) -> Arc<Self> {
            Self::answering(Ok(ScrapedPage {
                html: html.to_string(),
                final_url: PUBLIC_URL.to_string(),
                html_bytes: html.len(),
                locked: false,
            }))
        }

        fn failing(error: DomainError) -> Arc<Self> {
            Self::answering(Err(error))
        }

        fn answering(answer: Result<ScrapedPage, DomainError>) -> Arc<Self> {
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
        ) -> crate::domain::ports::page_scraper::BoxFuture<'_, Result<ScrapedPage, DomainError>>
        {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let answer = self
                .answer
                .lock()
                .unwrap()
                .take()
                .expect("the fake scraper answers once");
            Box::pin(std::future::ready(answer))
        }
    }

    /// The budget and the failure counter the decorator writes through.
    struct FakeUsage {
        can_reserve: bool,
        broken: bool,
        reserves: AtomicUsize,
        failures: AtomicUsize,
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
                failures: AtomicUsize::new(0),
                budgets: Mutex::new(Vec::new()),
            })
        }

        fn reserves(&self) -> usize {
            self.reserves.load(Ordering::Relaxed)
        }

        fn failures(&self) -> usize {
            self.failures.load(Ordering::Relaxed)
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
        ) -> crate::domain::ports::scraper_usage_repo::BoxFuture<'_, Result<(), DomainError>>
        {
            self.failures.fetch_add(1, Ordering::Relaxed);
            Box::pin(std::future::ready(Ok(())))
        }
    }

    fn fetcher<S>(
        direct: FakeDirect,
        scraper: S,
        usage: Arc<FakeUsage>,
        budget: u64,
    ) -> FallbackFetcher<FakeDirect, S> {
        FallbackFetcher::new(direct, scraper, 1, usage, budget)
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
        assert_eq!(usage.failures(), 0, "a stored article is not a failure");
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
    async fn a_failed_scrape_returns_the_original_result() {
        let scraper = FakeScraper::failing(DomainError::Internal(
            "Firecrawl API request timed out".into(),
        ));
        let usage = FakeUsage::allowing();
        let direct = FakeDirect {
            result: Err(ArticleFetchError::Permanent(
                "blocked by Cloudflare challenge".into(),
            )),
        };
        let result = fetcher(direct, scraper.clone(), usage.clone(), 0)
            .fetch(PUBLIC_URL)
            .await
            .unwrap_err();
        assert_eq!(
            result,
            ArticleFetchError::Permanent("blocked by Cloudflare challenge".into())
        );
        assert_eq!(scraper.calls(), 1);
        assert_eq!(usage.failures(), 1, "the page was refunded");
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
        assert_eq!(usage.failures(), 0);
    }

    /// A member-only preview is a definite answer: the error says so and the
    /// bookmark is not retried, because only a browser capture can do better.
    #[tokio::test]
    async fn a_locked_preview_is_recorded_and_not_retried() {
        let scraper = FakeScraper::answering(Ok(ScrapedPage {
            html: r#"<html><p>teaser</p><script>{"isLocked":true}</script></html>"#.to_string(),
            final_url: PUBLIC_URL.to_string(),
            html_bytes: 40,
            locked: true,
        }));
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
        assert_eq!(usage.failures(), 1);
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
        assert_eq!(usage.failures(), 1);
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
        assert_eq!(usage.failures(), 1, "the page was refunded");
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
}
