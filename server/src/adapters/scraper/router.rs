use crate::adapters::scraper::{DisabledScraper, FirecrawlScraper};
use crate::config::{Config, PageScraperBackend};
use crate::domain::ports::page_scraper::PageScraper;
use std::sync::Arc;
use std::time::Duration;

/// The scraper the configuration selects.
///
/// `PAGE_SCRAPER=firecrawl` without a key is not a startup failure on
/// purpose: the key is not deployed everywhere yet, and an app that cannot
/// boot without it could not be deployed at all. The fallback then stays
/// disabled, with a warning naming the missing variable.
pub fn page_scraper(config: &Config) -> Arc<dyn PageScraper> {
    build(
        config.page_scraper,
        config.page_scraper_api_key.clone(),
        Duration::from_secs(config.page_scraper_timeout_seconds),
    )
}

fn build(
    backend: PageScraperBackend,
    api_key: Option<String>,
    timeout: Duration,
) -> Arc<dyn PageScraper> {
    let api_key = api_key
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty());
    match backend {
        PageScraperBackend::Disabled => Arc::new(DisabledScraper),
        PageScraperBackend::Firecrawl => match api_key {
            Some(api_key) => Arc::new(FirecrawlScraper::new(api_key, timeout)),
            None => {
                tracing::warn!(
                    "PAGE_SCRAPER=firecrawl but PAGE_SCRAPER_API_KEY is missing or empty: \
                     the page scraper stays disabled"
                );
                Arc::new(DisabledScraper)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_disabled_backend_calls_no_vendor() {
        let scraper = build(PageScraperBackend::Disabled, None, Duration::from_secs(30));
        assert!(!scraper.enabled());
    }

    #[test]
    fn firecrawl_needs_a_key_and_stays_disabled_without_one() {
        for key in [None, Some(String::new()), Some("   ".to_string())] {
            let label = key.clone();
            let scraper = build(PageScraperBackend::Firecrawl, key, Duration::from_secs(30));
            assert!(!scraper.enabled(), "{label:?}");
        }
        let scraper = build(
            PageScraperBackend::Firecrawl,
            Some("fc-test".to_string()),
            Duration::from_secs(30),
        );
        assert!(scraper.enabled());
    }
}
