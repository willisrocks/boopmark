use crate::domain::error::DomainError;
use crate::domain::tag_alias::{
    ChangeKind, NewTagMergeProposal, ProposalSource, ProposalStatus, TagAlias, TagChangeBatch,
    TagMerge, TagMergeProposal, TagWrite,
};
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

    /// Apply merges that did not come from proposals (manual merge, rename).
    /// Returns `None`, recording no batch, when nothing would change, so a
    /// retried or double-submitted merge does not add an empty batch.
    async fn apply_merges(
        &self,
        user_id: Uuid,
        kind: ChangeKind,
        merges: &[TagMerge],
    ) -> Result<Option<TagChangeBatch>, DomainError>;

    /// Undo an applied batch. Fails with `InvalidInput` if it is already
    /// undone or a later, still-applied batch touched the same bookmarks or
    /// aliases.
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
