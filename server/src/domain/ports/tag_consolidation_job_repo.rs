use crate::domain::error::DomainError;
use crate::domain::tag_alias::NewTagMergeProposal;
use crate::domain::tag_consolidation::{ConsolidationJob, JobOutcome};
use uuid::Uuid;

/// The latest consolidation run per user, so the review page can poll it.
#[trait_variant::make(Send)]
pub trait TagConsolidationJobRepository: Send + Sync {
    /// Record a new running run and return its id, unless a run is already
    /// running. A run that started more than `stale_minutes` ago counts as
    /// dead and is replaced.
    async fn start_job(
        &self,
        user_id: Uuid,
        stale_minutes: i64,
    ) -> Result<Option<Uuid>, DomainError>;

    /// Store run `run_id`'s AI proposals in place of the pending ones, but
    /// only while it is still the user's running run, in one transaction
    /// under the per-user tag lock. Returns how many were stored, or `None`
    /// when a newer run replaced this one; then nothing changes.
    async fn store_run_proposals(
        &self,
        user_id: Uuid,
        run_id: Uuid,
        proposals: &[NewTagMergeProposal],
    ) -> Result<Option<usize>, DomainError>;

    /// Record how run `run_id` ended. A run that a newer one replaced
    /// changes nothing.
    async fn finish_job(
        &self,
        user_id: Uuid,
        run_id: Uuid,
        outcome: &JobOutcome,
    ) -> Result<(), DomainError>;

    async fn get_job(&self, user_id: Uuid) -> Result<Option<ConsolidationJob>, DomainError>;
}
