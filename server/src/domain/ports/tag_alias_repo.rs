use crate::domain::error::DomainError;
use crate::domain::tag_alias::{
    ChangeKind, NewTagAlias, NewTagMergeProposal, ProposalSource, ProposalStatus, TagAlias,
    TagChangeBatch, TagMerge, TagMergeProposal,
};
use uuid::Uuid;

/// Storage for aliases, merge proposals and undoable change batches.
///
/// Every method is scoped to `user_id`. The merge and undo methods each run
/// in one transaction with the affected bookmark rows locked, so a batch is
/// applied or undone completely or not at all.
#[trait_variant::make(Send)]
pub trait TagAliasRepository: Send + Sync {
    async fn list_aliases(&self, user_id: Uuid) -> Result<Vec<TagAlias>, DomainError>;

    /// Each distinct tag on the user's bookmarks with its bookmark count.
    async fn tag_counts(&self, user_id: Uuid) -> Result<Vec<(String, i64)>, DomainError>;

    /// Insert aliases whose key is not known yet, and return the stored row
    /// for every requested key. A key a concurrent writer inserted first
    /// keeps that writer's canonical.
    async fn insert_aliases_if_absent(
        &self,
        user_id: Uuid,
        aliases: &[NewTagAlias],
    ) -> Result<Vec<TagAlias>, DomainError>;

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
    async fn apply_merges(
        &self,
        user_id: Uuid,
        kind: ChangeKind,
        merges: &[TagMerge],
    ) -> Result<TagChangeBatch, DomainError>;

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
