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
}

impl<R, M, S> BookmarkService<R, M, S>
where
    R: BookmarkRepository + Send + Sync,
    M: MetadataExtractor + Send + Sync,
    S: ObjectStorage + Send + Sync,
{
    pub fn new(repo: Arc<R>, metadata: Arc<M>, storage: Arc<S>) -> Self {
        Self { repo, metadata, storage }
    }

    pub async fn create(&self, user_id: Uuid, mut input: CreateBookmark) -> Result<Bookmark, DomainError> {
        // Auto-extract metadata if title not provided
        if input.title.is_none() {
            if let Ok(meta) = self.metadata.extract(&input.url).await {
                input.title = input.title.or(meta.title);
                input.description = input.description.or(meta.description);
                input.domain = input.domain.or(meta.domain);

                // Download and store og:image
                if let Some(image_url) = meta.image_url {
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

    pub async fn list(&self, user_id: Uuid, filter: BookmarkFilter) -> Result<Vec<Bookmark>, DomainError> {
        self.repo.list(user_id, filter).await
    }

    pub async fn get(&self, id: Uuid, user_id: Uuid) -> Result<Bookmark, DomainError> {
        self.repo.get(id, user_id).await
    }

    pub async fn update(&self, id: Uuid, user_id: Uuid, input: UpdateBookmark) -> Result<Bookmark, DomainError> {
        self.repo.update(id, user_id, input).await
    }

    pub async fn delete(&self, id: Uuid, user_id: Uuid) -> Result<(), DomainError> {
        self.repo.delete(id, user_id).await
    }

    pub async fn extract_metadata(&self, url: &str) -> Result<UrlMetadata, DomainError> {
        self.metadata.extract(url).await
    }

    async fn download_and_store_image(&self, image_url: &str) -> Result<String, DomainError> {
        let client = reqwest::Client::builder()
            .user_agent("Boopmark/1.0 (+https://boopmark.app)")
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| DomainError::Internal(format!("client build error: {e}")))?;
        let resp = client.get(image_url).send().await
            .map_err(|e| DomainError::Internal(format!("image fetch error: {e}")))?;

        let content_type = resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_string();

        let bytes = resp.bytes().await
            .map_err(|e| DomainError::Internal(format!("image read error: {e}")))?;

        let key = format!("images/{}.{}", Uuid::new_v4(), extension_from_content_type(&content_type));
        self.storage.put(&key, bytes.to_vec(), &content_type).await
    }
}

fn extension_from_content_type(ct: &str) -> &str {
    match ct {
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        _ => "jpg",
    }
}
