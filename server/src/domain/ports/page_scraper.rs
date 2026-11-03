//! Server-side page scraping for URL-only saves: a remote renderer fetches a
//! page the guarded direct fetch cannot read (Cloudflare, bot checks), and the
//! app runs its own Readability pass on the returned HTML. Vendor adapters
//! live in `adapters::scraper`; the backend is picked by `PAGE_SCRAPER`, so
//! `disabled` is the default and a hosted vendor is one env value away. See
//! docs/superpowers/specs/2026-11-03-page-capture-design.md.
//!
//! Like `Embedder`, a request carries no API key: the scraper is the
//! operator's account, so the key lives in the adapter's configuration.

use std::fmt;

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
    /// The vendor says the page is a paywalled or member-only preview (its
    /// metadata flag). The HTML itself is checked again at extraction, so a
    /// vendor that does not report the flag still yields a preview verdict.
    pub locked: bool,
}

/// The final message recorded for a locked preview. The bookmark stays
/// searchable by its metadata, and no retry can change the answer: only the
/// browser capture stores a member-only story's full text.
pub const PREVIEW_LOCKED_MSG: &str = "preview only (locked)";

/// Why a scrape produced no page.
///
/// The vendor is paid per page it renders, so every variant says whether a
/// 2xx came back: the monthly cap must count what was paid for, even when the
/// page turned out unusable. Only a request that never got a 2xx is refunded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScrapeError {
    /// A retry may succeed: a timeout, a connection error, 408/409, 429, 5xx,
    /// or a 401/403 on the operator's key, which is rotated out of band.
    Retryable { message: String, billed: bool },
    /// The same request will not succeed later: a rejected payload, a page
    /// that answered an error status, an answer without HTML.
    Final { message: String, billed: bool },
    /// The account is out of credits (HTTP 402). No scrape can work until the
    /// credits reset, so the fallback switches itself off for the rest of the
    /// month instead of burning a request per bookmark.
    OutOfCredits,
}

impl ScrapeError {
    /// Whether the vendor charged for the page: a 2xx came back.
    pub fn billed(&self) -> bool {
        match self {
            Self::Retryable { billed, .. } | Self::Final { billed, .. } => *billed,
            Self::OutOfCredits => false,
        }
    }

    /// The message, carrying the HTTP status and never a key or a body.
    pub fn message(&self) -> &str {
        match self {
            Self::Retryable { message, .. } | Self::Final { message, .. } => message,
            Self::OutOfCredits => "the scraper account is out of credits",
        }
    }
}

impl fmt::Display for ScrapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for ScrapeError {}

/// A shared scraper is itself a scraper, so the decorator can take the
/// router's `Arc<dyn PageScraper>` without caring which backend it holds.
impl<T: PageScraper + ?Sized> PageScraper for std::sync::Arc<T> {
    fn enabled(&self) -> bool {
        (**self).enabled()
    }

    fn scrape(&self, url: &str) -> BoxFuture<'_, Result<ScrapedPage, ScrapeError>> {
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

    /// Fetch one page's HTML. The error says whether the vendor billed for
    /// it; see `ScrapeError`. Messages carry the HTTP status, never a key or
    /// a response body.
    fn scrape(&self, url: &str) -> BoxFuture<'_, Result<ScrapedPage, ScrapeError>>;
}
