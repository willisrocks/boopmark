use crate::domain::bookmark::{BookmarkFilter, BookmarkSort};
use crate::domain::error::DomainError;
use crate::domain::ports::embedder::{EmbedKind, Embedder};
use crate::domain::ports::search_repo::{SearchIndexRepository, SearchRepository};
use crate::domain::search::{
    MAX_SEARCH_RESULTS, QueryVector, SearchHit, SearchIndexStatus, SearchQuery, semantic_query_text,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Longer queries are cut here. The legs do not get better with more words,
/// and trigram matching cost grows with the query.
const MAX_QUERY_CHARS: usize = 256;
/// The query embedding is on the request path: past this, whatever the
/// backend (a hosted call, or a local model busy with a backfill), search
/// runs the lexical legs alone.
const QUERY_EMBED_TIMEOUT: Duration = Duration::from_millis(1500);
/// A provider outage would warn on every search; warn once per interval and
/// log the rest at debug.
const FALLBACK_WARN_INTERVAL: Duration = Duration::from_secs(60);

pub struct SearchService<R> {
    repo: Arc<R>,
    embedder: Arc<dyn Embedder>,
    last_fallback_warning: Mutex<Option<Instant>>,
    /// Why the vector schema could not be created at startup, if it could not.
    vector_unavailable: Option<String>,
}

impl<R: SearchRepository> SearchService<R> {
    pub fn new(repo: Arc<R>, embedder: Arc<dyn Embedder>) -> Self {
        Self {
            repo,
            embedder,
            last_fallback_warning: Mutex::new(None),
            vector_unavailable: None,
        }
    }

    /// Shown in the owner line of Settings → Search index.
    pub fn vector_unavailable(mut self, reason: Option<String>) -> Self {
        self.vector_unavailable = reason;
        self
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
        let vector = self.query_vector(&text).await;
        let mut query = SearchQuery {
            text,
            tags: filter.tags.filter(|tags| !tags.is_empty()),
            sort: filter.sort.unwrap_or(BookmarkSort::Relevance),
            limit,
            offset,
            vector,
        };
        match self.repo.search(user_id, &query).await {
            // The vector leg needs pgvector >= 0.8; whatever goes wrong with
            // it, the lexical legs still answer.
            Err(e) if query.vector.is_some() => {
                self.warn_fallback(&format!("the vector leg failed: {e}"));
                query.vector = None;
                self.repo.search(user_id, &query).await
            }
            result => result,
        }
    }

    /// The query's vector for the vector leg, or `None` to search
    /// lexically: embeddings disabled, nothing to embed, a timeout, an error
    /// or an unusable answer.
    async fn query_vector(&self, text: &str) -> Option<QueryVector> {
        if !self.embedder.enabled() {
            return None;
        }
        let text = semantic_query_text(text);
        if text.is_empty() {
            return None;
        }
        let embedded = tokio::time::timeout(
            QUERY_EMBED_TIMEOUT,
            self.embedder.embed(vec![text], EmbedKind::Query),
        )
        .await;
        let reason = match embedded {
            Ok(Ok(mut embeddings)) => match embeddings.vectors.pop() {
                Some(values)
                    if embeddings.vectors.is_empty() && values.len() == self.embedder.dims() =>
                {
                    return Some(QueryVector {
                        model_id: self.embedder.model_id().to_string(),
                        values,
                    });
                }
                _ => "the embedder returned no usable vector".to_string(),
            },
            Ok(Err(e)) => e.to_string(),
            Err(_) => format!("timed out after {QUERY_EMBED_TIMEOUT:?}"),
        };
        self.warn_fallback(&reason);
        None
    }

    fn warn_fallback(&self, reason: &str) {
        let now = Instant::now();
        let mut last = self.last_fallback_warning.lock().unwrap();
        if last.is_none_or(|at| now.duration_since(at) >= FALLBACK_WARN_INTERVAL) {
            *last = Some(now);
            tracing::warn!("search: query embedding failed ({reason}); lexical legs only");
        } else {
            tracing::debug!("search: query embedding failed ({reason}); lexical legs only");
        }
    }
}

/// Settings → Search index.
impl<R: SearchRepository + SearchIndexRepository> SearchService<R> {
    pub async fn index_status(&self, user_id: Uuid) -> Result<SearchIndexStatus, DomainError> {
        let model_id = self.embedder.enabled().then(|| self.embedder.model_id());
        self.repo.index_status(user_id, model_id).await
    }

    /// Queue the user's failed and skipped fetches again; returns how many.
    pub async fn retry_failed_fetches(&self, user_id: Uuid) -> Result<u64, DomainError> {
        self.repo.retry_failed_fetches(user_id).await
    }

    /// Fetch one bookmark's article again.
    pub async fn refetch(&self, user_id: Uuid, bookmark_id: Uuid) -> Result<(), DomainError> {
        if self.repo.refetch(user_id, bookmark_id).await? {
            Ok(())
        } else {
            Err(DomainError::NotFound)
        }
    }

    /// One line of deploy evidence for the owner: the embedder's `model_id`
    /// and the installed versions of the search extensions.
    pub async fn deployment_line(&self) -> Result<String, DomainError> {
        let versions = self.repo.extension_versions().await?;
        let version = |name: &str| {
            versions
                .iter()
                .find(|(ext, _)| ext == name)
                .map_or("not installed", |(_, v)| v.as_str())
                .to_string()
        };
        let line = format!(
            "{} · vector {} · pg_trgm {}",
            self.embedder.model_id(),
            version("vector"),
            version("pg_trgm")
        );
        Ok(match &self.vector_unavailable {
            Some(reason) => format!("vector search unavailable: {reason} · {line}"),
            None => line,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::embedding::tests::{DisabledStub, StubEmbedder};
    use crate::domain::search::{ContentSource, IngestFailure, IngestJob};

    #[derive(Default)]
    struct RecordingRepo {
        queries: Mutex<Vec<SearchQuery>>,
        /// Fail every search that has a vector leg.
        fail_vector: bool,
    }

    impl SearchRepository for RecordingRepo {
        async fn search(
            &self,
            _user_id: Uuid,
            query: &SearchQuery,
        ) -> Result<Vec<SearchHit>, DomainError> {
            self.queries.lock().unwrap().push(query.clone());
            if self.fail_vector && query.vector.is_some() {
                return Err(DomainError::Internal("unrecognized parameter".into()));
            }
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
            _source: ContentSource,
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
        async fn store_capture(
            &self,
            _bookmark_id: Uuid,
            _user_id: Uuid,
            _url: &str,
            _text: &str,
            _content_hash: &str,
        ) -> Result<bool, DomainError> {
            unreachable!()
        }
    }

    async fn run(filter: BookmarkFilter) -> Option<SearchQuery> {
        let repo = Arc::new(RecordingRepo::default());
        SearchService::new(repo.clone(), Arc::new(DisabledStub))
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
        let service = SearchService::new(repo.clone(), Arc::new(DisabledStub));
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

    async fn run_with(embedder: Arc<StubEmbedder>, text: &str) -> SearchQuery {
        let repo = Arc::new(RecordingRepo::default());
        SearchService::new(repo.clone(), embedder)
            .search(
                Uuid::new_v4(),
                BookmarkFilter {
                    search: Some(text.into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        repo.queries.lock().unwrap().pop().unwrap()
    }

    #[tokio::test]
    async fn embeds_the_query_without_its_exclusions() {
        let embedder = Arc::new(StubEmbedder::new(8));
        let q = run_with(embedder.clone(), "coffee -tea").await;
        let vector = q.vector.expect("the vector leg runs");
        assert_eq!(vector.model_id, "stub:hash@8");
        assert_eq!(vector.values, embedder.vector("coffee"));
        assert_eq!(
            q.text, "coffee -tea",
            "the lexical legs see the whole query"
        );
        let calls = embedder.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            (calls[0].0.clone(), calls[0].1),
            (vec!["coffee".to_string()], EmbedKind::Query)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_query_embedding_falls_back_to_the_lexical_legs() {
        let embedder = Arc::new(StubEmbedder {
            delay: Duration::from_secs(10),
            ..StubEmbedder::new(8)
        });
        let started = tokio::time::Instant::now();
        let q = run_with(embedder, "borrow checker").await;
        assert_eq!(q.vector, None);
        assert_eq!(started.elapsed(), QUERY_EMBED_TIMEOUT);
    }

    #[tokio::test]
    async fn a_failing_vector_leg_falls_back_to_the_lexical_legs() {
        let repo = Arc::new(RecordingRepo {
            fail_vector: true,
            ..Default::default()
        });
        let hits = SearchService::new(repo.clone(), Arc::new(StubEmbedder::new(8)))
            .search(
                Uuid::new_v4(),
                BookmarkFilter {
                    search: Some("rust".into()),
                    ..Default::default()
                },
            )
            .await;
        assert!(hits.is_ok());
        let queries = repo.queries.lock().unwrap();
        assert_eq!(queries.len(), 2);
        assert!(queries[0].vector.is_some() && queries[1].vector.is_none());
    }

    #[tokio::test]
    async fn embedding_errors_and_empty_queries_skip_the_vector_leg() {
        let embedder = Arc::new(StubEmbedder::new(8));
        assert_eq!(
            run_with(embedder.clone(), "fail: outage").await.vector,
            None
        );
        assert_eq!(run_with(embedder.clone(), "-tea").await.vector, None);
        assert_eq!(
            embedder.calls.lock().unwrap().len(),
            1,
            "nothing to embed in -tea"
        );
        // A disabled embedder is never called (it would panic).
        assert_eq!(
            run(BookmarkFilter {
                search: Some("rust".into()),
                ..Default::default()
            })
            .await
            .unwrap()
            .vector,
            None
        );
    }
}
