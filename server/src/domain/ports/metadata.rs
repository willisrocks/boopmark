use crate::domain::bookmark::UrlMetadata;
use crate::domain::error::DomainError;

#[trait_variant::make(Send)]
pub trait MetadataExtractor: Send + Sync {
    async fn extract(&self, url: &str) -> Result<UrlMetadata, DomainError>;
}
