use crate::domain::bookmark::UrlMetadata;
use crate::domain::error::DomainError;
use crate::domain::ports::metadata::MetadataExtractor;
use std::future::Future;
use std::pin::Pin;

pub struct IframelyExtractor {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
}

#[derive(serde::Deserialize)]
struct IframelyResponse {
    meta: Option<IframelyMeta>,
    links: Option<IframelyLinks>,
}

#[derive(serde::Deserialize)]
struct IframelyMeta {
    title: Option<String>,
    description: Option<String>,
}

#[derive(serde::Deserialize)]
struct IframelyLinks {
    thumbnail: Option<Vec<IframelyThumbnail>>,
}

#[derive(serde::Deserialize)]
struct IframelyThumbnail {
    href: Option<String>,
}

impl IframelyExtractor {
    pub fn new(api_key: String) -> Self {
        Self::with_base_url(api_key, "https://iframe.ly".to_string())
    }

    fn with_base_url(api_key: String, base_url: String) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap(),
            api_key,
            base_url,
        }
    }
}

impl MetadataExtractor for IframelyExtractor {
    fn extract(
        &self,
        url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<UrlMetadata, DomainError>> + Send + '_>> {
        let url = url.to_string();
        Box::pin(async move {
            let clean_url = super::validate_public_url(&url)?;
            let resp = self
                .client
                .get(format!("{}/api/iframely", self.base_url))
                .query(&[("url", &clean_url), ("api_key", &self.api_key)])
                .send()
                .await
                .map_err(|e| DomainError::Internal(format!("iframely fetch error: {e}")))?;

            if !resp.status().is_success() {
                return Err(DomainError::Internal(format!(
                    "iframely returned HTTP {}",
                    resp.status()
                )));
            }

            let data: IframelyResponse = resp
                .json()
                .await
                .map_err(|e| DomainError::Internal(format!("iframely parse error: {e}")))?;

            let meta = data.meta.unwrap_or(IframelyMeta {
                title: None,
                description: None,
            });
            let image_url = data
                .links
                .and_then(|l| l.thumbnail)
                .and_then(|t| t.into_iter().next())
                .and_then(|t| t.href);

            let domain = url::Url::parse(&url)
                .ok()
                .and_then(|u| u.host_str().map(|h| h.to_string()));

            Ok(UrlMetadata {
                title: meta.title,
                description: meta.description,
                image_url,
                domain,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, routing::get};

    async fn mock_success() -> Json<serde_json::Value> {
        Json(serde_json::json!({
            "meta": {
                "title": "Test Article",
                "description": "A test description"
            },
            "links": {
                "thumbnail": [{"href": "https://cdn.example.com/thumb.jpg"}]
            }
        }))
    }

    async fn mock_error() -> (axum::http::StatusCode, &'static str) {
        (axum::http::StatusCode::FORBIDDEN, "Forbidden")
    }

    #[tokio::test]
    async fn parses_iframely_response() {
        let app = Router::new().route("/api/iframely", get(mock_success));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(axum::serve(listener, app).into_future());

        let extractor =
            IframelyExtractor::with_base_url("test-key".to_string(), format!("http://{}", addr));
        let result = extractor
            .extract("https://medium.com/some-article")
            .await
            .unwrap();
        assert_eq!(result.title, Some("Test Article".to_string()));
        assert_eq!(result.description, Some("A test description".to_string()));
        assert_eq!(
            result.image_url,
            Some("https://cdn.example.com/thumb.jpg".to_string())
        );
    }

    #[tokio::test]
    async fn returns_error_on_api_failure() {
        let app = Router::new().route("/api/iframely", get(mock_error));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(axum::serve(listener, app).into_future());

        let extractor =
            IframelyExtractor::with_base_url("bad-key".to_string(), format!("http://{}", addr));
        let result = extractor.extract("https://medium.com/some-article").await;
        assert!(result.is_err());
    }
}
