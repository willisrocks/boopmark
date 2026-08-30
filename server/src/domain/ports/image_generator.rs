use crate::domain::error::DomainError;
use std::future::Future;
use std::pin::Pin;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ImageGenerationContext {
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug)]
pub struct GeneratedImage {
    pub bytes: Vec<u8>,
    pub mime_type: String,
}

pub trait ImageGenerator: Send + Sync {
    fn generate(
        &self,
        user_id: Uuid,
        context: ImageGenerationContext,
    ) -> Pin<Box<dyn Future<Output = Result<GeneratedImage, DomainError>> + Send + '_>>;
}
