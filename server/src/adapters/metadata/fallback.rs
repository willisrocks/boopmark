use crate::domain::bookmark::UrlMetadata;
use crate::domain::error::DomainError;
use crate::domain::ports::metadata::MetadataExtractor;
use std::future::Future;
use std::pin::Pin;

pub struct FallbackMetadataExtractor {
    extractors: Vec<Box<dyn MetadataExtractor>>,
}

impl FallbackMetadataExtractor {
    pub fn new(extractors: Vec<Box<dyn MetadataExtractor>>) -> Self {
        Self { extractors }
    }
}

impl MetadataExtractor for FallbackMetadataExtractor {
    fn extract(
        &self,
        url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<UrlMetadata, DomainError>> + Send + '_>> {
        let url = url.to_string();
        Box::pin(async move {
            let mut last_err =
                DomainError::Internal("no metadata extractors configured".to_string());
            for extractor in &self.extractors {
                match extractor.extract(&url).await {
                    Ok(meta) => return Ok(meta),
                    Err(e) => {
                        tracing::warn!(url = %url, error = %e, "metadata extractor failed, trying next");
                        last_err = e;
                    }
                }
            }
            Err(last_err)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FailingExtractor;
    impl MetadataExtractor for FailingExtractor {
        fn extract(
            &self,
            _url: &str,
        ) -> Pin<Box<dyn Future<Output = Result<UrlMetadata, DomainError>> + Send + '_>> {
            Box::pin(async { Err(DomainError::Internal("blocked".to_string())) })
        }
    }

    struct SuccessExtractor {
        title: Option<String>,
    }
    impl MetadataExtractor for SuccessExtractor {
        fn extract(
            &self,
            _url: &str,
        ) -> Pin<Box<dyn Future<Output = Result<UrlMetadata, DomainError>> + Send + '_>> {
            let title = self.title.clone();
            Box::pin(async move {
                Ok(UrlMetadata {
                    title,
                    description: None,
                    image_url: Some("https://example.com/img.jpg".to_string()),
                    domain: None,
                })
            })
        }
    }

    #[tokio::test]
    async fn falls_back_to_second_extractor_on_error() {
        let fallback = FallbackMetadataExtractor::new(vec![
            Box::new(FailingExtractor),
            Box::new(SuccessExtractor {
                title: Some("Fallback Title".to_string()),
            }),
        ]);
        let result = fallback.extract("https://example.com").await.unwrap();
        assert_eq!(result.title, Some("Fallback Title".to_string()));
        assert_eq!(
            result.image_url,
            Some("https://example.com/img.jpg".to_string())
        );
    }

    #[tokio::test]
    async fn returns_first_success_without_trying_later() {
        let fallback = FallbackMetadataExtractor::new(vec![
            Box::new(SuccessExtractor {
                title: Some("First".to_string()),
            }),
            Box::new(SuccessExtractor {
                title: Some("Second".to_string()),
            }),
        ]);
        let result = fallback.extract("https://example.com").await.unwrap();
        assert_eq!(result.title, Some("First".to_string()));
    }

    #[tokio::test]
    async fn returns_last_error_when_all_fail() {
        let fallback = FallbackMetadataExtractor::new(vec![
            Box::new(FailingExtractor),
            Box::new(FailingExtractor),
        ]);
        let result = fallback.extract("https://example.com").await;
        assert!(result.is_err());
    }
}
