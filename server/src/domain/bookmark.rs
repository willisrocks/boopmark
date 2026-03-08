use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Bookmark {
    pub id: Uuid,
    pub user_id: Uuid,
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub domain: Option<String>,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateBookmark {
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub domain: Option<String>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateBookmark {
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
pub struct BookmarkFilter {
    pub search: Option<String>,
    pub tags: Option<Vec<String>>,
    pub sort: Option<BookmarkSort>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BookmarkSort {
    #[default]
    Newest,
    Oldest,
    Title,
    Domain,
}

#[derive(Debug, Serialize)]
pub struct UrlMetadata {
    pub title: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub domain: Option<String>,
}
