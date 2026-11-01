//! Reviewed LLM consolidation (tag spec §3): a background run that stores
//! merge proposals, and the review screen's use cases.
//!
//! Nothing here changes a tag on its own. A run only writes proposals;
//! bookmarks change when the user applies the rows they checked, as one
//! batch that `undo` reverts.

use crate::app::settings::SettingsService;
use crate::app::tag_alias::TagAliasService;
use crate::app::tag_llm::{MergeSearch, find_merges};
use crate::domain::error::DomainError;
use crate::domain::ports::llm_client::LlmClient;
use crate::domain::ports::llm_settings_repo::LlmSettingsRepository;
use crate::domain::ports::tag_alias_repo::TagAliasRepository;
use crate::domain::ports::tag_consolidation_job_repo::TagConsolidationJobRepository;
use crate::domain::tag_alias::{ChangeKind, ProposalStatus, TagChangeBatch, TagMergeProposal};
use crate::domain::tag_consolidation::{
    ConsolidationJob, JobOutcome, JobStatus, ReviewRow, ReviewedRow, STALE_RUN_MINUTES,
    resolve_through_aliases, review_rows, tag_units,
};
use std::collections::HashSet;
use std::sync::Arc;
use uuid::Uuid;

/// Fewer distinct tags than this leave nothing worth asking a model about.
pub const MIN_TAGS_FOR_CONSOLIDATION: usize = 2;
/// Batches listed under the review screen's Undo.
const RECENT_BATCHES: i64 = 5;

pub const NO_PROVIDER: &str = "Add an AI provider key and enable AI in Settings first.";
const REJECTED: &str =
    "Your AI provider rejected the request. Check the API key and model in Settings.";
const UNAVAILABLE: &str = "The AI provider did not answer. Try again in a minute.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartOutcome {
    Started,
    AlreadyRunning,
}

/// What the review screen shows.
pub struct Review {
    pub rows: Vec<ReviewRow>,
    pub job: Option<ConsolidationJob>,
    /// Recent cleanups the user can undo here (normalize and LLM merges).
    pub batches: Vec<TagChangeBatch>,
}

pub struct TagConsolidationService<R, J, S> {
    tags: Arc<TagAliasService<R>>,
    jobs: Arc<J>,
    settings: Arc<SettingsService<S>>,
    llm: Arc<dyn LlmClient>,
}

