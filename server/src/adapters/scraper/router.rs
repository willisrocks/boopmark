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
    let scraper = build(
        config.page_scraper,
        config.page_scraper_api_key.clone(),
        Duration::from_secs(config.page_scraper_timeout_seconds),
    );
    // One line at startup, so production can tell a disabled fallback from a
    // budget of zero or a pace the vendor will rate-limit.
    let backend = if scraper.enabled() {
        "firecrawl"
    } else {
        "disabled"
    };
    let budget = config.page_scraper_monthly_budget_pages;
    let concurrency = config.page_scraper_concurrency;
    let rate = config.page_scraper_rate_per_minute;
    tracing::info!(
        "scraper enabled: {backend}, budget {budget}, concurrency {concurrency}, rate {rate}/min"
    );
    scraper
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
    use crate::config::{
        DEFAULT_PAGE_SCRAPER_TIMEOUT_SECONDS, LoginAdapter, MetadataFallbackBackend,
        ScreenshotBackend, StorageBackend,
    };
    use std::io::Write;
    use std::sync::Mutex;

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

    /// The scrape settings, with everything else at a placeholder: the router
    /// reads only these.
    fn config(
        backend: PageScraperBackend,
        api_key: Option<&str>,
        budget: u64,
        concurrency: usize,
        rate: usize,
    ) -> Config {
        Config {
            database_url: String::new(),
            app_url: "http://127.0.0.1".into(),
            port: 0,
            llm_settings_encryption_key: String::new(),
            session_secret: "router-test".into(),
            google_client_id: None,
            google_client_secret: None,
            enable_e2e_auth: false,
            login_adapter: LoginAdapter::LocalPassword,
            storage_backend: StorageBackend::Local,
            s3_endpoint: None,
            s3_access_key: None,
            s3_secret_key: None,
            s3_region: "auto".into(),
            s3_images_bucket: "boopmark-images".into(),
            s3_images_public_url: None,
            screenshot_backend: ScreenshotBackend::Disabled,
            screenshot_service_url: None,
            metadata_fallback_backend: MetadataFallbackBackend::None,
            iframely_api_key: None,
            opengraph_io_api_key: None,
            article_ingest_enabled: true,
            article_ingest_concurrency: 4,
            page_capture_enabled: true,
            page_capture_max_bytes: 5 * 1024 * 1024,
            page_scraper: backend,
            page_scraper_api_key: api_key.map(str::to_string),
            page_scraper_timeout_seconds: DEFAULT_PAGE_SCRAPER_TIMEOUT_SECONDS,
            page_scraper_monthly_budget_pages: budget,
            page_scraper_concurrency: concurrency,
            page_scraper_rate_per_minute: rate,
        }
    }

    /// The lines `emit` logs, as they reach a log sink: the startup line is
    /// asserted as emitted, not as a format string the test restates.
    fn captured(emit: impl FnOnce()) -> String {
        #[derive(Clone)]
        struct Buffer(Arc<Mutex<Vec<u8>>>);

        impl Write for Buffer {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let buffer = Arc::new(Mutex::new(Vec::new()));
        let writer = Buffer(buffer.clone());
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .finish();
        tracing::subscriber::with_default(subscriber, emit);
        String::from_utf8(buffer.lock().unwrap().clone()).unwrap()
    }

    #[test]
    fn the_startup_line_names_the_backend_budget_concurrency_and_rate() {
        let output = captured(|| {
            page_scraper(&config(
                PageScraperBackend::Firecrawl,
                Some("fc-test"),
                200,
                3,
                10,
            ));
        });
        assert!(
            output.contains("scraper enabled: firecrawl, budget 200, concurrency 3, rate 10/min"),
            "{output}"
        );
    }

    /// A disabled fallback still says so once, with the settings it would have
    /// used: "no scrapes happened" is otherwise indistinguishable from a
    /// queue that never reached the vendor.
    #[test]
    fn the_startup_line_says_disabled_without_a_key() {
        let output = captured(|| {
            page_scraper(&config(PageScraperBackend::Firecrawl, None, 0, 2, 60));
        });
        assert!(
            output.contains("scraper enabled: disabled, budget 0, concurrency 2, rate 60/min"),
            "{output}"
        );
    }
}
