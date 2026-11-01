use super::{PostgresPool, db_err, lock_user_tags};
use crate::domain::error::DomainError;
use crate::domain::ports::tag_alias_repo::TagAliasRepository;
use crate::domain::tag_alias::{
    AliasSource, ChangeKind, Confidence, NewTagMergeProposal, ProposalSource, ProposalStatus,
    TagAlias, TagChangeBatch, TagChangeSummary, TagIndex, TagMerge, TagMergeProposal, TagWrite,
    merge_mapping, resolve_writes, rewrite_tags, undo_tags,
};
use crate::domain::tag_consolidation::{
    ReviewedRow, TagSample, plan_reviewed_apply, resolve_through_aliases,
};
use crate::domain::tag_library::{ManualMerge, ManualMergePlan, library_rows, plan_manual_merge};
use crate::domain::tag_normalize::{clean_tag, compact_key};
use chrono::{DateTime, Utc};
use sqlx::{PgExecutor, Postgres, Transaction};
use std::collections::{BTreeMap, HashMap, HashSet};
use uuid::Uuid;

type AliasRow = (String, String, String, String, Option<Uuid>);

fn alias_from_row(
    (alias_key, alias, canonical, source, batch_id): AliasRow,
) -> Result<TagAlias, DomainError> {
    Ok(TagAlias {
        alias_key,
        alias,
        canonical,
        source: AliasSource::parse(&source)?,
        batch_id,
    })
}

const PROPOSAL_COLUMNS: &str =
    "id, user_id, canonical, members, source, confidence, status, batch_id, created_at, decided_at";

type ProposalRow = (
    Uuid,
    Uuid,
    String,
    Vec<String>,
    String,
    String,
    String,
    Option<Uuid>,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
);

fn proposal_from_row(row: ProposalRow) -> Result<TagMergeProposal, DomainError> {
    let (
        id,
        user_id,
        canonical,
        members,
        source,
        confidence,
        status,
        batch_id,
        created_at,
        decided_at,
    ) = row;
    Ok(TagMergeProposal {
        id,
        user_id,
        canonical,
        members,
        source: ProposalSource::parse(&source)?,
        confidence: Confidence::parse(&confidence)?,
        status: ProposalStatus::parse(&status)?,
        batch_id,
        created_at,
        decided_at,
    })
}

const BATCH_COLUMNS: &str = "id, user_id, kind, summary::text, undone_at, created_at";

type BatchRow = (
    Uuid,
    Uuid,
    String,
    String,
    Option<DateTime<Utc>>,
    DateTime<Utc>,
);

fn batch_from_row(row: BatchRow) -> Result<TagChangeBatch, DomainError> {
    let (id, user_id, kind, summary, undone_at, created_at) = row;
    Ok(TagChangeBatch {
        id,
        user_id,
        kind: ChangeKind::parse(&kind)?,
        summary: serde_json::from_str(&summary)
            .map_err(|e| DomainError::Internal(format!("bad tag batch summary: {e}")))?,
        undone_at,
        created_at,
    })
}

/// An alias row as it was before a batch changed it. Every column but the
/// key is `None` when the row did not exist.
#[derive(sqlx::FromRow)]
struct AliasImage {
    alias_key: String,
    alias: Option<String>,
    canonical: Option<String>,
    source: Option<String>,
    batch_id: Option<Uuid>,
    created_at: Option<DateTime<Utc>>,
}

fn sorted(members: &[String]) -> Vec<String> {
    let mut members = members.to_vec();
    members.sort();
    members.dedup();
    members
}

async fn load_aliases<'e>(
    executor: impl PgExecutor<'e>,
    user_id: Uuid,
) -> Result<Vec<TagAlias>, DomainError> {
    sqlx::query_as::<_, AliasRow>(
        "SELECT alias_key, alias, canonical, source, batch_id FROM tag_aliases
         WHERE user_id = $1
         ORDER BY canonical, alias",
    )
    .bind(user_id)
    .fetch_all(executor)
    .await
    .map_err(db_err)?
    .into_iter()
    .map(alias_from_row)
    .collect()
}

async fn load_tag_counts<'e>(
    executor: impl PgExecutor<'e>,
    user_id: Uuid,
) -> Result<Vec<(String, i64)>, DomainError> {
    sqlx::query_as::<_, (String, i64)>(
        "SELECT tag, COUNT(*) FROM bookmarks, unnest(tags) AS tag
         WHERE user_id = $1
         GROUP BY tag
         ORDER BY tag",
    )
    .bind(user_id)
    .fetch_all(executor)
    .await
    .map_err(db_err)
}

