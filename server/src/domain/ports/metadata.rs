use crate::domain::bookmark::UrlMetadata;
use crate::domain::error::DomainError;
use std::future::Future;
use std::pin::Pin;

pub trait MetadataExtractor: Send + Sync {
    fn extract(
        &self,
        url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<UrlMetadata, DomainError>> + Send + '_>>;
}
