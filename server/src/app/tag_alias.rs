//! Canonical tag use cases: write-path resolution, normalization proposals,
//! and applying or undoing merges.
//!
//! Existing tags are never merged automatically. `propose_normalization`
//! only stores proposals; bookmarks change when the user calls `apply_merge`
//! (review screen) or `merge_tags` (manual merge or rename on `/tags`), and
//! every such change is one batch that `undo_batch` reverts.

use crate::domain::error::DomainError;
use crate::domain::ports::llm_client::BoxFuture;
use crate::domain::ports::tag_alias_repo::TagAliasRepository;
use crate::domain::ports::tag_resolver::{ResolveFuture, TagResolver};
use crate::domain::ports::tag_vocabulary::TagVocabulary;
use crate::domain::tag_alias::{
    ChangeKind, ProposalSource, ProposalStatus, TagAlias, TagChangeBatch, TagMerge,
    TagMergeProposal, TagWrite, canonical_tag_counts, normalization_proposals, preview_tags,
};
use crate::domain::tag_consolidation::{ReviewedRow, TagSample};
use std::sync::Arc;
use uuid::Uuid;

pub struct TagAliasService<R> {
    repo: Arc<R>,
}

impl<R> TagAliasService<R>
where
    R: TagAliasRepository + Send + Sync,
{
    pub fn new(repo: Arc<R>) -> Self {
        Self { repo }
    }

    /// Resolve bookmark tag writes to canonical tags and remember any new
    /// spelling as an alias, in one pass serialized with the user's other
    /// tag changes. See [`TagResolver::resolve_tags`].
    pub async fn resolve_writes(
        &self,
        user_id: Uuid,
        writes: &[TagWrite],
    ) -> Result<Vec<Vec<String>>, DomainError> {
        self.repo.resolve_writes(user_id, writes).await
    }

    /// Each canonical tag on the user's bookmarks with its bookmark count.
    pub async fn tag_counts(&self, user_id: Uuid) -> Result<Vec<(String, i64)>, DomainError> {
        self.repo.tag_counts(user_id).await
    }

    /// The on-demand normalization job: group the user's existing tags by
    /// L1-L3 and store each group with duplicates as a pending proposal.
    /// It never changes bookmarks. Re-running replaces earlier pending
    /// deterministic proposals and skips groups the user dismissed.
    pub async fn propose_normalization(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<TagMergeProposal>, DomainError> {
        let counts = self.repo.tag_counts(user_id).await?;
        let proposals = normalization_proposals(&counts);
        self.repo
            .replace_pending_proposals(user_id, ProposalSource::Deterministic, &proposals)
            .await
    }

    /// Run [`Self::propose_normalization`] once per user: only when the user
    /// has no deterministic proposal of any status yet, so applied or
    /// dismissed groups are never proposed again. Pages that show proposals
    /// call it lazily on first visit.
    pub async fn ensure_normalization_proposals(&self, user_id: Uuid) -> Result<(), DomainError> {
        let proposals = self.repo.list_proposals(user_id, None).await?;
        if !proposals
            .iter()
            .any(|p| p.source == ProposalSource::Deterministic)
        {
            self.propose_normalization(user_id).await?;
        }
        Ok(())
    }

    /// Apply the chosen pending proposals as one undoable batch.
    pub async fn apply_merge(
        &self,
        user_id: Uuid,
        proposal_ids: &[Uuid],
    ) -> Result<TagChangeBatch, DomainError> {
        self.repo.apply_proposals(user_id, proposal_ids).await
    }

    /// Merge tags directly, without a proposal: a manual merge
    /// (`ChangeKind::Merge`) or a rename, which is a merge into a new name
    /// (`ChangeKind::Rename`). One undoable batch, or `None` when nothing
    /// would change (a retried or double-submitted request).
    pub async fn merge_tags(
        &self,
        user_id: Uuid,
        kind: ChangeKind,
        merges: &[TagMerge],
    ) -> Result<Option<TagChangeBatch>, DomainError> {
        if !matches!(kind, ChangeKind::Merge | ChangeKind::Rename) {
            return Err(DomainError::InvalidInput(format!(
                "{} is not a manual merge",
                kind.as_str()
            )));
        }
        self.repo.apply_merges(user_id, kind, merges).await
    }

    pub async fn undo_batch(
        &self,
        user_id: Uuid,
        batch_id: Uuid,
    ) -> Result<TagChangeBatch, DomainError> {
        self.repo.undo_batch(user_id, batch_id).await
    }

    pub async fn dismiss_proposals(&self, user_id: Uuid, ids: &[Uuid]) -> Result<u64, DomainError> {
        self.repo.dismiss_proposals(user_id, ids).await
    }

    /// Apply the rows checked on the review screen, exactly as shown, as
    /// one undoable batch. See [`TagAliasRepository::apply_reviewed`].
    pub async fn apply_reviewed(
        &self,
        user_id: Uuid,
        rows: &[ReviewedRow],
    ) -> Result<TagChangeBatch, DomainError> {
        self.repo.apply_reviewed(user_id, rows).await
    }

    /// Each distinct tag with its count and a sample title.
    pub async fn tag_samples(&self, user_id: Uuid) -> Result<Vec<TagSample>, DomainError> {
        self.repo.tag_samples(user_id).await
    }

    pub async fn list_proposals(
        &self,
        user_id: Uuid,
        status: Option<ProposalStatus>,
    ) -> Result<Vec<TagMergeProposal>, DomainError> {
        self.repo.list_proposals(user_id, status).await
    }

    pub async fn list_batches(
        &self,
        user_id: Uuid,
        limit: i64,
    ) -> Result<Vec<TagChangeBatch>, DomainError> {
        self.repo.list_batches(user_id, limit).await
    }

    pub async fn list_aliases(&self, user_id: Uuid) -> Result<Vec<TagAlias>, DomainError> {
        self.repo.list_aliases(user_id).await
    }
}

impl<R> TagResolver for TagAliasService<R>
where
    R: TagAliasRepository + Send + Sync + 'static,
{
    fn resolve_tags(&self, user_id: Uuid, writes: Vec<TagWrite>) -> ResolveFuture<'_> {
        Box::pin(async move { self.resolve_writes(user_id, &writes).await })
    }
}