impl<R, J, S> TagConsolidationService<R, J, S>
where
    R: TagAliasRepository + Send + Sync + 'static,
    J: TagConsolidationJobRepository + Send + Sync + 'static,
    S: LlmSettingsRepository + Send + Sync + 'static,
{
    pub fn new(
        tags: Arc<TagAliasService<R>>,
        jobs: Arc<J>,
        settings: Arc<SettingsService<S>>,
        llm: Arc<dyn LlmClient>,
    ) -> Self {
        Self {
            tags,
            jobs,
            settings,
            llm,
        }
    }

    /// Start a run in the background unless one is running. Fails with
    /// `InvalidInput` (a message for the user) when no provider is
    /// configured or the library is too small.
    pub async fn start(self: &Arc<Self>, user_id: Uuid) -> Result<StartOutcome, DomainError> {
        if self
            .settings
            .get_text_provider_credentials(user_id)
            .await?
            .is_none()
        {
            return Err(DomainError::InvalidInput(NO_PROVIDER.into()));
        }
        let distinct = self.tags.tag_samples(user_id).await?.len();
        if distinct < MIN_TAGS_FOR_CONSOLIDATION {
            return Err(DomainError::InvalidInput(
                "Add a few more tags before looking for duplicates.".into(),
            ));
        }
        let Some(run_id) = self.jobs.start_job(user_id, STALE_RUN_MINUTES).await? else {
            return Ok(StartOutcome::AlreadyRunning);
        };
        let service = Arc::clone(self);
        tokio::spawn(async move { service.run(user_id, run_id).await });
        Ok(StartOutcome::Started)
    }

    /// One run: refresh the deterministic proposals, ask the model, store
    /// its proposals, and record the outcome on the job row. A run that a
    /// newer one replaced stores and records nothing.
    pub async fn run(&self, user_id: Uuid, run_id: Uuid) {
        let outcome = match self.search(user_id, run_id).await {
            Ok(None) => {
                tracing::warn!(user_id = %user_id, "tag consolidation run was replaced; its proposals are dropped");
                return;
            }
            Ok(Some((search, stored))) => JobOutcome {
                status: JobStatus::Succeeded,
                error: None,
                proposals: i32::try_from(stored).unwrap_or(i32::MAX),
                pairs_checked: i32::try_from(search.pairs_checked).unwrap_or(i32::MAX),
                input_tokens: i64::from(search.usage.input_tokens),
                output_tokens: i64::from(search.usage.output_tokens),
            },
            Err(error) => {
                // Errors carry the vendor and status, never a key or prompt.
                tracing::warn!(user_id = %user_id, error = %error, "tag consolidation run failed");
                let message = match error {
                    DomainError::InvalidInput(message) if message == NO_PROVIDER => message,
                    DomainError::InvalidInput(_) => REJECTED.into(),
                    _ => UNAVAILABLE.into(),
                };
                JobOutcome {
                    status: JobStatus::Failed,
                    error: Some(message),
                    proposals: 0,
                    pairs_checked: 0,
                    input_tokens: 0,
                    output_tokens: 0,
                }
            }
        };
        if let Err(error) = self.jobs.finish_job(user_id, run_id, &outcome).await {
            tracing::error!(user_id = %user_id, error = %error, "failed to record tag consolidation outcome");
        }
    }

    /// The run's work; `None` when a newer run replaced it before it could
    /// store its proposals.
    async fn search(
        &self,
        user_id: Uuid,
        run_id: Uuid,
    ) -> Result<Option<(MergeSearch, usize)>, DomainError> {
        let credentials = self
            .settings
            .get_text_provider_credentials(user_id)
            .await?
            .ok_or_else(|| DomainError::InvalidInput(NO_PROVIDER.into()))?;
        // The LLM rows build on the spelling-variant rows, so refresh them.
        self.tags.propose_normalization(user_id).await?;
        let units = tag_units(&self.tags.tag_samples(user_id).await?);
        let search = find_merges(self.llm.as_ref(), &credentials, &units).await?;
        tracing::info!(
            user_id = %user_id,
            units = units.len(),
            whole_list_groups = search.whole_list_groups,
            whole_list_attempts = search.whole_list_attempts,
            pairs = search.pairs_checked,
            echo_mismatches = search.echo_mismatches,
            failed_batches = search.failed_batches,
            proposals = search.proposals.len(),
            input_tokens = search.usage.input_tokens,
            output_tokens = search.usage.output_tokens,
            "tag consolidation run finished"
        );
        Ok(self
            .jobs
            .store_run_proposals(user_id, run_id, &search.proposals)
            .await?
            .map(|stored| (search, stored)))
    }

    pub async fn job(&self, user_id: Uuid) -> Result<Option<ConsolidationJob>, DomainError> {
        Ok(self
            .jobs
            .get_job(user_id)
            .await?
            .map(|job| job.settle(chrono::Utc::now())))
    }

    pub async fn review(&self, user_id: Uuid) -> Result<Review, DomainError> {
        let pending = self
            .tags
            .list_proposals(user_id, Some(ProposalStatus::Pending))
            .await?;
        let samples = self.tags.tag_samples(user_id).await?;
        let aliases = self.tags.list_aliases(user_id).await?;
        let on_bookmarks: HashSet<String> = samples.iter().map(|s| s.tag.clone()).collect();
        let pending = resolve_through_aliases(&pending, &aliases, &on_bookmarks);
        let batches = self
            .tags
            .list_batches(user_id, RECENT_BATCHES * 4)
            .await?
            .into_iter()
            .filter(|b| matches!(b.kind, ChangeKind::Normalize | ChangeKind::LlmMerge))
            .take(RECENT_BATCHES as usize)
            .collect();
        Ok(Review {
            rows: review_rows(&pending, &samples),
            job: self.job(user_id).await?,
            batches,
        })
    }

    /// The on-demand spelling-variant job (no LLM).
    pub async fn propose_normalization(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<TagMergeProposal>, DomainError> {
        self.tags.propose_normalization(user_id).await
    }

    /// Apply the rows the user checked, exactly as the page showed them, as
    /// one undoable batch; see [`TagAliasService::apply_reviewed`].
    pub async fn apply(
        &self,
        user_id: Uuid,
        rows: &[ReviewedRow],
    ) -> Result<TagChangeBatch, DomainError> {
        self.tags.apply_reviewed(user_id, rows).await
    }

    pub async fn dismiss(&self, user_id: Uuid, ids: &[Uuid]) -> Result<u64, DomainError> {
        self.tags.dismiss_proposals(user_id, ids).await
    }

    pub async fn undo(&self, user_id: Uuid, batch_id: Uuid) -> Result<TagChangeBatch, DomainError> {
        self.tags.undo_batch(user_id, batch_id).await
    }
}
