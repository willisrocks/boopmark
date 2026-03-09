use crate::domain::bookmark::*;
use crate::domain::error::DomainError;
use crate::domain::ports::bookmark_repo::BookmarkRepository;
use crate::domain::ports::metadata::MetadataExtractor;
use crate::domain::ports::storage::ObjectStorage;
use std::sync::Arc;
use uuid::Uuid;

pub struct BookmarkService<R, M, S> {
    repo: Arc<R>,
    metadata: Arc<M>,
    storage: Arc<S>,
    http_client: reqwest::Client,
}

impl<R, M, S> BookmarkService<R, M, S>
where
    R: BookmarkRepository + Send + Sync,
    M: MetadataExtractor + Send + Sync,
    S: ObjectStorage + Send + Sync,
{
    pub fn new(repo: Arc<R>, metadata: Arc<M>, storage: Arc<S>) -> Self {
        let http_client = reqwest::Client::builder()
            .user_agent("Boopmark/1.0 (+https://boopmark.app)")
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");
        Self {
            repo,
            metadata,
            storage,
            http_client,
        }
    }

    pub async fn create(
        &self,
        user_id: Uuid,
        mut input: CreateBookmark,
    ) -> Result<Bookmark, DomainError> {
        if needs_metadata(&input) {
            if let Ok(meta) = self.metadata.extract(&input.url).await {
                if let Some(image_url) = merge_metadata(&mut input, meta) {
                    if let Ok(stored_url) = self.download_and_store_image(&image_url).await {
                        input.image_url = Some(stored_url);
                    }
                }
            }
        }

        // Extract domain from URL if not set
        if input.domain.is_none() {
            if let Ok(parsed) = url::Url::parse(&input.url) {
                input.domain = parsed.host_str().map(|h| h.to_string());
            }
        }

        self.repo.create(user_id, input).await
    }

    pub async fn list(
        &self,
        user_id: Uuid,
        filter: BookmarkFilter,
    ) -> Result<Vec<Bookmark>, DomainError> {
        self.repo.list(user_id, filter).await
    }

    pub async fn get(&self, id: Uuid, user_id: Uuid) -> Result<Bookmark, DomainError> {
        self.repo.get(id, user_id).await
    }

    pub async fn update(
        &self,
        id: Uuid,
        user_id: Uuid,
        input: UpdateBookmark,
    ) -> Result<Bookmark, DomainError> {
        self.repo.update(id, user_id, input).await
    }

    pub async fn delete(&self, id: Uuid, user_id: Uuid) -> Result<(), DomainError> {
        self.repo.delete(id, user_id).await
    }

    pub async fn extract_metadata(&self, url: &str) -> Result<UrlMetadata, DomainError> {
        self.metadata.extract(url).await
    }

    async fn download_and_store_image(&self, image_url: &str) -> Result<String, DomainError> {
        let resp = self
            .http_client
            .get(image_url)
            .send()
            .await
            .map_err(|e| DomainError::Internal(format!("image fetch error: {e}")))?;

        if !resp.status().is_success() {
            return Err(DomainError::Internal(format!(
                "image fetch returned HTTP {}",
                resp.status()
            )));
        }

        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_string();

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| DomainError::Internal(format!("image read error: {e}")))?;

        let key = format!(
            "images/{}.{}",
            Uuid::new_v4(),
            extension_from_content_type(&content_type)
        );
        self.storage.put(&key, bytes.to_vec(), &content_type).await
    }
}

fn extension_from_content_type(ct: &str) -> &str {
    let mime = ct.split(';').next().unwrap_or(ct).trim();
    match mime {
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        _ => "jpg",
    }
}

fn needs_metadata(input: &CreateBookmark) -> bool {
    input.title.is_none()
        || input.description.is_none()
        || input.domain.is_none()
        || input.image_url.is_none()
}

fn merge_metadata(input: &mut CreateBookmark, meta: UrlMetadata) -> Option<String> {
    if input.title.is_none() {
        input.title = meta.title;
    }
    if input.description.is_none() {
        input.description = meta.description;
    }
    if input.domain.is_none() {
        input.domain = meta.domain;
    }
    if input.image_url.is_none() {
        return meta.image_url;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_metadata_when_image_or_domain_is_missing() {
        let input = CreateBookmark {
            url: "https://github.com/danshapiro/trycycle".to_string(),
            title: Some("trycycle".to_string()),
            description: Some("already filled".to_string()),
            image_url: None,
            domain: None,
            tags: None,
        };

        assert!(needs_metadata(&input));
    }

    #[test]
    fn merge_metadata_preserves_user_text_but_returns_missing_image() {
        let mut input = CreateBookmark {
            url: "https://github.com/danshapiro/trycycle".to_string(),
            title: Some("Custom Title".to_string()),
            description: None,
            image_url: None,
            domain: None,
            tags: None,
        };

        let image = merge_metadata(
            &mut input,
            UrlMetadata {
                title: Some("Suggested Title".to_string()),
                description: Some("Suggested description".to_string()),
                image_url: Some("https://example.com/preview.png".to_string()),
                domain: Some("github.com".to_string()),
            },
        );

        assert_eq!(input.title.as_deref(), Some("Custom Title"));
        assert_eq!(input.description.as_deref(), Some("Suggested description"));
        assert_eq!(input.domain.as_deref(), Some("github.com"));
        assert_eq!(image.as_deref(), Some("https://example.com/preview.png"));
    }
}
