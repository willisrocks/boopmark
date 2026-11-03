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

#[cfg(test)]
mod page_capture_postgres_tests {
    use super::*;
    use crate::adapters::postgres::search_repo::search_tests::{SERIAL, bookmark, pool, user};

    fn capture(bookmark_id: Uuid, user_id: Uuid, html: &[u8]) -> NewCapture {
        NewCapture {
            bookmark_id,
            user_id,
            page_url: "https://medium.com/@writer/a-post".into(),
            html_gz: html.to_vec(),
        }
    }

    async fn queued(pool: &sqlx::PgPool) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM bookmark_page_captures")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// Claiming removes the payload, so a second claim cannot parse it again;
    /// a re-capture replaces the waiting payload rather than adding one.
    #[tokio::test]
    #[ignore = "requires a migrated Postgres database (DATABASE_URL)"]
    async fn captures_are_claimed_once_replaced_and_purged() {
        let _serial = SERIAL.lock().await;
        let pool = pool().await;
        let repo = PostgresPool::new(pool.clone());
        let owner = user(&pool).await;
        let page = bookmark(
            &repo,
            owner,
            "https://medium.com/@writer/a-post",
            "A",
            None,
            &[],
        )
        .await;

        assert!(
            repo.enqueue_capture(&capture(page.id, owner, b"first gzip"))
                .await
                .unwrap()
        );
        assert!(
            repo.enqueue_capture(&capture(page.id, owner, b"second gzip"))
                .await
                .unwrap(),
            "a re-capture replaces the waiting payload"
        );
        assert_eq!(queued(&pool).await, 1);

        let claimed = repo.claim_captures(10).await.unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].bookmark_id, page.id);
        assert_eq!(claimed[0].user_id, owner);
        assert_eq!(claimed[0].html_gz, b"second gzip");
        assert_eq!(queued(&pool).await, 0, "claiming took the row");
        assert!(repo.claim_captures(10).await.unwrap().is_empty());

        // Fresh rows survive the sweep; a day-old one does not.
        assert!(
            repo.enqueue_capture(&capture(page.id, owner, b"gzip"))
                .await
                .unwrap()
        );
        assert_eq!(
            repo.purge_stale_captures(Duration::from_secs(24 * 60 * 60))
                .await
                .unwrap(),
            0
        );
        assert_eq!(queued(&pool).await, 1);
        sqlx::query("UPDATE bookmark_page_captures SET created_at = now() - interval '2 days'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            repo.purge_stale_captures(Duration::from_secs(24 * 60 * 60))
                .await
                .unwrap(),
            1
        );
        assert_eq!(queued(&pool).await, 0);

        // A failure is recorded on the bookmark without changing its state.
        repo.record_capture_failure(page.id, "no readable article")
            .await
            .unwrap();
        let (status, error): (String, Option<String>) =
            sqlx::query_as("SELECT status, error FROM bookmark_contents WHERE bookmark_id = $1")
                .bind(page.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "pending");
        assert_eq!(error.as_deref(), Some("no readable article"));

        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(owner)
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }

    /// The queue is full at 64 waiting payloads: past that, the route answers
    /// `503 busy` instead of growing the write surface.
    #[tokio::test]
    #[ignore = "requires a migrated Postgres database (DATABASE_URL)"]
    async fn a_full_queue_refuses_new_payloads() {
        let _serial = SERIAL.lock().await;
        let pool = pool().await;
        let repo = PostgresPool::new(pool.clone());
        let owner = user(&pool).await;
        // Only this module writes to the queue, and SERIAL holds the others
        // back; a row left by a crashed run would skew the capacity check.
        sqlx::query("DELETE FROM bookmark_page_captures")
            .execute(&pool)
            .await
            .unwrap();
        let mut pages = Vec::new();
        for n in 0..=CAPTURE_QUEUE_ROWS {
            pages.push(
                bookmark(
                    &repo,
                    owner,
                    &format!("https://example.org/{n}"),
                    "Page",
                    None,
                    &[],
                )
                .await,
            );
        }
        // Every slot but one.
        for page in pages.iter().take(CAPTURE_QUEUE_ROWS as usize - 1) {
            assert!(
                repo.enqueue_capture(&capture(page.id, owner, b"gzip"))
                    .await
                    .unwrap()
            );
        }
        // The last one fits ...
        let last = &pages[CAPTURE_QUEUE_ROWS as usize - 1];
        assert!(
            repo.enqueue_capture(&capture(last.id, owner, b"gzip"))
                .await
                .unwrap()
        );
        // ... and one more is refused.
        let overflow = pages.last().unwrap();
        assert!(
            !repo
                .enqueue_capture(&capture(overflow.id, owner, b"gzip"))
                .await
                .unwrap()
        );
        // A re-capture replaces its own waiting payload, so it is not refused.
        let first = &pages[0];
        assert!(
            repo.enqueue_capture(&capture(first.id, owner, b"gzip again"))
                .await
                .unwrap()
        );
        assert_eq!(queued(&pool).await, CAPTURE_QUEUE_ROWS);

        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(owner)
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }
}
