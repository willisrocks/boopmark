use crate::domain::error::DomainError;
use crate::domain::ports::screenshot::ScreenshotProvider;
use std::future::Future;
use std::pin::Pin;

pub struct PlaywrightScreenshot {
    http: reqwest::Client,
    base_url: String,
}

impl PlaywrightScreenshot {
    pub fn new(base_url: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build screenshot HTTP client");
        Self { http, base_url }
    }
}

impl ScreenshotProvider for PlaywrightScreenshot {
    fn capture(
        &self,
        url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, DomainError>> + Send + '_>> {
        let url = url.to_string();
        Box::pin(async move {
            let resp = self
                .http
                .post(format!("{}/screenshot", self.base_url))
                .json(&serde_json::json!({ "url": url }))
                .send()
                .await
                .map_err(|e| DomainError::Internal(e.to_string()))?;

            if !resp.status().is_success() {
                return Err(DomainError::Internal(format!(
                    "screenshot sidecar returned {}",
                    resp.status()
                )));
            }

            resp.bytes()
                .await
                .map(|b| b.to_vec())
                .map_err(|e| DomainError::Internal(e.to_string()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, response::IntoResponse, routing::post};

    async fn fake_screenshot() -> impl IntoResponse {
        let jpeg_bytes: Vec<u8> = vec![0xFF, 0xD8, 0xFF, 0xD9];
        (
            axum::http::StatusCode::OK,
            [("Content-Type", "image/jpeg")],
            jpeg_bytes,
        )
    }

    #[tokio::test]
    async fn capture_returns_bytes_from_sidecar() {
        let app = Router::new().route("/screenshot", post(fake_screenshot));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let client = PlaywrightScreenshot::new(format!("http://{}", addr));
        let result = client.capture("https://example.com").await;

        assert!(result.is_ok());
        let bytes = result.unwrap();
        assert_eq!(&bytes[..2], &[0xFF, 0xD8]);
    }

    #[tokio::test]
    async fn capture_returns_error_on_sidecar_failure() {
        let app = Router::new().route(
            "/screenshot",
            post(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let client = PlaywrightScreenshot::new(format!("http://{}", addr));
        let result = client.capture("https://example.com").await;

        assert!(result.is_err());
    }
}