/// Apply merges inside `tx`, which holds the per-user lock: rewrite bookmark
/// tags, repoint and add aliases, and record the batch with before-images of
/// everything it changed. With `skip_if_unchanged`, nothing is recorded and
/// `None` is returned when no bookmark and no alias would change.
async fn apply_merges_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    kind: ChangeKind,
    merges: &[TagMerge],
    skip_if_unchanged: bool,
) -> Result<Option<TagChangeBatch>, DomainError> {
    let mapping = merge_mapping(merges)?;
    let sources: Vec<String> = mapping.keys().cloned().collect();

    let rows = sqlx::query_as::<_, (Uuid, Vec<String>)>(
        "SELECT id, tags FROM bookmarks
         WHERE user_id = $1 AND tags && $2
         ORDER BY id
         FOR UPDATE",
    )
    .bind(user_id)
    .bind(&sources)
    .fetch_all(&mut **tx)
    .await
    .map_err(db_err)?;
    let changes: Vec<(Uuid, Vec<String>, Vec<String>)> = rows
        .into_iter()
        .filter_map(|(id, before)| {
            let after = rewrite_tags(&before, &mapping);
            (after != before).then_some((id, before, after))
        })
        .collect();

    // Aliases: every alias of a merged tag now points at its target, and
    // every merged spelling and target gets a key of its own.
    let repointed = sqlx::query_as::<_, AliasRow>(
        "SELECT alias_key, alias, canonical, source, batch_id FROM tag_aliases
         WHERE user_id = $1 AND canonical = ANY($2)",
    )
    .bind(user_id)
    .bind(&sources)
    .fetch_all(&mut **tx)
    .await
    .map_err(db_err)?;
    let mut desired: BTreeMap<String, (String, String)> = BTreeMap::new();
    for (key, alias, canonical, _, _) in repointed {
        desired.insert(key, (alias, mapping[&canonical].clone()));
    }
    for (source, target) in &mapping {
        desired.insert(compact_key(source), (source.clone(), target.clone()));
    }
    for target in mapping.values() {
        desired.insert(compact_key(target), (target.clone(), target.clone()));
    }

    let keys: Vec<String> = desired.keys().cloned().collect();
    let mut current: HashMap<String, AliasImage> = sqlx::query_as::<_, AliasImage>(
        "SELECT alias_key, alias, canonical, source, batch_id, created_at FROM tag_aliases
         WHERE user_id = $1 AND alias_key = ANY($2)",
    )
    .bind(user_id)
    .bind(&keys)
    .fetch_all(&mut **tx)
    .await
    .map_err(db_err)?
    .into_iter()
    .map(|image| (image.alias_key.clone(), image))
    .collect();
    desired.retain(|key, (_, canonical)| {
        current
            .get(key)
            .is_none_or(|b| b.canonical.as_ref() != Some(canonical))
    });

    if skip_if_unchanged && changes.is_empty() && desired.is_empty() {
        return Ok(None);
    }

    let summary = TagChangeSummary {
        merges: merges.to_vec(),
        bookmarks_changed: changes.len() as u64,
    };
    let summary_json = serde_json::to_string(&summary)
        .map_err(|e| DomainError::Internal(format!("bad tag batch summary: {e}")))?;
    let batch = sqlx::query_as::<_, BatchRow>(&format!(
        "INSERT INTO tag_change_batches (user_id, kind, summary)
         VALUES ($1, $2, $3::jsonb)
         RETURNING {BATCH_COLUMNS}"
    ))
    .bind(user_id)
    .bind(kind.as_str())
    .bind(&summary_json)
    .fetch_one(&mut **tx)
    .await
    .map_err(db_err)?;
    let batch = batch_from_row(batch)?;

    for (id, before, after) in &changes {
        sqlx::query(
            "UPDATE bookmarks SET tags = $1, updated_at = now() WHERE id = $2 AND user_id = $3",
        )
        .bind(after)
        .bind(id)
        .bind(user_id)
        .execute(&mut **tx)
        .await
        .map_err(db_err)?;
        sqlx::query(
            "INSERT INTO tag_change_snapshots (batch_id, bookmark_id, tags_before, tags_after)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(batch.id)
        .bind(id)
        .bind(before)
        .bind(after)
        .execute(&mut **tx)
        .await
        .map_err(db_err)?;
    }

    let alias_source = kind.alias_source().as_str();
    for (key, (alias, canonical)) in &desired {
        let before = current.remove(key);
        sqlx::query(
            "INSERT INTO tag_change_alias_snapshots
                (batch_id, alias_key, alias_before, canonical_before, source_before,
                 batch_id_before, created_at_before)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(batch.id)
        .bind(key)
        .bind(before.as_ref().and_then(|b| b.alias.as_ref()))
        .bind(before.as_ref().and_then(|b| b.canonical.as_ref()))
        .bind(before.as_ref().and_then(|b| b.source.as_ref()))
        .bind(before.as_ref().and_then(|b| b.batch_id))
        .bind(before.as_ref().and_then(|b| b.created_at))
        .execute(&mut **tx)
        .await
        .map_err(db_err)?;
        sqlx::query(
            "INSERT INTO tag_aliases (user_id, alias_key, alias, canonical, source, batch_id)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (user_id, alias_key) DO UPDATE
             SET alias = EXCLUDED.alias, canonical = EXCLUDED.canonical,
                 source = EXCLUDED.source, batch_id = EXCLUDED.batch_id",
        )
        .bind(user_id)
        .bind(key)
        .bind(alias)
        .bind(canonical)
        .bind(alias_source)
        .bind(batch.id)
        .execute(&mut **tx)
        .await
        .map_err(db_err)?;
    }

    Ok(Some(batch))
}

/// Replace the user's pending proposals from `source`, skipping member sets
/// the user dismissed. The caller holds the per-user lock.
pub(super) async fn replace_pending_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    source: ProposalSource,
    proposals: &[NewTagMergeProposal],
) -> Result<Vec<TagMergeProposal>, DomainError> {
    sqlx::query(
        "DELETE FROM tag_merge_proposals
         WHERE user_id = $1 AND source = $2 AND status = 'pending'",
    )
    .bind(user_id)
    .bind(source.as_str())
    .execute(&mut **tx)
    .await
    .map_err(db_err)?;
    let dismissed: HashSet<Vec<String>> = sqlx::query_scalar::<_, Vec<String>>(
        "SELECT members FROM tag_merge_proposals
         WHERE user_id = $1 AND status = 'dismissed'",
    )
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(db_err)?
    .iter()
    .map(|members| sorted(members))
    .collect();

    let mut out = Vec::with_capacity(proposals.len());
    for proposal in proposals {
        let members = sorted(&proposal.members);
        if dismissed.contains(&members) {
            continue;
        }
        let row = sqlx::query_as::<_, ProposalRow>(&format!(
            "INSERT INTO tag_merge_proposals (user_id, canonical, members, source, confidence)
             VALUES ($1, $2, $3, $4, $5)
             RETURNING {PROPOSAL_COLUMNS}"
        ))
        .bind(user_id)
        .bind(&proposal.canonical)
        .bind(&members)
        .bind(source.as_str())
        .bind(proposal.confidence.as_str())
        .fetch_one(&mut **tx)
        .await
        .map_err(db_err)?;
        out.push(proposal_from_row(row)?);
    }
    Ok(out)
}

/// Apply the merges of proposals `ids` as one batch and mark them applied.
async fn apply_proposal_merges_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    kind: ChangeKind,
    merges: &[TagMerge],
    ids: &[Uuid],
) -> Result<TagChangeBatch, DomainError> {
    let batch = apply_merges_tx(tx, user_id, kind, merges, false)
        .await?
        .ok_or_else(|| DomainError::Internal("no batch recorded for proposals".into()))?;
    sqlx::query(
        "UPDATE tag_merge_proposals
         SET status = 'applied', batch_id = $3, decided_at = now()
         WHERE user_id = $1 AND id = ANY($2)",
    )
    .bind(user_id)
    .bind(ids)
    .bind(batch.id)
    .execute(&mut **tx)
    .await
    .map_err(db_err)?;
    Ok(batch)
}

