use crate::adapters::postgres::PostgresPool;
use crate::adapters::scraper::HtmlMetadataExtractor;
use crate::adapters::storage::local::LocalStorage;
use crate::adapters::storage::s3::S3Storage;
use crate::app::auth::AuthService;
use crate::app::bookmarks::BookmarkService;
use crate::app::settings::SettingsService;
use crate::config::Config;
use crate::domain::error::DomainError;
use crate::domain::ports::llm_enricher::LlmEnricher;
use crate::domain::ports::storage::ObjectStorage;
use std::sync::Arc;

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
    pub enricher: Arc<dyn LlmEnricher>,
    pub images_storage: ImageStorage,
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
