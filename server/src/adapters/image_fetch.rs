use crate::adapters::net::{
    GuardedGetError, Lookup, SystemLookup, UrlGuard, guarded_client_builder, guarded_get,
    guarded_request,
};
use crate::domain::error::DomainError;
use crate::domain::ports::image_fetcher::{FetchedImage, ImageFetcher};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use url::Url;

/// Larger images are refused; card images are resized to 1200x630 anyway.
const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);

/// Fetches card images through the guarded client: public addresses only
/// (checked after DNS), every redirect hop checked, no proxy, web ports only.
pub struct GuardedImageFetcher {
    client: reqwest::Client,
    guard: UrlGuard,
}

impl GuardedImageFetcher {
    pub fn new() -> Self {
        Self::with_lookup(UrlGuard::public(), SystemLookup)
    }

    pub(crate) fn with_lookup(guard: UrlGuard, lookup: impl Lookup) -> Self {
        let client = guarded_client_builder(guard, lookup)
            .timeout(TIMEOUT)
            .build()
            .expect("failed to build image HTTP client");
        Self { client, guard }
    }

    /// Also reaches loopback fixture servers.
    #[cfg(test)]
    pub fn allowing_loopback() -> Self {
        Self::with_lookup(UrlGuard::allowing_loopback(), SystemLookup)
    }

    async fn download(&self, url: &str) -> Result<FetchedImage, DomainError> {
        let url = Url::parse(url)
            .map_err(|e| DomainError::InvalidInput(format!("invalid image URL: {e}")))?;
        let mut resp = guarded_get(&self.client, self.guard, None, url)
            .await
            .map_err(fetch_error)?;
        if !resp.status().is_success() {
            return Err(DomainError::Internal(format!(
                "image fetch returned HTTP {}",
                resp.status()
            )));
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| {
                v.split(';')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase()
            })
            .unwrap_or_default();
        if !content_type.starts_with("image/") {
            return Err(DomainError::InvalidInput(format!(
                "not an image: {}",
                if content_type.is_empty() {
                    "no content type"
                } else {
                    &content_type
                }
            )));
        }
        if resp
            .content_length()
            .is_some_and(|len| len > MAX_IMAGE_BYTES as u64)
        {
            return Err(too_big());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| DomainError::Internal(format!("image read error: {e}")))?
        {
            if bytes.len() + chunk.len() > MAX_IMAGE_BYTES {
                return Err(too_big());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(FetchedImage {
            bytes,
            content_type,
        })
    }

    /// Some image hosts (including common CDNs) reject HEAD while serving the
    /// same resource over GET. Then a one-byte ranged GET decides, and only
    /// its headers are read.
    async fn reachable(&self, url: &str) -> bool {
        let Ok(url) = Url::parse(url) else {
            return false;
        };
        let head = guarded_request(&self.client, self.guard, None, url.clone(), |c, u| {
            c.head(u)
        });
        if head.await.is_ok_and(|r| r.status().is_success()) {
            return true;
        }
        guarded_request(&self.client, self.guard, None, url, |c, u| {
            c.get(u).header(reqwest::header::RANGE, "bytes=0-0")
        })
        .await
        .is_ok_and(|r| r.status().is_success())
    }
}

impl ImageFetcher for GuardedImageFetcher {
    fn fetch(
        &self,
        url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<FetchedImage, DomainError>> + Send + '_>> {
        let url = url.to_string();
        Box::pin(async move { self.download(&url).await })
    }

    fn is_reachable(&self, url: &str) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        let url = url.to_string();
        Box::pin(async move { self.reachable(&url).await })
    }
}

fn fetch_error(err: GuardedGetError) -> DomainError {
    match err {
        GuardedGetError::Blocked(blocked) => DomainError::InvalidInput(blocked.to_string()),
        GuardedGetError::TooManyRedirects => {
            DomainError::Internal("image fetch error: too many redirects".to_string())
        }
        GuardedGetError::Request(e) => DomainError::Internal(format!("image fetch error: {e}")),
    }
}

fn too_big() -> DomainError {
    DomainError::InvalidInput(format!(
        "image is larger than {} MB",
        MAX_IMAGE_BYTES / (1024 * 1024)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::net::FakeLookup;
    use axum::Router;
    use axum::http::header;
    use axum::routing::get;
    use std::collections::HashMap;

    async fn site() -> String {
        let app = Router::new()
            .route(
                "/cat.png",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "image/png")],
                        vec![0x89, b'P', b'N', b'G'],
                    )
                }),
            )
            .route(
                "/page",
                get(|| async { ([(header::CONTENT_TYPE, "text/html")], "<html></html>") }),
            )
            .route(
                "/huge.jpg",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "image/jpeg")],
                        vec![0u8; MAX_IMAGE_BYTES + 1],
                    )
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://127.0.0.1:{port}")
    }

    #[tokio::test]
    async fn downloads_images_and_refuses_other_content() {
        let base = site().await;
        let fetcher = GuardedImageFetcher::allowing_loopback();
        let image = fetcher.fetch(&format!("{base}/cat.png")).await.unwrap();
        assert_eq!(image.content_type, "image/png");
        assert_eq!(image.bytes, vec![0x89, b'P', b'N', b'G']);
        assert!(matches!(
            fetcher.fetch(&format!("{base}/page")).await,
            Err(DomainError::InvalidInput(msg)) if msg.contains("not an image: text/html")
        ));
        assert!(matches!(
            fetcher.fetch(&format!("{base}/huge.jpg")).await,
            Err(DomainError::InvalidInput(msg)) if msg.contains("larger than 10 MB")
        ));
        assert!(fetcher.is_reachable(&format!("{base}/cat.png")).await);
        assert!(!fetcher.is_reachable(&format!("{base}/missing.png")).await);
    }

    #[tokio::test]
    async fn refuses_image_urls_on_private_addresses() {
        let fetcher = GuardedImageFetcher::with_lookup(
            UrlGuard::public(),
            FakeLookup(HashMap::from([("cdn.intranet.example", vec!["10.0.0.9"])])),
        );
        for url in [
            "http://cdn.intranet.example/logo.png",
            "http://169.254.169.254/latest/meta-data/",
            "http://127.0.0.1:4000/uploads/x.png",
        ] {
            assert!(
                matches!(fetcher.fetch(url).await, Err(DomainError::InvalidInput(msg)) if msg.contains("refusing")),
                "{url}"
            );
            assert!(!fetcher.is_reachable(url).await, "{url}");
        }
    }
}
