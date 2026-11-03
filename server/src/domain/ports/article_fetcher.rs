use crate::domain::search::ContentSource;
use std::future::Future;
use std::pin::Pin;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedArticle {
    /// Readable text of the article, already capped.
    pub text: String,
    /// The URL after redirects.
    pub final_url: String,
    pub html_bytes: usize,
    /// Which path produced the text: the direct fetch, or the scraper
    /// fallback. A browser capture never comes through this port.
    pub source: ContentSource,
    /// The publisher served a paywalled preview of the article, not the
    /// article. The text is what a signed-out reader sees, so it is never
    /// stored as a full fetch.
    pub preview: bool,
}

/// Why no article came back. The ingest worker retries `Transient` errors
/// on a schedule; `Permanent` and `Skipped` are final and leave the
/// bookmark searchable by its metadata only.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ArticleFetchError {
    /// Timeouts, connection errors, 429 and 5xx responses.
    #[error("{0}")]
    Transient(String),
    /// The URL is refused or the page will not change on a retry: blocked
    /// addresses, 4xx responses, challenge pages, unreadable documents.
    #[error("{0}")]
    Permanent(String),
    /// The response is not an HTML article (another content type, or too big).
    #[error("{0}")]
    Skipped(String),
}

pub trait ArticleFetcher: Send + Sync {
    fn fetch(
        &self,
        url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<FetchedArticle, ArticleFetchError>> + Send + '_>>;
}
