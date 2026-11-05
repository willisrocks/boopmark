use crate::domain::error::DomainError;
use crate::domain::tag_alias::{
    NewTagMergeProposal, ProposalSource, ProposalStatus, TagAlias, TagChangeBatch,
    TagMergeProposal, TagWrite,
};
use crate::domain::tag_consolidation::{ReviewedRow, TagSample};
use crate::domain::tag_library::{ManualMerge, ManualMergePlan};
use uuid::Uuid;

/// Storage for aliases, merge proposals and undoable change batches.
///
/// Every method is scoped to `user_id`. Methods that write tags or aliases
/// (resolve, apply, undo) are serialized per user, and the merge and undo
/// methods each run in one transaction, so a batch is applied or undone
/// completely or not at all.
#[trait_variant::make(Send)]
pub trait TagAliasRepository: Send + Sync {
    async fn list_aliases(&self, user_id: Uuid) -> Result<Vec<TagAlias>, DomainError>;

    /// Each distinct tag on the user's bookmarks with its bookmark count.
    async fn tag_counts(&self, user_id: Uuid) -> Result<Vec<(String, i64)>, DomainError>;

    /// Each distinct tag with its bookmark count and the title of its most
    /// recent titled bookmark, for consolidation and the review screen.
    async fn tag_samples(&self, user_id: Uuid) -> Result<Vec<TagSample>, DomainError>;

    /// Resolve bookmark tag writes to canonical tags with
    /// [`crate::domain::tag_alias::resolve_writes`] and remember the new
    /// aliases, in one transaction serialized with every other tag change
    /// of this user. Returns the resolved tags per write, in order.
    async fn resolve_writes(
        &self,
        user_id: Uuid,
        writes: &[TagWrite],
    ) -> Result<Vec<Vec<String>>, DomainError>;

    /// Replace the user's pending proposals from `source` with `proposals`.
    /// A proposal whose member set the user already dismissed is skipped.
    /// Applied and dismissed proposals are kept.
    async fn replace_pending_proposals(
        &self,
        user_id: Uuid,
        source: ProposalSource,
        proposals: &[NewTagMergeProposal],
    ) -> Result<Vec<TagMergeProposal>, DomainError>;

    /// The user's proposals, newest first, optionally filtered by status.
    async fn list_proposals(
        &self,
        user_id: Uuid,
        status: Option<ProposalStatus>,
    ) -> Result<Vec<TagMergeProposal>, DomainError>;

    /// Mark pending proposals dismissed. Returns how many changed.
    async fn dismiss_proposals(&self, user_id: Uuid, ids: &[Uuid]) -> Result<u64, DomainError>;

    /// Apply pending proposals as one batch (`normalize`, or `llm_merge` if
    /// any came from the LLM). Fails without changes if any id is unknown or
    /// not pending.
    async fn apply_proposals(
        &self,
        user_id: Uuid,
        ids: &[Uuid],
    ) -> Result<TagChangeBatch, DomainError>;

    /// Apply the rows checked on the review screen as one batch, in one
    /// transaction under the per-user lock: the proposals pending now are
    /// resolved through the current aliases, every row must still match
    /// what the page showed ([`crate::domain::tag_consolidation::plan_reviewed_apply`]),
    /// and edited targets are saved with the apply. Fails without changes
    /// otherwise.
    async fn apply_reviewed(
        &self,
        user_id: Uuid,
        rows: &[ReviewedRow],
    ) -> Result<TagChangeBatch, DomainError>;

    /// Apply a manual merge or rename, which did not come from a proposal,
    /// in one transaction under the per-user lock: the request is checked
    /// against the tags and aliases as they are under the lock
    /// ([`crate::domain::tag_library::plan_manual_merge`]), so a request
    /// from a stale page, or a retried or double-submitted one, is refused.
    /// Returns the plan, and `None` instead of a batch when nothing would
    /// change, so no empty batch is recorded.
    async fn apply_manual_merge(
        &self,
        user_id: Uuid,
        request: &ManualMerge,
    ) -> Result<(ManualMergePlan, Option<TagChangeBatch>), DomainError>;

    /// Delete a tag no bookmark uses, with its alias rows, as one
    /// `ChangeKind::Delete` batch that `undo_batch` reverts. Checked under
    /// the per-user lock ([`crate::domain::tag_library::plan_tag_delete`]).
    async fn delete_unused_tag(
        &self,
        user_id: Uuid,
        name: &str,
    ) -> Result<TagChangeBatch, DomainError>;

    /// Undo an applied batch. Fails with `InvalidInput` if it is already
    /// undone, is older than the undo window
    /// ([`crate::domain::tag_alias::UNDO_WINDOW_DAYS`]), or a later,
    /// still-applied batch touched the same bookmarks or aliases.
    async fn undo_batch(
        &self,
        user_id: Uuid,
        batch_id: Uuid,
    ) -> Result<TagChangeBatch, DomainError>;

    /// The user's batches, newest first.
    async fn list_batches(
        &self,
        user_id: Uuid,
        limit: i64,
    ) -> Result<Vec<TagChangeBatch>, DomainError>;
}
