//! Adapters for the `PageScraper` port, plus the router that builds the one
//! selected by configuration:
//!
//! ```text
//! PAGE_SCRAPER=disabled | firecrawl                (default: disabled)
//! PAGE_SCRAPER_API_KEY=fc-...                      (firecrawl only; operator key)
//! PAGE_SCRAPER_TIMEOUT_SECONDS=30                  (one scrape, retries excluded)
//! PAGE_SCRAPER_MONTHLY_BUDGET_PAGES=0              (0 = no cap)
//! PAGE_SCRAPER_CONCURRENCY=2                       (1..=10 in flight)
//! ```
//!
//! The companion eval chose Firecrawl and dropped Obscura
//! (`docs/superpowers/specs/assets/2026-11-03-scraper-eval.md`), so there is
//! no Obscura adapter and no `PAGE_SCRAPER_URL`.

mod disabled;
mod firecrawl;
mod router;

pub use disabled::DisabledScraper;
pub use firecrawl::FirecrawlScraper;
pub use router::page_scraper;
