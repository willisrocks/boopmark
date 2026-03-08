use crate::adapters::postgres::PostgresPool;
use crate::adapters::scraper::HtmlMetadataExtractor;
use crate::adapters::storage::local::LocalStorage;
use crate::adapters::storage::s3::S3Storage;
use crate::app::auth::AuthService;
use crate::app::bookmarks::BookmarkService;
use crate::config::Config;
use std::sync::Arc;

/// Application state shared across all request handlers.
///
/// Uses an enum to handle the two storage backends at the type level,
/// avoiding dyn dispatch while keeping a single AppState type.
#[derive(Clone)]
pub struct AppState {
    pub bookmarks: Bookmarks,
    pub auth: Arc<AuthService<PostgresPool, PostgresPool, PostgresPool>>,
    pub config: Arc<Config>,
}

#[derive(Clone)]
pub enum Bookmarks {
    Local(Arc<BookmarkService<PostgresPool, HtmlMetadataExtractor, LocalStorage>>),
    S3(Arc<BookmarkService<PostgresPool, HtmlMetadataExtractor, S3Storage>>),
}
