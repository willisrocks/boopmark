use super::PostgresPool;
use crate::domain::error::DomainError;
use crate::domain::ports::page_capture_repo::{
    CAPTURE_QUEUE_BYTES, CAPTURE_QUEUE_ROWS, CaptureQueueRepository, NewCapture, QueuedCapture,
};
use std::time::Duration;
use uuid::Uuid;

fn db_error(e: sqlx::Error) -> DomainError {
    DomainError::Internal(e.to_string())
}

impl CaptureQueueRepository for PostgresPool {
    async fn enqueue_capture(&self, capture: &NewCapture) -> Result<bool, DomainError> {
        // The capacity check and the insert are one statement, so two
        // captures cannot both see room for the last slot. A re-capture of
        // the same bookmark replaces its payload and does not count twice.
        let queued: Option<Uuid> = sqlx::query_scalar(
            "INSERT INTO bookmark_page_captures (bookmark_id, user_id, page_url, html_gz)
             SELECT $1, $2, $3, $4
             WHERE (SELECT count(*) FROM bookmark_page_captures
                    WHERE bookmark_id <> $1) < $5
               AND (SELECT coalesce(sum(length(html_gz)), 0) FROM bookmark_page_captures
                    WHERE bookmark_id <> $1) + length($4::bytea) <= $6
             ON CONFLICT (bookmark_id) DO UPDATE SET
                 user_id = EXCLUDED.user_id,
                 page_url = EXCLUDED.page_url,
                 html_gz = EXCLUDED.html_gz,
                 created_at = now()
             RETURNING bookmark_id",
        )
        .bind(capture.bookmark_id)
        .bind(capture.user_id)
        .bind(&capture.page_url)
        .bind(&capture.html_gz)
        .bind(CAPTURE_QUEUE_ROWS)
        .bind(CAPTURE_QUEUE_BYTES)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(queued.is_some())
    }

    async fn claim_captures(&self, limit: i64) -> Result<Vec<QueuedCapture>, DomainError> {
        // Deleting the row in the claim makes each payload parse exactly once
        // and keeps no lock held across the parse. A worker that dies with
        // payloads in hand loses them, which the queue's 24 h expiry would
        // have done anyway.
        sqlx::query_as::<_, QueuedCapture>(
            "DELETE FROM bookmark_page_captures
             WHERE bookmark_id IN (
                 SELECT bookmark_id FROM bookmark_page_captures
                 ORDER BY created_at, bookmark_id
                 LIMIT $1
                 FOR UPDATE SKIP LOCKED
             )
             RETURNING bookmark_id, user_id, page_url, html_gz",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)
    }

    async fn record_capture_failure(
        &self,
        bookmark_id: Uuid,
        error: &str,
    ) -> Result<(), DomainError> {
        sqlx::query(
            "UPDATE bookmark_contents SET error = $2, updated_at = now()
             WHERE bookmark_id = $1",
        )
        .bind(bookmark_id)
        .bind(error)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn purge_stale_captures(&self, max_age: Duration) -> Result<u64, DomainError> {
        let purged: i64 = sqlx::query_scalar(
            "WITH purged AS (
                 DELETE FROM bookmark_page_captures
                 WHERE created_at < now() - make_interval(secs => $1)
                 RETURNING bookmark_id
             )
             SELECT count(*) FROM purged",
        )
        .bind(max_age.as_secs_f64())
        .fetch_one(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(purged as u64)
    }
}
