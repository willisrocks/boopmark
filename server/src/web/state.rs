use crate::adapters::postgres::PostgresPool;
use crate::adapters::scraper::HtmlMetadataExtractor;
use crate::adapters::storage::local::LocalStorage;
use crate::adapters::storage::s3::S3Storage;
use crate::app::auth::AuthService;
use crate::app::bookmarks::BookmarkService;
use crate::app::enrichment::EnrichmentService;
use crate::app::settings::SettingsService;
use crate::config::Config;
use crate::domain::error::DomainError;
use crate::domain::ports::storage::ObjectStorage;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Application state shared across all request handlers.
///
/// Uses an enum to handle the two storage backends at the type level,
/// avoiding dyn dispatch while keeping a single AppState type.
#[derive(Clone)]
pub struct AppState {
    pub bookmarks: Bookmarks,
    pub auth: Arc<AuthService<PostgresPool, PostgresPool, PostgresPool>>,
    pub settings: Arc<SettingsService<PostgresPool>>,
    pub config: Arc<Config>,
    pub enrichment: Arc<EnrichmentService<HtmlMetadataExtractor, PostgresPool>>,
    pub images_storage: ImageStorage,
    pub active_image_fix_jobs: Arc<Mutex<HashSet<Uuid>>>,
}

#[derive(Clone)]
pub enum Bookmarks {
    Local(Arc<BookmarkService<PostgresPool, HtmlMetadataExtractor, LocalStorage>>),
    S3(Arc<BookmarkService<PostgresPool, HtmlMetadataExtractor, S3Storage>>),
}

/// Enum-based storage for avatar images, matching the pattern used by `Bookmarks`.
#[derive(Clone)]
pub enum ImageStorage {
    Local(LocalStorage),
    S3(S3Storage),
}

impl ObjectStorage for ImageStorage {
    async fn put(
        &self,
        key: &str,
        data: Vec<u8>,
        content_type: &str,
    ) -> Result<String, DomainError> {
        match self {
            Self::Local(s) => s.put(key, data, content_type).await,
            Self::S3(s) => s.put(key, data, content_type).await,
        }
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, DomainError> {
        match self {
            Self::Local(s) => s.get(key).await,
            Self::S3(s) => s.get(key).await,
        }
    }

    async fn delete(&self, key: &str) -> Result<(), DomainError> {
        match self {
            Self::Local(s) => s.delete(key).await,
            Self::S3(s) => s.delete(key).await,
        }
    }

    fn public_url(&self, key: &str) -> String {
        match self {
            Self::Local(s) => s.public_url(key),
            Self::S3(s) => s.public_url(key),
        }
    }
}

impl ImageStorage {
    /// Extract the storage key from a full public URL produced by this storage.
    /// Returns `None` if the URL does not match this storage's prefix.
    pub fn key_from_url(&self, url: &str) -> Option<String> {
        // public_url("") gives us the prefix with a trailing separator
        let prefix = self.public_url("");
        url.strip_prefix(prefix.trim_end_matches('/'))
            .map(|rest| rest.trim_start_matches('/').to_string())
            .filter(|key| !key.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_from_url_extracts_key_from_local_url() {
        let storage = ImageStorage::Local(LocalStorage::new(
            "./uploads/images".into(),
            "http://localhost:4000/uploads/images".to_string(),
        ));
        assert_eq!(
            storage.key_from_url("http://localhost:4000/uploads/images/avatars/abc.jpg"),
            Some("avatars/abc.jpg".to_string()),
        );
    }

    #[test]
    fn key_from_url_returns_none_for_foreign_url() {
        let storage = ImageStorage::Local(LocalStorage::new(
            "./uploads/images".into(),
            "http://localhost:4000/uploads/images".to_string(),
        ));
        assert_eq!(
            storage.key_from_url("https://lh3.googleusercontent.com/photo.jpg"),
            None,
        );
    }

    #[test]
    fn key_from_url_returns_none_for_empty_key() {
        let storage = ImageStorage::Local(LocalStorage::new(
            "./uploads/images".into(),
            "http://localhost:4000/uploads/images".to_string(),
        ));
        assert_eq!(
            storage.key_from_url("http://localhost:4000/uploads/images/"),
            None,
        );
    }
}
