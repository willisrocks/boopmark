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
    pub override_image_url: Option<String>,
    pub domain: Option<String>,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Bookmark {
    /// The image shown on cards and in the edit preview. The scraped/imported
    /// image remains intact so removing an override can restore it.
    pub fn effective_image_url(&self) -> Option<&str> {
        self.override_image_url
            .as_deref()
            .or(self.image_url.as_deref())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BookmarkSort {
    #[default]
    Newest,
    Oldest,
    Title,
    Domain,
    /// Search rank. Only meaningful with a search; a plain list treats it as `Newest`.
    Relevance,
}

impl BookmarkSort {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "newest" => Some(Self::Newest),
            "oldest" => Some(Self::Oldest),
            "title" => Some(Self::Title),
            "domain" => Some(Self::Domain),
            "relevance" => Some(Self::Relevance),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Newest => "newest",
            Self::Oldest => "oldest",
            Self::Title => "title",
            Self::Domain => "domain",
            Self::Relevance => "relevance",
        }
    }

    /// The sort to apply: a search defaults to relevance, and relevance means
    /// nothing without one.
    pub fn resolve(requested: Option<Self>, searching: bool) -> Self {
        match requested {
            Some(Self::Relevance) if !searching => Self::Newest,
            Some(sort) => sort,
            None if searching => Self::Relevance,
            None => Self::Newest,
        }
    }
}

impl BookmarkFilter {
    /// The search text, trimmed; `None` when there is nothing to search for.
    pub fn search_text(&self) -> Option<&str> {
        self.search
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }
}

#[derive(Debug, Serialize)]
pub struct UrlMetadata {
    pub title: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub domain: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_search_defaults_to_relevance_and_a_list_never_uses_it() {
        assert_eq!(BookmarkSort::resolve(None, true), BookmarkSort::Relevance);
        assert_eq!(BookmarkSort::resolve(None, false), BookmarkSort::Newest);
        assert_eq!(
            BookmarkSort::resolve(Some(BookmarkSort::Relevance), false),
            BookmarkSort::Newest
        );
        assert_eq!(
            BookmarkSort::resolve(Some(BookmarkSort::Title), true),
            BookmarkSort::Title
        );
        for sort in ["newest", "oldest", "title", "domain", "relevance"] {
            assert_eq!(BookmarkSort::parse(sort).unwrap().as_str(), sort);
        }
        assert_eq!(BookmarkSort::parse("bogus"), None);
    }

    #[test]
    fn search_text_is_trimmed_and_empty_means_none() {
        let filter = |s: &str| BookmarkFilter {
            search: Some(s.into()),
            ..Default::default()
        };
        assert_eq!(filter("  rust  ").search_text(), Some("rust"));
        assert_eq!(filter("   ").search_text(), None);
        assert_eq!(BookmarkFilter::default().search_text(), None);
    }
}
