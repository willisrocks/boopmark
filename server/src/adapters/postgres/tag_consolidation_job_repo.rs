use super::tag_alias_repo::replace_pending_tx;
use super::{PostgresPool, lock_user_tags};
use crate::domain::error::DomainError;
use crate::domain::ports::tag_consolidation_job_repo::TagConsolidationJobRepository;
use crate::domain::tag_alias::{NewTagMergeProposal, ProposalSource};
use crate::domain::tag_consolidation::{ConsolidationJob, JobOutcome, JobStatus};
use chrono::{DateTime, Utc};
use uuid::Uuid;

fn db_err(e: sqlx::Error) -> DomainError {
    DomainError::Internal(e.to_string())
}

type JobRow = (
    Uuid,
    String,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
    Option<String>,
    i32,
    i32,
    i64,
    i64,
);

impl TagConsolidationJobRepository for PostgresPool {
    async fn start_job(
        &self,
        user_id: Uuid,
        stale_minutes: i64,
    ) -> Result<Option<Uuid>, DomainError> {
        // One statement, so two clicks cannot both start a run.
        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO tag_consolidation_jobs (user_id, run_id, status)
             VALUES ($1, $2, 'running')
             ON CONFLICT (user_id) DO UPDATE
             SET run_id = EXCLUDED.run_id, status = 'running', started_at = now(),
                 finished_at = NULL, error = NULL, proposals = 0, pairs_checked = 0,
                 input_tokens = 0, output_tokens = 0
             WHERE tag_consolidation_jobs.status <> 'running'
                OR tag_consolidation_jobs.started_at < now() - make_interval(mins => $3)
             RETURNING run_id",
        )
        .bind(user_id)
        .bind(Uuid::new_v4())
        .bind(i32::try_from(stale_minutes).unwrap_or(i32::MAX))
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)
    }

    async fn store_run_proposals(
        &self,
        user_id: Uuid,
        run_id: Uuid,
        proposals: &[NewTagMergeProposal],
    ) -> Result<Option<usize>, DomainError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        lock_user_tags(&mut tx, user_id).await?;
        let current = sqlx::query_scalar::<_, Uuid>(
            "SELECT run_id FROM tag_consolidation_jobs
             WHERE user_id = $1 AND run_id = $2 AND status = 'running'
             FOR UPDATE",
        )
        .bind(user_id)
        .bind(run_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
        if current.is_none() {
            return Ok(None);
        }
        let stored = replace_pending_tx(&mut tx, user_id, ProposalSource::Llm, proposals).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(Some(stored.len()))
    }

    async fn finish_job(
        &self,
        user_id: Uuid,
        run_id: Uuid,
        outcome: &JobOutcome,
    ) -> Result<(), DomainError> {
        sqlx::query(
            "UPDATE tag_consolidation_jobs
             SET status = $3, finished_at = now(), error = $4, proposals = $5,
                 pairs_checked = $6, input_tokens = $7, output_tokens = $8
             WHERE user_id = $1 AND run_id = $2",
        )
        .bind(user_id)
        .bind(run_id)
        .bind(outcome.status.as_str())
        .bind(&outcome.error)
        .bind(outcome.proposals)
        .bind(outcome.pairs_checked)
        .bind(outcome.input_tokens)
        .bind(outcome.output_tokens)
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(db_err)
    }

    async fn get_job(&self, user_id: Uuid) -> Result<Option<ConsolidationJob>, DomainError> {
        let row = sqlx::query_as::<_, JobRow>(
            "SELECT run_id, status, started_at, finished_at, error, proposals, pairs_checked,
                    input_tokens, output_tokens
             FROM tag_consolidation_jobs WHERE user_id = $1",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.map(
            |(
                run_id,
                status,
                started_at,
                finished_at,
                error,
                proposals,
                pairs_checked,
                input_tokens,
                output_tokens,
            )| {
                Ok(ConsolidationJob {
                    run_id,
                    status: JobStatus::parse(&status)?,
                    started_at,
                    finished_at,
                    error,
                    proposals,
                    pairs_checked,
                    input_tokens,
                    output_tokens,
                })
            },
        )
        .transpose()
    }
}

