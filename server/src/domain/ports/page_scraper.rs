//! Server-side page scraping for URL-only saves: a remote renderer fetches a
//! page the guarded direct fetch cannot read (Cloudflare, bot checks), and the
//! app runs its own Readability pass on the returned HTML. Vendor adapters
//! live in `adapters::scraper`; the backend is picked by `PAGE_SCRAPER`, so
//! `disabled` is the default and a hosted vendor is one env value away. See
//! docs/superpowers/specs/2026-11-03-page-capture-design.md.
//!
//! Like `Embedder`, a request carries no API key: the scraper is the
//! operator's account, so the key lives in the adapter's configuration.

use crate::domain::error::DomainError;

pub use crate::domain::ports::llm_client::BoxFuture;

/// The page a scraper returned, before extraction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrapedPage {
    /// The vendor's HTML for the page.
    pub html: String,
    /// The URL after redirects, as the vendor reports it.
    pub final_url: String,
    /// Size of `html`, recorded like a direct fetch's body size.
    pub html_bytes: usize,
    /// The vendor's page is a paywalled or member-only preview: the HTML
    /// holds the teaser, not the article. The caller decides what to do with
    /// it; this port only reports it.
    pub locked: bool,
}

/// The final message recorded for a locked preview. The bookmark stays
/// searchable by its metadata, and no retry can change the answer: only the
/// browser capture stores a member-only story's full text.
pub const PREVIEW_LOCKED_MSG: &str = "preview only (locked)";

/// A shared scraper is itself a scraper, so the decorator can take the
/// router's `Arc<dyn PageScraper>` without caring which backend it holds.
impl<T: PageScraper + ?Sized> PageScraper for std::sync::Arc<T> {
    fn enabled(&self) -> bool {
        (**self).enabled()
    }

    fn scrape(&self, url: &str) -> BoxFuture<'_, Result<ScrapedPage, DomainError>> {
        let url = url.to_string();
        Box::pin(async move { (**self).scrape(&url).await })
    }
}

pub trait PageScraper: Send + Sync {
    /// False for the disabled backend: the fallback decorator then behaves
    /// exactly like the direct fetch. Defaulted so real adapters need not
    /// implement it.
    fn enabled(&self) -> bool {
        true
    }

    /// Fetch one page's HTML. Errors:
    ///
    /// - `InvalidInput`: the request will never be accepted (disabled, a
    ///   malformed URL or vendor payload, HTTP 400). Do not retry.
    /// - `Internal`: transient, vendor-side or operator-fixable (network,
    ///   timeout, 401/403 for the operator's key, 408, 409, 429, 5xx, a
    ///   malformed response). The ingest worker retries with backoff.
    ///
    /// Messages carry the HTTP status, never a key or a response body.
    fn scrape(&self, url: &str) -> BoxFuture<'_, Result<ScrapedPage, DomainError>>;
}
