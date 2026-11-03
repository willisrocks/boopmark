use crate::domain::error::DomainError;
use crate::domain::search::{
    ContentSource, IngestFailure, IngestJob, SearchHit, SearchIndexStatus, SearchQuery,
};
use std::time::Duration;
use uuid::Uuid;

#[trait_variant::make(Send)]
pub trait SearchRepository: Send + Sync {
    /// Relevance-ranked search over the user's bookmarks, at most
    /// `MAX_SEARCH_RESULTS` fused results before `limit`/`offset` apply.
    async fn search(
        &self,
        user_id: Uuid,
        query: &SearchQuery,
    ) -> Result<Vec<SearchHit>, DomainError>;

    /// Claim up to `limit` due rows of the ingest queue. Claimed rows are
    /// leased: they are not handed out again until `lease` has passed, so a
    /// worker that dies mid-fetch does not lose them.
    async fn claim_ingest_batch(
        &self,
        limit: i64,
        lease: Duration,
    ) -> Result<Vec<IngestJob>, DomainError>;

    /// Store the extracted text, rebuild the lexical document and mark the
    /// row fetched, recording where the text came from. Returns false when
    /// the bookmark was deleted or its URL changed since the claim; the
    /// result is then simply dropped.
    async fn store_article(
        &self,
        job: &IngestJob,
        text: &str,
        content_hash: &str,
        source: ContentSource,
    ) -> Result<bool, DomainError>;

    /// Record a failed attempt; see `IngestFailure`. Returns false like
    /// `store_article`.
    async fn record_ingest_failure(
        &self,
        job: &IngestJob,
        failure: &IngestFailure,
    ) -> Result<bool, DomainError>;

    /// Store text captured from the browser that displayed the page: the row
    /// is `fetched` with `source = 'captured'`, and any fetch already in
    /// flight loses its claim, so its result is dropped. Returns false when
    /// the bookmark was deleted or is not this user's.
    async fn store_capture(
        &self,
        bookmark_id: Uuid,
        user_id: Uuid,
        text: &str,
        content_hash: &str,
    ) -> Result<bool, DomainError>;
}

/// The user-facing state of the search index (Settings → Search index).
#[trait_variant::make(Send)]
pub trait SearchIndexRepository: Send + Sync {
    /// Counts of the user's bookmarks by ingest state, and how many are
    /// embedded by `model_id` (when embeddings are on).
    async fn index_status(
        &self,
        user_id: Uuid,
        model_id: Option<&str>,
    ) -> Result<SearchIndexStatus, DomainError>;

    /// Queue every failed or skipped fetch of the user again. Returns how
    /// many were queued.
    async fn retry_failed_fetches(&self, user_id: Uuid) -> Result<u64, DomainError>;

    /// Queue one of the user's bookmarks for a new fetch, whatever its
    /// state. Returns false when the user has no such bookmark.
    async fn refetch(&self, user_id: Uuid, bookmark_id: Uuid) -> Result<bool, DomainError>;

    /// `(name, version)` of the installed search extensions (`pg_trgm`,
    /// `vector`), for the owner's deploy check.
    async fn extension_versions(&self) -> Result<Vec<(String, String)>, DomainError>;
}
