use crate::domain::error::DomainError;
use crate::domain::ports::page_scraper::{BoxFuture, PageScraper, ScrapedPage};

/// The default backend: no vendor is called, and the fallback decorator
/// behaves exactly like the direct fetch.
pub struct DisabledScraper;

impl PageScraper for DisabledScraper {
    fn enabled(&self) -> bool {
        false
    }

    fn scrape(&self, _url: &str) -> BoxFuture<'_, Result<ScrapedPage, DomainError>> {
        Box::pin(std::future::ready(Err(DomainError::InvalidInput(
            "page scraper disabled".to_string(),
        ))))
    }
}
