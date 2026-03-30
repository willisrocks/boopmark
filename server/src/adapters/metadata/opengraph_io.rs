use crate::domain::bookmark::UrlMetadata;
use crate::domain::error::DomainError;
use crate::domain::ports::metadata::MetadataExtractor;
use std::future::Future;
use std::pin::Pin;

pub struct OpengraphIoExtractor {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
}

#[derive(serde::Deserialize)]
struct OpengraphIoResponse {
    #[serde(rename = "hybridGraph")]
    hybrid_graph: Option<HybridGraph>,
}

#[derive(serde::Deserialize)]
struct HybridGraph {
    title: Option<String>,
    description: Option<String>,
    image: Option<String>,
}

impl OpengraphIoExtractor {
    pub fn new(api_key: String) -> Self {
        Self::with_base_url(api_key, "https://opengraph.io".to_string())
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

impl MetadataExtractor for OpengraphIoExtractor {
    fn extract(
        &self,
        url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<UrlMetadata, DomainError>> + Send + '_>> {
        let url = url.to_string();
        Box::pin(async move {
            let encoded_url = urlencoding::encode(&url);
            let resp = self
                .client
                .get(format!("{}/api/1.1/site/{}", self.base_url, encoded_url))
                .query(&[("app_id", &self.api_key)])
                .send()
                .await
                .map_err(|e| DomainError::Internal(format!("opengraph.io fetch error: {e}")))?;

            if !resp.status().is_success() {
                return Err(DomainError::Internal(format!(
                    "opengraph.io returned HTTP {}",
                    resp.status()
                )));
            }

            let data: OpengraphIoResponse = resp
                .json()
                .await
                .map_err(|e| DomainError::Internal(format!("opengraph.io parse error: {e}")))?;

            let graph = data.hybrid_graph.unwrap_or(HybridGraph {
                title: None,
                description: None,
                image: None,
            });

            let domain = url::Url::parse(&url)
                .ok()
                .and_then(|u| u.host_str().map(|h| h.to_string()));

            Ok(UrlMetadata {
                title: graph.title,
                description: graph.description,
                image_url: graph.image,
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
            "hybridGraph": {
                "title": "OG Test",
                "description": "OG description",
                "image": "https://cdn.example.com/og.jpg"
            }
        }))
    }

    async fn mock_error() -> (axum::http::StatusCode, &'static str) {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "Internal Server Error",
        )
    }

    #[tokio::test]
    async fn parses_opengraph_io_response() {
        // Use fallback routing because the opengraph.io API encodes the target
        // URL in the request path, making static route matching impractical.
        let app = Router::new().fallback(get(mock_success));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(axum::serve(listener, app).into_future());

        let extractor =
            OpengraphIoExtractor::with_base_url("test-key".to_string(), format!("http://{}", addr));
        let result = extractor
            .extract("https://medium.com/some-article")
            .await
            .unwrap();
        assert_eq!(result.title, Some("OG Test".to_string()));
        assert_eq!(result.description, Some("OG description".to_string()));
        assert_eq!(
            result.image_url,
            Some("https://cdn.example.com/og.jpg".to_string())
        );
    }

    #[tokio::test]
    async fn returns_error_on_api_failure() {
        let app = Router::new().fallback(get(mock_error));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(axum::serve(listener, app).into_future());

        let extractor =
            OpengraphIoExtractor::with_base_url("bad-key".to_string(), format!("http://{}", addr));
        let result = extractor.extract("https://medium.com/some-article").await;
        assert!(result.is_err());
    }
}
