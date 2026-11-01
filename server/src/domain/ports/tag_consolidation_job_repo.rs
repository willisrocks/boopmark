use crate::domain::error::DomainError;
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
