use crate::domain::error::DomainError;
use std::future::Future;
use std::pin::Pin;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedImage {
    pub bytes: Vec<u8>,
    pub content_type: String,
}

/// Downloads and checks images at user-supplied or scraped URLs (card
/// images), with the same SSRF guard as every other outbound fetch.
pub trait ImageFetcher: Send + Sync {
    /// Download an image. Fails for anything that is not an `image/*`
    /// response or is larger than the adapter's cap.
    fn fetch(
        &self,
        url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<FetchedImage, DomainError>> + Send + '_>>;

    /// Whether the URL still serves something, without downloading it.
    fn is_reachable(&self, url: &str) -> Pin<Box<dyn Future<Output = bool> + Send + '_>>;
}
