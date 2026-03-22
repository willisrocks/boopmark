use crate::domain::error::DomainError;
use std::future::Future;
use std::pin::Pin;

pub trait ScreenshotProvider: Send + Sync {
    fn capture(
        &self,
        url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, DomainError>> + Send + '_>>;
}