impl TagAliasRepository for PostgresPool {
    async fn list_aliases(&self, user_id: Uuid) -> Result<Vec<TagAlias>, DomainError> {
        load_aliases(&self.pool, user_id).await
    }

    async fn tag_counts(&self, user_id: Uuid) -> Result<Vec<(String, i64)>, DomainError> {
        load_tag_counts(&self.pool, user_id).await
    }

    async fn tag_samples(&self, user_id: Uuid) -> Result<Vec<TagSample>, DomainError> {
        let rows = sqlx::query_as::<_, (String, i64, Option<String>)>(
            "SELECT tag, COUNT(*),
                    (ARRAY_AGG(title ORDER BY created_at DESC, id DESC)
                        FILTER (WHERE title IS NOT NULL AND title <> ''))[1]
             FROM bookmarks, unnest(tags) AS tag
             WHERE user_id = $1
             GROUP BY tag
             ORDER BY tag",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(rows
            .into_iter()
            .map(|(tag, count, sample_title)| TagSample {
                tag,
                count,
                sample_title,
            })
            .collect())
    }

    async fn resolve_writes(
        &self,
        user_id: Uuid,
        writes: &[TagWrite],
    ) -> Result<Vec<Vec<String>>, DomainError> {
        if writes.iter().all(|w| w.tags.is_empty()) {
            return Ok(writes.iter().map(|_| Vec::new()).collect());
        }
        // Fast path, without the lock: when every new tag already has an
        // alias row (the usual case once a library has settled), alias rows
        // alone decide the result and nothing is written.
        let keys: Vec<String> = writes
            .iter()
            .flat_map(|w| w.tags.iter().filter(|t| !w.keep.contains(t)))
            .filter_map(|t| clean_tag(t).map(compact_key))
            .collect();
        let known = sqlx::query_as::<_, AliasRow>(
            "SELECT alias_key, alias, canonical, source, batch_id FROM tag_aliases
             WHERE user_id = $1 AND alias_key = ANY($2)",
        )
        .bind(user_id)
        .bind(&keys)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?
        .into_iter()
        .map(alias_from_row)
        .collect::<Result<Vec<_>, _>>()?;
        let (resolved, records) = resolve_writes(writes, &mut TagIndex::new(&known, &[]));
        if records.is_empty() {
            return Ok(resolved);
        }

        let mut tx = self.pool.begin().await.map_err(db_err)?;
        lock_user_tags(&mut tx, user_id).await?;
        let aliases = load_aliases(&mut *tx, user_id).await?;
        let counts = load_tag_counts(&mut *tx, user_id).await?;
        let (resolved, records) = resolve_writes(writes, &mut TagIndex::new(&aliases, &counts));
        if !records.is_empty() {
            let keys: Vec<&str> = records.iter().map(|a| a.alias_key.as_str()).collect();
            let spellings: Vec<&str> = records.iter().map(|a| a.alias.as_str()).collect();
            let canonicals: Vec<&str> = records.iter().map(|a| a.canonical.as_str()).collect();
            // The lock makes conflicts impossible; DO NOTHING only guards
            // against a key repeated within one batch of records.
            sqlx::query(
                "INSERT INTO tag_aliases (user_id, alias_key, alias, canonical, source)
                 SELECT $1, k, a, c, $5
                 FROM UNNEST($2::text[], $3::text[], $4::text[]) AS t(k, a, c)
                 ON CONFLICT (user_id, alias_key) DO NOTHING",
            )
            .bind(user_id)
            .bind(&keys)
            .bind(&spellings)
            .bind(&canonicals)
            .bind(AliasSource::Normalize.as_str())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(resolved)
    }

    async fn replace_pending_proposals(
        &self,
        user_id: Uuid,
        source: ProposalSource,
        proposals: &[NewTagMergeProposal],
    ) -> Result<Vec<TagMergeProposal>, DomainError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        // Serialized with other runs and with apply/undo, so two concurrent
        // runs cannot both delete and then both insert their groups.
        lock_user_tags(&mut tx, user_id).await?;
        let out = replace_pending_tx(&mut tx, user_id, source, proposals).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(out)
    }

