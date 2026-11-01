use super::PostgresPool;
use crate::domain::error::DomainError;
use crate::domain::ports::tag_consolidation_job_repo::TagConsolidationJobRepository;
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
