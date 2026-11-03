use crate::domain::ports::page_scraper::{BoxFuture, PageScraper, ScrapeError, ScrapedPage};

/// The default backend: no vendor is called, and the fallback decorator
/// behaves exactly like the direct fetch.
pub struct DisabledScraper;

impl PageScraper for DisabledScraper {
    fn enabled(&self) -> bool {
        false
    }

    fn scrape(&self, _url: &str) -> BoxFuture<'_, Result<ScrapedPage, ScrapeError>> {
        Box::pin(std::future::ready(Err(ScrapeError::Final {
            message: "page scraper disabled".to_string(),
            billed: false,
        })))
    }
}