    async fn list_proposals(
        &self,
        user_id: Uuid,
        status: Option<ProposalStatus>,
    ) -> Result<Vec<TagMergeProposal>, DomainError> {
        sqlx::query_as::<_, ProposalRow>(&format!(
            "SELECT {PROPOSAL_COLUMNS} FROM tag_merge_proposals
             WHERE user_id = $1 AND ($2::text IS NULL OR status = $2)
             ORDER BY created_at DESC, canonical"
        ))
        .bind(user_id)
        .bind(status.map(ProposalStatus::as_str))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?
        .into_iter()
        .map(proposal_from_row)
        .collect()
    }

    async fn dismiss_proposals(&self, user_id: Uuid, ids: &[Uuid]) -> Result<u64, DomainError> {
        sqlx::query(
            "UPDATE tag_merge_proposals SET status = 'dismissed', decided_at = now()
             WHERE user_id = $1 AND id = ANY($2) AND status = 'pending'",
        )
        .bind(user_id)
        .bind(ids)
        .execute(&self.pool)
        .await
        .map(|r| r.rows_affected())
        .map_err(db_err)
    }

    async fn apply_proposals(
        &self,
        user_id: Uuid,
        ids: &[Uuid],
    ) -> Result<TagChangeBatch, DomainError> {
        let ids: Vec<Uuid> = ids
            .iter()
            .copied()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        if ids.is_empty() {
            return Err(DomainError::InvalidInput("no proposals selected".into()));
        }
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        lock_user_tags(&mut tx, user_id).await?;
        let proposals = sqlx::query_as::<_, ProposalRow>(&format!(
            "SELECT {PROPOSAL_COLUMNS} FROM tag_merge_proposals
             WHERE user_id = $1 AND id = ANY($2)
             ORDER BY created_at, id
             FOR UPDATE"
        ))
        .bind(user_id)
        .bind(&ids)
        .fetch_all(&mut *tx)
        .await
        .map_err(db_err)?
        .into_iter()
        .map(proposal_from_row)
        .collect::<Result<Vec<_>, _>>()?;
        if proposals.len() != ids.len() {
            return Err(DomainError::NotFound);
        }
        if proposals
            .iter()
            .any(|p| p.status != ProposalStatus::Pending)
        {
            return Err(DomainError::InvalidInput(
                "only pending proposals can be applied".into(),
            ));
        }

        let kind = if proposals.iter().any(|p| p.source == ProposalSource::Llm) {
            ChangeKind::LlmMerge
        } else {
            ChangeKind::Normalize
        };
        let merges: Vec<TagMerge> = proposals.iter().map(TagMergeProposal::merge).collect();
        let batch = apply_proposal_merges_tx(&mut tx, user_id, kind, &merges, &ids).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(batch)
    }

    async fn apply_reviewed(
        &self,
        user_id: Uuid,
        rows: &[ReviewedRow],
    ) -> Result<TagChangeBatch, DomainError> {
        if rows.is_empty() {
            return Err(DomainError::InvalidInput(
                "Check at least one merge.".into(),
            ));
        }
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        lock_user_tags(&mut tx, user_id).await?;
        let pending = sqlx::query_as::<_, ProposalRow>(&format!(
            "SELECT {PROPOSAL_COLUMNS} FROM tag_merge_proposals
             WHERE user_id = $1 AND status = 'pending'
             ORDER BY created_at, id
             FOR UPDATE"
        ))
        .bind(user_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(db_err)?
        .into_iter()
        .map(proposal_from_row)
        .collect::<Result<Vec<_>, _>>()?;
        let aliases = load_aliases(&mut *tx, user_id).await?;
        let on_bookmarks: HashSet<String> = load_tag_counts(&mut *tx, user_id)
            .await?
            .into_iter()
            .map(|(tag, _)| tag)
            .collect();
        let plan = plan_reviewed_apply(
            rows,
            &resolve_through_aliases(&pending, &aliases, &on_bookmarks),
        )?;
        // Edited targets are saved with the apply, so a failed apply leaves
        // none behind and a second tab cannot slip its own in between.
        for (id, canonical) in &plan.edited {
            sqlx::query(
                "UPDATE tag_merge_proposals SET canonical = $3 WHERE user_id = $1 AND id = $2",
            )
            .bind(user_id)
            .bind(id)
            .bind(canonical)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }
        let batch =
            apply_proposal_merges_tx(&mut tx, user_id, plan.kind, &plan.merges, &plan.ids).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(batch)
    }

    async fn apply_manual_merge(
        &self,
        user_id: Uuid,
        request: &ManualMerge,
    ) -> Result<(ManualMergePlan, Option<TagChangeBatch>), DomainError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        lock_user_tags(&mut tx, user_id).await?;
        let aliases = load_aliases(&mut *tx, user_id).await?;
        let counts = load_tag_counts(&mut *tx, user_id).await?;
        let plan = plan_manual_merge(request, &library_rows(&counts, &aliases), &aliases)?;
        let batch = apply_merges_tx(
            &mut tx,
            user_id,
            plan.kind,
            std::slice::from_ref(&plan.merge),
            true,
        )
        .await?;
        tx.commit().await.map_err(db_err)?;
        Ok((plan, batch))
    }

