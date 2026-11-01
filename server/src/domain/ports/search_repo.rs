use crate::domain::error::DomainError;
use crate::domain::search::{IngestFailure, IngestJob, SearchHit, SearchQuery};
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
    /// row fetched. Returns false when the bookmark was deleted or its URL
    /// changed since the claim; the result is then simply dropped.
    async fn store_article(
        &self,
        job: &IngestJob,
        text: &str,
        content_hash: &str,
    ) -> Result<bool, DomainError>;

    /// Record a failed attempt; see `IngestFailure`. Returns false like
    /// `store_article`.
    async fn record_ingest_failure(
        &self,
        job: &IngestJob,
        failure: &IngestFailure,
    ) -> Result<bool, DomainError>;
}
