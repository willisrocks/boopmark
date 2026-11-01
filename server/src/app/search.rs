use crate::domain::bookmark::{BookmarkFilter, BookmarkSort};
use crate::domain::error::DomainError;
use crate::domain::ports::search_repo::SearchRepository;
use crate::domain::search::{MAX_SEARCH_RESULTS, SearchHit, SearchQuery};
use std::sync::Arc;
use uuid::Uuid;

/// Longer queries are cut here. The legs do not get better with more words,
/// and trigram matching cost grows with the query.
const MAX_QUERY_CHARS: usize = 256;

pub struct SearchService<R> {
    repo: Arc<R>,
}

impl<R: SearchRepository> SearchService<R> {
    pub fn new(repo: Arc<R>) -> Self {
        Self { repo }
    }

    /// Relevance-ranked search. The filter's `search` must be non-empty
    /// (an empty search is a plain list); `sort` defaults to relevance, and
    /// `limit`/`offset` page through at most `MAX_SEARCH_RESULTS` results.
    pub async fn search(
        &self,
        user_id: Uuid,
        filter: BookmarkFilter,
    ) -> Result<Vec<SearchHit>, DomainError> {
        let text: String = filter
            .search
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .chars()
            .take(MAX_QUERY_CHARS)
            .collect();
        if text.is_empty() {
            return Err(DomainError::InvalidInput("search text is required".into()));
        }
        let offset = filter.offset.unwrap_or(0).clamp(0, MAX_SEARCH_RESULTS);
        let limit = filter
            .limit
            .unwrap_or(MAX_SEARCH_RESULTS)
            .clamp(0, MAX_SEARCH_RESULTS - offset);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let query = SearchQuery {
            text,
            tags: filter.tags.filter(|tags| !tags.is_empty()),
            sort: filter.sort.unwrap_or(BookmarkSort::Relevance),
            limit,
            offset,
        };
        self.repo.search(user_id, &query).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::search::{IngestFailure, IngestJob};
    use std::sync::Mutex;
    use std::time::Duration;

    #[derive(Default)]
    struct RecordingRepo {
        queries: Mutex<Vec<SearchQuery>>,
    }

    impl SearchRepository for RecordingRepo {
        async fn search(
            &self,
            _user_id: Uuid,
            query: &SearchQuery,
        ) -> Result<Vec<SearchHit>, DomainError> {
            self.queries.lock().unwrap().push(query.clone());
            Ok(Vec::new())
        }
        async fn claim_ingest_batch(
            &self,
            _limit: i64,
            _lease: Duration,
        ) -> Result<Vec<IngestJob>, DomainError> {
            unreachable!()
        }
        async fn store_article(
            &self,
            _job: &IngestJob,
            _text: &str,
            _content_hash: &str,
        ) -> Result<bool, DomainError> {
            unreachable!()
        }
        async fn record_ingest_failure(
            &self,
            _job: &IngestJob,
            _failure: &IngestFailure,
        ) -> Result<bool, DomainError> {
            unreachable!()
        }
    }

    async fn run(filter: BookmarkFilter) -> Option<SearchQuery> {
        let repo = Arc::new(RecordingRepo::default());
        SearchService::new(repo.clone())
            .search(Uuid::new_v4(), filter)
            .await
            .unwrap();
        repo.queries.lock().unwrap().pop()
    }

    #[tokio::test]
    async fn defaults_to_relevance_and_the_result_cap() {
        let q = run(BookmarkFilter {
            search: Some("  borrow checker ".into()),
            tags: Some(vec![]),
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!(q.text, "borrow checker");
        assert_eq!(q.sort, BookmarkSort::Relevance);
        assert_eq!((q.limit, q.offset), (MAX_SEARCH_RESULTS, 0));
        assert_eq!(q.tags, None);
    }

    #[tokio::test]
    async fn paging_stays_inside_the_cap() {
        let q = run(BookmarkFilter {
            search: Some("rust".into()),
            sort: Some(BookmarkSort::Title),
            limit: Some(500),
            offset: Some(40),
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!((q.limit, q.offset), (10, 40));
        assert_eq!(q.sort, BookmarkSort::Title);
        assert!(
            run(BookmarkFilter {
                search: Some("rust".into()),
                offset: Some(50),
                ..Default::default()
            })
            .await
            .is_none(),
            "past the cap there is nothing to query"
        );
    }

    #[tokio::test]
    async fn rejects_an_empty_search_and_truncates_long_ones() {
        let repo = Arc::new(RecordingRepo::default());
        let service = SearchService::new(repo.clone());
        let empty = service
            .search(
                Uuid::new_v4(),
                BookmarkFilter {
                    search: Some("   ".into()),
                    ..Default::default()
                },
            )
            .await;
        assert!(matches!(empty, Err(DomainError::InvalidInput(_))));
        let q = run(BookmarkFilter {
            search: Some("é".repeat(1000)),
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!(q.text.chars().count(), MAX_QUERY_CHARS);
    }
}