    async fn undo_batch(
        &self,
        user_id: Uuid,
        batch_id: Uuid,
    ) -> Result<TagChangeBatch, DomainError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        lock_user_tags(&mut tx, user_id).await?;
        let batch = sqlx::query_as::<_, BatchRow>(&format!(
            "SELECT {BATCH_COLUMNS} FROM tag_change_batches
             WHERE id = $1 AND user_id = $2
             FOR UPDATE"
        ))
        .bind(batch_id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?
        .ok_or(DomainError::NotFound)
        .and_then(batch_from_row)?;
        if batch.undone_at.is_some() {
            return Err(DomainError::InvalidInput(
                "this change was already undone".into(),
            ));
        }

        let blocked = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                 SELECT 1 FROM tag_change_batches later
                 WHERE later.user_id = $1 AND later.undone_at IS NULL
                   AND later.seq > (SELECT seq FROM tag_change_batches WHERE id = $2)
                   AND (EXISTS (SELECT 1 FROM tag_change_snapshots ls
                                JOIN tag_change_snapshots mine
                                  ON mine.bookmark_id = ls.bookmark_id AND mine.batch_id = $2
                                WHERE ls.batch_id = later.id)
                        OR EXISTS (SELECT 1 FROM tag_change_alias_snapshots la
                                   JOIN tag_change_alias_snapshots mine
                                     ON mine.alias_key = la.alias_key AND mine.batch_id = $2
                                   WHERE la.batch_id = later.id)))",
        )
        .bind(user_id)
        .bind(batch.id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
        if blocked {
            return Err(DomainError::InvalidInput(
                "undo the newer change first".into(),
            ));
        }

        let snapshots = sqlx::query_as::<_, (Uuid, Vec<String>, Vec<String>, Vec<String>)>(
            "SELECT b.id, b.tags, s.tags_before, s.tags_after
             FROM tag_change_snapshots s
             JOIN bookmarks b ON b.id = s.bookmark_id AND b.user_id = $2
             WHERE s.batch_id = $1
             ORDER BY b.id
             FOR UPDATE OF b",
        )
        .bind(batch.id)
        .bind(user_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(db_err)?;
        for (id, current, before, after) in snapshots {
            let restored = undo_tags(&current, &before, &after);
            if restored != current {
                sqlx::query(
                    "UPDATE bookmarks SET tags = $1, updated_at = now() WHERE id = $2 AND user_id = $3",
                )
                .bind(&restored)
                .bind(id)
                .bind(user_id)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            }
        }

        let alias_snapshots = sqlx::query_as::<_, AliasImage>(
            "SELECT alias_key, alias_before AS alias, canonical_before AS canonical,
                    source_before AS source, batch_id_before AS batch_id,
                    created_at_before AS created_at
             FROM tag_change_alias_snapshots
             WHERE batch_id = $1",
        )
        .bind(batch.id)
        .fetch_all(&mut *tx)
        .await
        .map_err(db_err)?;
        for image in alias_snapshots {
            let AliasImage {
                alias_key: key,
                batch_id: before_batch,
                ..
            } = image;
            match (image.alias, image.canonical, image.source, image.created_at) {
                (Some(alias), Some(canonical), Some(source), Some(created_at)) => {
                    sqlx::query(
                        "INSERT INTO tag_aliases
                            (user_id, alias_key, alias, canonical, source, batch_id, created_at)
                         VALUES ($1, $2, $3, $4, $5, $6, $7)
                         ON CONFLICT (user_id, alias_key) DO UPDATE
                         SET alias = EXCLUDED.alias, canonical = EXCLUDED.canonical,
                             source = EXCLUDED.source, batch_id = EXCLUDED.batch_id,
                             created_at = EXCLUDED.created_at",
                    )
                    .bind(user_id)
                    .bind(&key)
                    .bind(alias)
                    .bind(canonical)
                    .bind(source)
                    .bind(before_batch)
                    .bind(created_at)
                    .execute(&mut *tx)
                    .await
                    .map_err(db_err)?;
                }
                _ => {
                    sqlx::query("DELETE FROM tag_aliases WHERE user_id = $1 AND alias_key = $2")
                        .bind(user_id)
                        .bind(&key)
                        .execute(&mut *tx)
                        .await
                        .map_err(db_err)?;
                }
            }
        }

        sqlx::query(
            "UPDATE tag_merge_proposals
             SET status = 'pending', batch_id = NULL, decided_at = NULL
             WHERE user_id = $1 AND batch_id = $2",
        )
        .bind(user_id)
        .bind(batch.id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;

        let undone = sqlx::query_as::<_, BatchRow>(&format!(
            "UPDATE tag_change_batches SET undone_at = now()
             WHERE id = $1
             RETURNING {BATCH_COLUMNS}"
        ))
        .bind(batch.id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        batch_from_row(undone)
    }

    async fn list_batches(
        &self,
        user_id: Uuid,
        limit: i64,
    ) -> Result<Vec<TagChangeBatch>, DomainError> {
        sqlx::query_as::<_, BatchRow>(&format!(
            "SELECT {BATCH_COLUMNS} FROM tag_change_batches
             WHERE user_id = $1
             ORDER BY seq DESC
             LIMIT $2"
        ))
        .bind(user_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?
        .into_iter()
        .map(batch_from_row)
        .collect()
    }
}

#[cfg(test)]
mod tag_alias_postgres_tests {
    use super::*;
    use crate::app::tag_alias::TagAliasService;
    use crate::domain::tag_library::STALE_LIBRARY;
    use sqlx::PgPool;
    use sqlx::postgres::PgPoolOptions;
    use std::sync::Arc;

    type BookmarkState = Vec<(Uuid, Vec<String>, DateTime<Utc>)>;
    type AliasState = Vec<(String, String, String, String, Option<Uuid>, DateTime<Utc>)>;

    async fn bookmark(pool: &PgPool, user_id: Uuid, tags: &[&str]) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO bookmarks (user_id, url, tags) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(user_id)
        .bind(format!("https://example.com/{}", Uuid::new_v4()))
        .bind(tags.iter().map(|t| t.to_string()).collect::<Vec<_>>())
        .fetch_one(pool)
        .await
        .expect("insert bookmark")
    }

    async fn bookmarks(pool: &PgPool, user_id: Uuid) -> BookmarkState {
        sqlx::query_as("SELECT id, tags, updated_at FROM bookmarks WHERE user_id = $1 ORDER BY id")
            .bind(user_id)
            .fetch_all(pool)
            .await
            .expect("load bookmarks")
    }

    async fn tags_of(pool: &PgPool, id: Uuid) -> Vec<String> {
        sqlx::query_scalar("SELECT tags FROM bookmarks WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .expect("load tags")
    }

    async fn aliases(pool: &PgPool, user_id: Uuid) -> AliasState {
        sqlx::query_as(
            "SELECT alias_key, alias, canonical, source, batch_id, created_at FROM tag_aliases
             WHERE user_id = $1 ORDER BY alias_key",
        )
        .bind(user_id)
        .fetch_all(pool)
        .await
        .expect("load aliases")
    }

    fn s(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_string()).collect()
    }

    fn manual(kind: ChangeKind, sources: &[&str], target: &str) -> ManualMerge {
        ManualMerge {
            kind,
            sources: s(sources),
            target: target.into(),
        }
    }

    async fn resolve(
        svc: &TagAliasService<PostgresPool>,
        user_id: Uuid,
        tags: &[&str],
        keep: &[&str],
    ) -> Vec<String> {
        let write = TagWrite {
            tags: s(tags),
            keep: s(keep),
        };
        svc.resolve_writes(user_id, &[write])
            .await
            .expect("resolve tags")
            .remove(0)
    }

    async fn connect() -> PgPool {
        let database_url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL is required for the ignored tag alias Postgres tests");
        PgPoolOptions::new()
            .max_connections(8)
            .connect(&database_url)
            .await
            .expect("DATABASE_URL must point at a migrated database")
    }

    async fn new_user(pool: &PgPool) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, email, name) VALUES ($1, $2, 'tag alias test')")
            .bind(id)
            .bind(format!("tag-alias-{id}@example.test"))
            .execute(pool)
            .await
            .expect("insert user");
        id
    }

    /// Run explicitly with `--ignored` and `DATABASE_URL` pointing at a
    /// migrated Postgres (CI applies migrations/*.sql with psql first).
    #[tokio::test]
    #[ignore = "requires a Postgres database in DATABASE_URL"]
    async fn tag_alias_propose_apply_undo_round_trip() {
        let pool = connect().await;
        let repo = Arc::new(PostgresPool::new(pool.clone()));
        let svc = TagAliasService::new(repo.clone());
        let user = new_user(&pool).await;
        let other = new_user(&pool).await;

        // A messy legacy library, written without the resolver.
        let b_github_cased = bookmark(&pool, user, &["GitHub", "rust"]).await;
        let b_github_lower = bookmark(&pool, user, &["github"]).await;
        bookmark(&pool, user, &["GitHub"]).await;
        bookmark(&pool, user, &["coding agents"]).await;
        let b_agent = bookmark(&pool, user, &["coding-agent", "rust"]).await;
        bookmark(&pool, user, &["Rust"]).await;
        let b_untouched = bookmark(&pool, user, &["UI/UX", "ux", "kubernetes"]).await;
        let other_bookmark = bookmark(&pool, other, &["github", "GitHub"]).await;
        // Aliases from earlier writes: one points at a tag that will be
        // merged away, so the merge must repoint it and undo restore it.
        sqlx::query(
            "INSERT INTO tag_aliases (user_id, alias_key, alias, canonical, source)
             VALUES ($1, 'octocat', 'octocat', 'github', 'manual'),
                    ($1, 'github', 'github', 'github', 'normalize')",
        )
        .bind(user)
        .execute(&pool)
        .await
        .expect("insert aliases");

        let legacy = bookmarks(&pool, user).await;
        let other_legacy = bookmarks(&pool, other).await;
        let aliases_before = aliases(&pool, user).await;

        // Proposing never changes bookmarks or aliases.
        let proposals = svc.propose_normalization(user).await.expect("propose");
        let summary: Vec<(String, Vec<String>)> = proposals
            .iter()
            .map(|p| (p.canonical.clone(), p.members.clone()))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("GitHub".to_string(), s(&["GitHub", "github"])),
                (
                    "coding agents".to_string(),
                    s(&["coding agents", "coding-agent"])
                ),
                ("rust".to_string(), s(&["Rust", "rust"])),
            ]
        );
        assert_eq!(bookmarks(&pool, user).await, legacy);
        assert_eq!(aliases(&pool, user).await, aliases_before);

        // Re-running replaces pending proposals; a dismissed group stays gone.
        let rust = proposals.iter().find(|p| p.canonical == "rust").unwrap();
        assert_eq!(svc.dismiss_proposals(user, &[rust.id]).await.unwrap(), 1);
        let proposals = svc.propose_normalization(user).await.expect("re-propose");
        assert_eq!(proposals.len(), 2);
        assert_eq!(
            svc.list_proposals(user, Some(ProposalStatus::Pending))
                .await
                .unwrap()
                .len(),
            2
        );
        let ids: Vec<Uuid> = proposals.iter().map(|p| p.id).collect();

        // Apply both as one batch.
        let batch = svc.apply_merge(user, &ids).await.expect("apply");
        assert_eq!(batch.kind, ChangeKind::Normalize);
        assert_eq!(batch.summary.bookmarks_changed, 2);
        assert_eq!(tags_of(&pool, b_github_cased).await, s(&["GitHub", "rust"]));
        assert_eq!(tags_of(&pool, b_github_lower).await, s(&["GitHub"]));
        assert_eq!(tags_of(&pool, b_agent).await, s(&["coding agents", "rust"]));
        let changed = [b_github_lower, b_agent];
        let after_apply = bookmarks(&pool, user).await;
        for (before, after) in legacy.iter().zip(&after_apply) {
            if !changed.contains(&before.0) {
                assert_eq!(before, after, "an unmerged bookmark changed");
            }
        }
        assert_eq!(bookmarks(&pool, other).await, other_legacy);
        assert!(
            svc.list_proposals(user, Some(ProposalStatus::Applied))
                .await
                .unwrap()
                .iter()
                .all(|p| p.batch_id == Some(batch.id))
        );
        assert!(matches!(
            svc.apply_merge(user, &ids).await,
            Err(DomainError::InvalidInput(_))
        ));

        // Old names, including the repointed alias, resolve to the merge target.
        let canon = |aliases: &AliasState, key: &str| {
            aliases.iter().find(|a| a.0 == key).map(|a| a.2.clone())
        };
        let merged_aliases = aliases(&pool, user).await;
        assert_eq!(canon(&merged_aliases, "octocat").as_deref(), Some("GitHub"));
        assert_eq!(canon(&merged_aliases, "github").as_deref(), Some("GitHub"));
        assert_eq!(
            canon(&merged_aliases, "codingagent").as_deref(),
            Some("coding agents")
        );
        assert_eq!(
            resolve(
                &svc,
                user,
                &["github", "Octocat", "coding-agent", "Coding Agents"],
                &[]
            )
            .await,
            s(&["GitHub", "coding agents"])
        );

        // The user edits a merged bookmark before undoing.
        sqlx::query("UPDATE bookmarks SET tags = $1 WHERE id = $2")
            .bind(s(&["GitHub", "news"]))
            .bind(b_github_lower)
            .execute(&pool)
            .await
            .unwrap();

        let undone = svc.undo_batch(user, batch.id).await.expect("undo");
        assert!(undone.undone_at.is_some());
        assert_eq!(tags_of(&pool, b_github_lower).await, s(&["news", "github"]));
        assert_eq!(tags_of(&pool, b_agent).await, s(&["coding-agent", "rust"]));
        assert_eq!(
            tags_of(&pool, b_untouched).await,
            s(&["UI/UX", "ux", "kubernetes"])
        );
        assert_eq!(
            tags_of(&pool, other_bookmark).await,
            s(&["github", "GitHub"])
        );
        // Every alias row is exactly as before, so old spellings resolve as before.
        assert_eq!(aliases(&pool, user).await, aliases_before);
        assert_eq!(
            resolve(&svc, user, &["Octocat", "GITHUB"], &[]).await,
            s(&["github"])
        );
        assert_eq!(
            svc.list_proposals(user, Some(ProposalStatus::Pending))
                .await
                .unwrap()
                .len(),
            2
        );
        assert!(matches!(
            svc.undo_batch(user, batch.id).await,
            Err(DomainError::InvalidInput(_))
        ));

        // A batch that a later, still-applied batch depends on cannot be undone.
        let first = svc.apply_merge(user, &ids).await.expect("re-apply");
        let rename = manual(ChangeKind::Rename, &["GitHub"], "Git Hub");
        let second = svc
            .merge_tags(user, &rename)
            .await
            .expect("rename")
            .1
            .expect("the rename changes tags");
        // A retried rename names a tag that is gone: refused, no empty batch.
        assert!(matches!(
            svc.merge_tags(user, &rename).await,
            Err(DomainError::InvalidInput(m)) if m == STALE_LIBRARY
        ));
        assert!(matches!(
            svc.undo_batch(user, first.id).await,
            Err(DomainError::InvalidInput(_))
        ));
        svc.undo_batch(user, second.id).await.expect("undo rename");
        svc.undo_batch(user, first.id).await.expect("undo first");
        assert_eq!(tags_of(&pool, b_github_cased).await, s(&["GitHub", "rust"]));
        assert_eq!(tags_of(&pool, b_agent).await, s(&["coding-agent", "rust"]));
        assert_eq!(svc.list_batches(user, 10).await.unwrap().len(), 3);

        // Other users cannot see or undo this user's batches.
        assert!(matches!(
            svc.undo_batch(other, first.id).await,
            Err(DomainError::NotFound)
        ));

        sqlx::query("DELETE FROM users WHERE id IN ($1, $2)")
            .bind(user)
            .bind(other)
            .execute(&pool)
            .await
            .expect("cleanup users");
        pool.close().await;
    }

    /// Concurrent writes of new spellings agree on one canonical, never
    /// touch the tags already on a bookmark, and a double-submitted manual
    /// merge records one batch.
    #[tokio::test]
    #[ignore = "requires a Postgres database in DATABASE_URL"]
    async fn tag_alias_concurrent_writes_and_merges_are_serialized() {
        let pool = connect().await;
        let svc = Arc::new(TagAliasService::new(Arc::new(PostgresPool::new(
            pool.clone(),
        ))));
        let user = new_user(&pool).await;
        bookmark(&pool, user, &["Rust"]).await;

        let spellings = ["Zig", "zig", "ZIG", "zig-lang", "Zig Lang", "zigs"];
        let tasks: Vec<_> = spellings
            .iter()
            .map(|spelling| {
                let svc = svc.clone();
                let write = TagWrite {
                    tags: s(&["Rust", "rusts", spelling]),
                    keep: s(&["Rust"]),
                };
                tokio::spawn(async move { svc.resolve_writes(user, &[write]).await })
            })
            .collect();
        let mut results = Vec::new();
        for task in tasks {
            results.push(task.await.unwrap().expect("resolve").remove(0));
        }
        for tags in &results {
            assert_eq!(tags[0], "Rust", "a kept tag was rewritten: {tags:?}");
        }
        let zig: HashSet<&String> = results
            .iter()
            .zip(spellings)
            .filter(|(_, spelling)| compact_key(spelling) == "zig" || *spelling == "zigs")
            .map(|(tags, _)| tags.last().unwrap())
            .collect();
        assert_eq!(
            zig.len(),
            1,
            "spellings of one new tag diverged: {results:?}"
        );
        let zig = (*zig.iter().next().unwrap()).clone();
        let stored: Vec<String> = sqlx::query_scalar(
            "SELECT canonical FROM tag_aliases WHERE user_id = $1 AND alias_key IN ('zig', 'zigs')",
        )
        .bind(user)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(stored.iter().all(|c| *c == zig), "{stored:?} vs {zig}");

        bookmark(&pool, user, &["Zig Lang"]).await;
        let merge = manual(ChangeKind::Merge, &["Zig Lang"], &zig);
        let (left, right) =
            tokio::join!(svc.merge_tags(user, &merge), svc.merge_tags(user, &merge));
        // The second sees the first's result under the lock and is refused.
        let recorded = [left, right]
            .into_iter()
            .filter_map(|r| r.ok().and_then(|(_, batch)| batch))
            .count();
        assert_eq!(recorded, 1);
        assert_eq!(svc.list_batches(user, 10).await.unwrap().len(), 1);

        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(user)
            .execute(&pool)
            .await
            .expect("cleanup user");
        pool.close().await;
    }

    /// Two tabs act on the same page at once: one merges `k8s` into
    /// `kubernetes`, the other renames `k8s` to `kube`. The library is
    /// checked under the lock, so whichever runs second sees that `k8s` is
    /// gone and is refused, instead of repointing the alias the first one
    /// just wrote.
    #[tokio::test]
    #[ignore = "requires a Postgres database in DATABASE_URL"]
    async fn tag_alias_concurrent_stale_merge_is_refused_under_the_lock() {
        let pool = connect().await;
        let svc = Arc::new(TagAliasService::new(Arc::new(PostgresPool::new(
            pool.clone(),
        ))));
        let user = new_user(&pool).await;
        let pods = bookmark(&pool, user, &["k8s"]).await;
        bookmark(&pool, user, &["kubernetes"]).await;

        let merge = manual(ChangeKind::Merge, &["k8s"], "kubernetes");
        let rename = manual(ChangeKind::Rename, &["k8s"], "kube");
        let (merged, renamed) =
            tokio::join!(svc.merge_tags(user, &merge), svc.merge_tags(user, &rename));
        let (winner, refused) = match (merged, renamed) {
            (Ok((_, Some(_))), Err(e)) => ("kubernetes", e),
            (Err(e), Ok((_, Some(_)))) => ("kube", e),
            other => panic!("exactly one change should apply: {other:?}"),
        };
        assert!(
            matches!(&refused, DomainError::InvalidInput(m) if m == STALE_LIBRARY),
            "{refused:?}"
        );
        assert_eq!(tags_of(&pool, pods).await, s(&[winner]));
        let k8s: String = sqlx::query_scalar(
            "SELECT canonical FROM tag_aliases WHERE user_id = $1 AND alias_key = 'k8s'",
        )
        .bind(user)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(k8s, winner);
        assert_eq!(svc.list_batches(user, 10).await.unwrap().len(), 1);

        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(user)
            .execute(&pool)
            .await
            .expect("cleanup user");
        pool.close().await;
    }
}