impl<R> TagVocabulary for TagAliasService<R>
where
    R: TagAliasRepository + Send + Sync + 'static,
{
    fn canonical_tags(
        &self,
        user_id: Uuid,
    ) -> BoxFuture<'_, Result<Vec<(String, i64)>, DomainError>> {
        Box::pin(async move { Ok(canonical_tag_counts(&self.repo.tag_counts(user_id).await?)) })
    }

    fn preview_tags(
        &self,
        user_id: Uuid,
        tags: Vec<String>,
    ) -> BoxFuture<'_, Result<Vec<String>, DomainError>> {
        Box::pin(async move {
            let aliases = self.repo.list_aliases(user_id).await?;
            let counts = self.repo.tag_counts(user_id).await?;
            Ok(preview_tags(&tags, &aliases, &counts))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::tag_alias::{Confidence, NewTagMergeProposal, TagChangeSummary};
    use std::sync::Mutex;

    /// In-memory stand-in for the repository: aliases, tag counts and the
    /// proposals the service stores. Resolution, merge and undo live in the
    /// repository; the Postgres test covers them.
    #[derive(Default)]
    struct StubRepo {
        aliases: Mutex<Vec<TagAlias>>,
        counts: Vec<(String, i64)>,
        proposals: Mutex<Vec<(ProposalSource, NewTagMergeProposal)>>,
        merged: Mutex<Vec<(ChangeKind, Vec<TagMerge>)>>,
    }

    fn batch(kind: ChangeKind) -> TagChangeBatch {
        TagChangeBatch {
            id: Uuid::nil(),
            user_id: Uuid::nil(),
            kind,
            summary: TagChangeSummary::default(),
            undone_at: None,
            created_at: chrono::Utc::now(),
        }
    }

    impl TagAliasRepository for StubRepo {
        async fn list_aliases(&self, _user_id: Uuid) -> Result<Vec<TagAlias>, DomainError> {
            Ok(self.aliases.lock().unwrap().clone())
        }

        async fn tag_counts(&self, _user_id: Uuid) -> Result<Vec<(String, i64)>, DomainError> {
            Ok(self.counts.clone())
        }

        async fn tag_samples(&self, _user_id: Uuid) -> Result<Vec<TagSample>, DomainError> {
            unimplemented!("not used by these tests")
        }

        async fn resolve_writes(
            &self,
            _user_id: Uuid,
            writes: &[TagWrite],
        ) -> Result<Vec<Vec<String>>, DomainError> {
            Ok(writes.iter().map(|w| w.tags.clone()).collect())
        }

        async fn replace_pending_proposals(
            &self,
            _user_id: Uuid,
            source: ProposalSource,
            proposals: &[NewTagMergeProposal],
        ) -> Result<Vec<TagMergeProposal>, DomainError> {
            let mut stored = self.proposals.lock().unwrap();
            stored.retain(|(s, _)| *s != source);
            stored.extend(proposals.iter().cloned().map(|p| (source, p)));
            Ok(proposals
                .iter()
                .map(|p| TagMergeProposal {
                    id: Uuid::new_v4(),
                    user_id: Uuid::nil(),
                    canonical: p.canonical.clone(),
                    members: p.members.clone(),
                    source,
                    confidence: p.confidence,
                    status: ProposalStatus::Pending,
                    batch_id: None,
                    created_at: chrono::Utc::now(),
                    decided_at: None,
                })
                .collect())
        }

        async fn list_proposals(
            &self,
            _user_id: Uuid,
            _status: Option<ProposalStatus>,
        ) -> Result<Vec<TagMergeProposal>, DomainError> {
            Ok(Vec::new())
        }

        async fn apply_reviewed(
            &self,
            _user_id: Uuid,
            _rows: &[ReviewedRow],
        ) -> Result<TagChangeBatch, DomainError> {
            unimplemented!("not used by these tests")
        }

        async fn dismiss_proposals(
            &self,
            _user_id: Uuid,
            _ids: &[Uuid],
        ) -> Result<u64, DomainError> {
            Ok(0)
        }

        async fn apply_proposals(
            &self,
            _user_id: Uuid,
            _ids: &[Uuid],
        ) -> Result<TagChangeBatch, DomainError> {
            Ok(batch(ChangeKind::Normalize))
        }

        async fn apply_merges(
            &self,
            _user_id: Uuid,
            kind: ChangeKind,
            merges: &[TagMerge],
        ) -> Result<Option<TagChangeBatch>, DomainError> {
            self.merged.lock().unwrap().push((kind, merges.to_vec()));
            Ok(Some(batch(kind)))
        }

        async fn undo_batch(
            &self,
            _user_id: Uuid,
            _batch_id: Uuid,
        ) -> Result<TagChangeBatch, DomainError> {
            Ok(batch(ChangeKind::Merge))
        }

        async fn list_batches(
            &self,
            _user_id: Uuid,
            _limit: i64,
        ) -> Result<Vec<TagChangeBatch>, DomainError> {
            Ok(Vec::new())
        }
    }

    fn s(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_string()).collect()
    }

    fn counts(values: &[(&str, i64)]) -> Vec<(String, i64)> {
        values.iter().map(|(t, c)| ((*t).to_string(), *c)).collect()
    }

    fn service(repo: StubRepo) -> (TagAliasService<StubRepo>, Arc<StubRepo>) {
        let repo = Arc::new(repo);
        (TagAliasService::new(repo.clone()), repo)
    }

    #[tokio::test]
    async fn propose_normalization_only_stores_proposals() {
        let (svc, repo) = service(StubRepo {
            counts: counts(&[
                ("ai coding agents", 3),
                ("AI coding agents", 2),
                ("coding-agent", 2),
                ("coding agents", 4),
                ("github", 3),
                ("GitHub", 9),
                ("rust", 5),
                ("Rust", 1),
                ("software engineering", 2),
                ("software-engineering", 1),
                ("local ai", 1),
                ("local-AI", 1),
                ("UI/UX", 2),
                ("ux", 1),
            ]),
            ..Default::default()
        });
        let proposals = svc.propose_normalization(Uuid::new_v4()).await.unwrap();
        let groups: Vec<(&str, Vec<&str>)> = proposals
            .iter()
            .map(|p| {
                (
                    p.canonical.as_str(),
                    p.members.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            groups,
            vec![
                ("GitHub", vec!["GitHub", "github"]),
                (
                    "ai coding agents",
                    vec!["AI coding agents", "ai coding agents"]
                ),
                ("coding agents", vec!["coding agents", "coding-agent"]),
                ("local ai", vec!["local ai", "local-AI"]),
                ("rust", vec!["Rust", "rust"]),
                (
                    "software engineering",
                    vec!["software engineering", "software-engineering"]
                ),
            ]
        );
        assert!(proposals.iter().all(|p| p.confidence == Confidence::High
            && p.status == ProposalStatus::Pending
            && p.source == ProposalSource::Deterministic));
        assert!(repo.merged.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn merge_tags_accepts_only_manual_kinds() {
        let (svc, repo) = service(StubRepo::default());
        let merges = vec![TagMerge {
            canonical: "GitHub".into(),
            members: s(&["github"]),
        }];
        let user = Uuid::new_v4();
        assert!(
            svc.merge_tags(user, ChangeKind::Merge, &merges)
                .await
                .is_ok()
        );
        assert!(
            svc.merge_tags(user, ChangeKind::Rename, &merges)
                .await
                .is_ok()
        );
        assert!(matches!(
            svc.merge_tags(user, ChangeKind::Normalize, &merges).await,
            Err(DomainError::InvalidInput(_))
        ));
        assert_eq!(repo.merged.lock().unwrap().len(), 2);
    }
}