#[cfg(test)]
mod tag_consolidation_postgres_tests {
    use super::*;
    use crate::app::secrets::SecretBox;
    use crate::app::settings::{SaveLlmSettingsInput, SettingsService};
    use crate::app::tag_alias::TagAliasService;
    use crate::app::tag_consolidation::TagConsolidationService;
    use crate::domain::ports::llm_client::{
        BoxFuture, LlmClient, StructuredRequest, StructuredResponse, TokenUsage,
    };
    use crate::domain::ports::tag_resolver::TagResolver;
    use crate::domain::tag_alias::{ProposalSource, TagWrite};
    use crate::domain::tag_consolidation::{
        LIST_CHANGED, ReviewRow, ReviewedRow, STALE_RUN_MINUTES,
    };
    use serde_json::{Value, json};
    use sqlx::PgPool;
    use sqlx::postgres::PgPoolOptions;
    use std::collections::HashMap;
    use std::sync::Arc;

    /// Answers like a model would, without a network: the whole-list call
    /// proposes `wfh` ~ `remote work` at `medium`; the pair check echoes
    /// every pair and judges it from a fixed table.
    struct ScriptedLlm {
        reject: bool,
    }

    fn verdict(a: &str, b: &str) -> Option<&'static str> {
        let mut pair = [a, b];
        pair.sort();
        match pair {
            ["javascript", "js"] | ["k8s", "kubernetes"] => Some("high"),
            ["remote work", "wfh"] => Some("medium"),
            _ => None,
        }
    }

    /// The first JSON string after `marker` in a prompt line.
    fn quoted_after(line: &str, marker: &str) -> String {
        let rest = &line[line.find(marker).expect("marker") + marker.len()..];
        serde_json::Deserializer::from_str(rest)
            .into_iter::<String>()
            .next()
            .expect("a quoted tag")
            .expect("valid JSON string")
    }

    impl LlmClient for ScriptedLlm {
        fn complete_structured(
            &self,
            req: StructuredRequest,
        ) -> BoxFuture<'_, Result<StructuredResponse, DomainError>> {
            Box::pin(async move {
                if self.reject {
                    return Err(DomainError::InvalidInput(
                        "Anthropic returned HTTP 401".into(),
                    ));
                }
                let json = if req.json_schema["properties"].get("merges").is_some() {
                    json!({"merges": [{
                        "canonical": "remote work",
                        "members": ["remote work", "wfh"],
                        "confidence": "medium"
                    }]})
                } else {
                    let pairs: Vec<Value> = req
                        .user
                        .lines()
                        .map(|line| {
                            let id: u64 = line.split('.').next().unwrap().parse().unwrap();
                            let (a, b) = (quoted_after(line, "A: "), quoted_after(line, "| B: "));
                            let confidence = verdict(&a, &b);
                            json!({
                                "id": id, "a": a, "b": b,
                                "same": confidence.is_some(),
                                "confidence": confidence.unwrap_or("high"),
                            })
                        })
                        .collect();
                    json!({ "pairs": pairs })
                };
                Ok(StructuredResponse {
                    json,
                    usage: TokenUsage {
                        input_tokens: 10,
                        output_tokens: 5,
                    },
                })
            })
        }
    }

    async fn connect() -> PgPool {
        let database_url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL is required for the ignored tag consolidation Postgres tests");
        PgPoolOptions::new()
            .max_connections(4)
            .connect(&database_url)
            .await
            .expect("DATABASE_URL must point at a migrated database")
    }

    async fn new_user(pool: &PgPool) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, email, name) VALUES ($1, $2, 'consolidation test')")
            .bind(id)
            .bind(format!("tag-consolidation-{id}@example.test"))
            .execute(pool)
            .await
            .expect("insert user");
        id
    }

    async fn bookmark(pool: &PgPool, user_id: Uuid, tags: &[&str]) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO bookmarks (user_id, url, title, tags) VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(user_id)
        .bind(format!("https://example.com/{}", Uuid::new_v4()))
        .bind(format!("About {}", tags.join(" and ")))
        .bind(tags.iter().map(|t| t.to_string()).collect::<Vec<_>>())
        .fetch_one(pool)
        .await
        .expect("insert bookmark")
    }

    async fn tags_by_id(pool: &PgPool, user_id: Uuid) -> HashMap<Uuid, Vec<String>> {
        sqlx::query_as::<_, (Uuid, Vec<String>)>(
            "SELECT id, tags FROM bookmarks WHERE user_id = $1",
        )
        .bind(user_id)
        .fetch_all(pool)
        .await
        .expect("load bookmarks")
        .into_iter()
        .collect()
    }

    /// The row with this target and source, checked as the page showed it.
    fn checked(
        rows: &[ReviewRow],
        canonical: &str,
        source: ProposalSource,
        edited: Option<&str>,
    ) -> ReviewedRow {
        let row = rows
            .iter()
            .find(|r| r.canonical == canonical && r.source == source)
            .unwrap_or_else(|| panic!("no {canonical} row"));
        ReviewedRow {
            id: row.id,
            shown: row.chips.iter().map(|c| c.tag.clone()).collect(),
            canonical: Some(edited.unwrap_or(&row.canonical).to_string()),
        }
    }

    async fn pending_canonical(
        tags: &TagAliasService<PostgresPool>,
        user: Uuid,
        id: Uuid,
    ) -> String {
        tags.list_proposals(user, None)
            .await
            .unwrap()
            .into_iter()
            .find(|p| p.id == id)
            .unwrap()
            .canonical
    }

    type Service = TagConsolidationService<PostgresPool, PostgresPool, PostgresPool>;

    async fn service(
        pool: &PgPool,
        user_id: Uuid,
        reject: bool,
    ) -> (Arc<Service>, Arc<TagAliasService<PostgresPool>>) {
        let repo = Arc::new(PostgresPool::new(pool.clone()));
        let settings = Arc::new(SettingsService::new(
            repo.clone(),
            Arc::new(SecretBox::new(
                "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
            )),
        ));
        settings
            .save(
                user_id,
                SaveLlmSettingsInput {
                    enabled: true,
                    anthropic_api_key: Some("sk-ant-test".into()),
                    ..Default::default()
                },
            )
            .await
            .expect("save LLM settings");
        let tags = Arc::new(TagAliasService::new(repo.clone()));
        let svc = Arc::new(TagConsolidationService::new(
            tags.clone(),
            repo,
            settings,
            Arc::new(ScriptedLlm { reject }),
        ));
        (svc, tags)
    }

    async fn cleanup(pool: &PgPool, user_id: Uuid) {
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(user_id)
            .execute(pool)
            .await
            .expect("cleanup user");
    }

    /// Run explicitly with `--ignored` and `DATABASE_URL` pointing at a
    /// migrated Postgres (CI applies migrations/*.sql with psql first).
    #[tokio::test]
    #[ignore = "requires a Postgres database in DATABASE_URL"]
    async fn consolidation_job_proposes_and_only_checked_rows_apply_then_undo() {
        let pool = connect().await;
        let user = new_user(&pool).await;
        let b_untouched = bookmark(&pool, user, &["javascript", "rust"]).await;
        let b_js = bookmark(&pool, user, &["js"]).await;
        let b_js_cased = bookmark(&pool, user, &["JS"]).await;
        let b_k8s = bookmark(&pool, user, &["k8s"]).await;
        bookmark(&pool, user, &["kubernetes"]).await;
        let b_wfh = bookmark(&pool, user, &["wfh"]).await;
        bookmark(&pool, user, &["remote work"]).await;
        let b_github = bookmark(&pool, user, &["GitHub"]).await;
        bookmark(&pool, user, &["github"]).await;
        let before = tags_by_id(&pool, user).await;
        let (svc, tags) = service(&pool, user, false).await;
        let repo = PostgresPool::new(pool.clone());

        // One run at a time; a run left running too long is replaced, and
        // the replaced run cannot record its outcome.
        let stale = repo
            .start_job(user, STALE_RUN_MINUTES)
            .await
            .unwrap()
            .unwrap();
        assert!(
            repo.start_job(user, STALE_RUN_MINUTES)
                .await
                .unwrap()
                .is_none()
        );
        sqlx::query(
            "UPDATE tag_consolidation_jobs SET started_at = now() - interval '1 hour'
             WHERE user_id = $1",
        )
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
        let run_id = repo
            .start_job(user, STALE_RUN_MINUTES)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(run_id, stale);

        // The run stores proposals and changes no tag.
        svc.run(user, run_id).await;
        let stale_outcome = JobOutcome {
            status: JobStatus::Failed,
            error: Some("stale".into()),
            proposals: 0,
            pairs_checked: 0,
            input_tokens: 0,
            output_tokens: 0,
        };
        repo.finish_job(user, stale, &stale_outcome).await.unwrap();
        let job = svc.job(user).await.unwrap().expect("job row");
        assert_eq!(job.run_id, run_id);
        assert_eq!(job.status, JobStatus::Succeeded, "{:?}", job.error);
        assert_eq!(job.proposals, 3);
        assert!(job.pairs_checked >= 3);
        assert!(job.input_tokens > 0);
        assert_eq!(tags_by_id(&pool, user).await, before);

        let review = svc.review(user).await.unwrap();
        // "Check these" first and unchecked, then the pre-checked rows; the
        // AI `javascript` row shows the `JS`/`js` spellings it also merges.
        let rows: Vec<String> = review
            .rows
            .iter()
            .map(|r| {
                let chips: Vec<&str> = r.chips.iter().map(|c| c.tag.as_str()).collect();
                format!(
                    "{} {:?} {} {} [{}]",
                    r.canonical,
                    r.source,
                    r.confidence.as_str(),
                    if r.prechecked { "checked" } else { "unchecked" },
                    chips.join(", ")
                )
            })
            .collect();
        let s = |v: &[&str]| v.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        assert_eq!(
            rows,
            s(&[
                "remote work Llm medium unchecked [remote work, wfh]",
                "javascript Llm high checked [javascript, js, JS]",
                "github Deterministic high checked [github, GitHub]",
                "js Deterministic high checked [js, JS]",
                "kubernetes Llm high checked [kubernetes, k8s]",
            ])
        );

        // Two tabs show the same rows and edit the kubernetes target
        // differently. Tab A checks javascript (which brings its JS/js row),
        // github and kubernetes; `remote work` stays unchecked.
        let tab_a = vec![
            checked(&review.rows, "javascript", ProposalSource::Llm, None),
            checked(&review.rows, "github", ProposalSource::Deterministic, None),
            checked(
                &review.rows,
                "kubernetes",
                ProposalSource::Llm,
                Some("Kubernetes"),
            ),
        ];
        let tab_b = vec![checked(
            &review.rows,
            "kubernetes",
            ProposalSource::Llm,
            Some("K8S"),
        )];

        // A failed apply saves no edit: tab B adds a row id that does not
        // exist, so its kubernetes edit must not stick.
        let mut broken = tab_b.clone();
        broken.push(ReviewedRow {
            id: Uuid::new_v4(),
            shown: vec![],
            canonical: None,
        });
        assert!(svc.apply(user, &broken).await.is_err());
        assert_eq!(tags_by_id(&pool, user).await, before);
        let kubernetes = pending_canonical(&tags, user, tab_b[0].id).await;
        assert_eq!(kubernetes, "kubernetes");

        let batch = svc.apply(user, &tab_a).await.expect("apply");
        assert_eq!(batch.summary.bookmarks_changed, 5);

        let after = tags_by_id(&pool, user).await;
        assert_eq!(after[&b_untouched], before[&b_untouched]);
        assert_eq!(after[&b_js], s(&["javascript"]));
        assert_eq!(after[&b_js_cased], s(&["javascript"]));
        assert_eq!(after[&b_k8s], s(&["Kubernetes"]));
        assert_eq!(after[&b_github], s(&["github"]));
        assert_eq!(after[&b_wfh], s(&["wfh"]), "an unchecked row never applies");
        // A double submit and the second tab are refused; nothing changes.
        for stale in [&tab_a, &tab_b] {
            let error = svc.apply(user, stale).await.unwrap_err();
            assert!(matches!(error, DomainError::InvalidInput(ref m) if m == LIST_CHANGED));
            assert_eq!(tags_by_id(&pool, user).await, after);
        }
        // The next save resolves the merged spellings.
        let resolved = tags
            .resolve_tags(
                user,
                vec![TagWrite {
                    tags: s(&["JS", "k8s"]),
                    keep: vec![],
                }],
            )
            .await
            .unwrap();
        assert_eq!(resolved, vec![s(&["javascript", "Kubernetes"])]);
        let remaining = svc.review(user).await.unwrap().rows;
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].canonical, "remote work");

        // Undo puts every bookmark and proposal back.
        svc.undo(user, batch.id).await.expect("undo");
        assert_eq!(tags_by_id(&pool, user).await, before);
        assert_eq!(svc.review(user).await.unwrap().rows.len(), 5);

        cleanup(&pool, user).await;
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires a Postgres database in DATABASE_URL"]
    async fn a_rejected_key_fails_the_run_with_a_message_and_no_proposals() {
        let pool = connect().await;
        let user = new_user(&pool).await;
        bookmark(&pool, user, &["js", "javascript"]).await;
        let (svc, _) = service(&pool, user, true).await;
        let run_id = PostgresPool::new(pool.clone())
            .start_job(user, STALE_RUN_MINUTES)
            .await
            .unwrap()
            .unwrap();
        svc.run(user, run_id).await;
        let job = svc.job(user).await.unwrap().unwrap();
        assert_eq!(job.status, JobStatus::Failed);
        let error = job.error.unwrap();
        assert!(error.contains("rejected"), "{error}");
        assert!(
            !error.contains("401") && !error.contains("sk-ant"),
            "{error}"
        );
        assert!(svc.review(user).await.unwrap().rows.is_empty());
        cleanup(&pool, user).await;
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires a Postgres database in DATABASE_URL"]
    async fn a_spelling_that_appears_after_the_page_loaded_is_not_merged() {
        let pool = connect().await;
        let user = new_user(&pool).await;
        bookmark(&pool, user, &["javascript"]).await;
        let b_js = bookmark(&pool, user, &["js"]).await;
        let (svc, _) = service(&pool, user, false).await;
        let repo = PostgresPool::new(pool.clone());
        let run_id = repo
            .start_job(user, STALE_RUN_MINUTES)
            .await
            .unwrap()
            .unwrap();
        svc.run(user, run_id).await;
        let page = svc.review(user).await.unwrap().rows;
        let row = checked(&page, "javascript", ProposalSource::Llm, None);
        assert_eq!(row.shown, ["javascript", "js"]);

        // Another client adds `JS`, and another tab finds it as a spelling
        // variant of `js`, which the old row would now also merge.
        let b_cased = bookmark(&pool, user, &["JS"]).await;
        svc.propose_normalization(user).await.unwrap();
        let error = svc
            .apply(user, std::slice::from_ref(&row))
            .await
            .unwrap_err();
        assert!(matches!(error, DomainError::InvalidInput(ref m) if m == LIST_CHANGED));
        let tags = tags_by_id(&pool, user).await;
        assert_eq!(tags[&b_js], ["js"]);
        assert_eq!(tags[&b_cased], ["JS"]);

        // After a reload the row shows `JS` and applying it merges it.
        let page = svc.review(user).await.unwrap().rows;
        let row = checked(&page, "javascript", ProposalSource::Llm, None);
        assert_eq!(row.shown, ["javascript", "js", "JS"]);
        svc.apply(user, &[row]).await.expect("apply after reload");
        let tags = tags_by_id(&pool, user).await;
        assert_eq!(tags[&b_js], ["javascript"]);
        assert_eq!(tags[&b_cased], ["javascript"]);

        cleanup(&pool, user).await;
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires a Postgres database in DATABASE_URL"]
    async fn a_replaced_run_cannot_store_its_proposals() {
        let pool = connect().await;
        let user = new_user(&pool).await;
        bookmark(&pool, user, &["javascript"]).await;
        bookmark(&pool, user, &["js"]).await;
        let (svc, tags) = service(&pool, user, false).await;
        let repo = PostgresPool::new(pool.clone());
        let old = repo
            .start_job(user, STALE_RUN_MINUTES)
            .await
            .unwrap()
            .unwrap();
        sqlx::query(
            "UPDATE tag_consolidation_jobs SET started_at = now() - interval '2 hours'
             WHERE user_id = $1",
        )
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
        let new = repo
            .start_job(user, STALE_RUN_MINUTES)
            .await
            .unwrap()
            .unwrap();

        // The old worker finishes late: it stores and records nothing.
        svc.run(user, old).await;
        let llm_rows = |rows: Vec<crate::domain::tag_alias::TagMergeProposal>| {
            rows.into_iter()
                .filter(|p| p.source == ProposalSource::Llm)
                .count()
        };
        assert_eq!(llm_rows(tags.list_proposals(user, None).await.unwrap()), 0);
        let job = svc.job(user).await.unwrap().unwrap();
        assert_eq!((job.run_id, job.status), (new, JobStatus::Running));

        svc.run(user, new).await;
        assert_eq!(llm_rows(tags.list_proposals(user, None).await.unwrap()), 1);
        assert_eq!(
            svc.job(user).await.unwrap().unwrap().status,
            JobStatus::Succeeded
        );

        cleanup(&pool, user).await;
        pool.close().await;
    }
}
