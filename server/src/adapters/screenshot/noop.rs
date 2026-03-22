use crate::domain::error::DomainError;
use crate::domain::ports::screenshot::ScreenshotProvider;
use std::future::Future;
use std::pin::Pin;

/// No-op screenshot provider used when screenshots are disabled.
pub struct NoopScreenshot;

impl ScreenshotProvider for NoopScreenshot {
    fn capture(
        &self,
        _url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, DomainError>> + Send + '_>> {
        Box::pin(async { Err(DomainError::Internal("screenshots are disabled".into())) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn noop_capture_returns_error() {
        let client = NoopScreenshot;
        let result = client.capture("https://example.com").await;
        assert!(result.is_err());
    }
}
