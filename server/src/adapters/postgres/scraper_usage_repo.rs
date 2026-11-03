use super::PostgresPool;
use crate::domain::error::DomainError;
use crate::domain::ports::scraper_usage_repo::{BoxFuture, ScraperUsageRepository};

fn db_error(e: sqlx::Error) -> DomainError {
    DomainError::Internal(e.to_string())
}

/// The current month's row is keyed by the first day of the UTC month, so a
/// restart or a deploy continues the same spend.
const THIS_MONTH: &str = "date_trunc('month', now() AT TIME ZONE 'utc')::date";

impl ScraperUsageRepository for PostgresPool {
    fn reserve_page(&self, budget: u64) -> BoxFuture<'_, Result<bool, DomainError>> {
        // One statement reserves the page: the insert of a month's first page
        // and the capped increment of a later one both hold the row's lock,
        // so concurrent workers see each other's reservations and the cap is
        // never exceeded. `WHERE` on `DO UPDATE` is what turns "the month is
        // full" into no row rather than a count the caller would have to undo.
        Box::pin(async move {
            let budget = i64::try_from(budget).unwrap_or(i64::MAX);
            let pages: Option<i64> = sqlx::query_scalar(&format!(
                "INSERT INTO scraper_usage (month, pages) VALUES ({THIS_MONTH}, 1)
                 ON CONFLICT (month) DO UPDATE
                 SET pages = scraper_usage.pages + 1, updated_at = now()
                 WHERE $1 = 0 OR scraper_usage.pages < $1
                 RETURNING pages"
            ))
            .bind(budget)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_error)?;
            Ok(pages.is_some())
        })
    }

    fn record_failure(&self, billed: bool) -> BoxFuture<'_, Result<(), DomainError>> {
        // A billed page stays counted: the vendor charged for it even though
        // the result was unusable. A request that never got a 2xx is
        // refunded; `greatest` keeps the count from going negative if a
        // failure is ever recorded without a matching reservation.
        let statement = if billed {
            format!(
                "INSERT INTO scraper_usage (month, failures) VALUES ({THIS_MONTH}, 1)
                 ON CONFLICT (month) DO UPDATE
                 SET failures = scraper_usage.failures + 1, updated_at = now()"
            )
        } else {
            format!(
                "INSERT INTO scraper_usage (month, pages, failures) VALUES ({THIS_MONTH}, 0, 1)
                 ON CONFLICT (month) DO UPDATE
                 SET pages = greatest(0, scraper_usage.pages - 1),
                     failures = scraper_usage.failures + 1,
                     updated_at = now()"
            )
        };
        Box::pin(async move {
            sqlx::query(&statement)
                .execute(&self.pool)
                .await
                .map_err(db_error)?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod scraper_usage_postgres_tests {
    use super::*;
    use crate::adapters::postgres::search_repo::search_tests::{SERIAL, pool};
    use std::sync::Arc;

    async fn usage(pool: &sqlx::PgPool) -> (i64, i64) {
        sqlx::query_as(&format!(
            "SELECT pages, failures FROM scraper_usage WHERE month = {THIS_MONTH}"
        ))
        .fetch_optional(pool)
        .await
        .unwrap()
        .unwrap_or((0, 0))
    }

    async fn clear(pool: &sqlx::PgPool) {
        sqlx::query(&format!(
            "DELETE FROM scraper_usage WHERE month = {THIS_MONTH}"
        ))
        .execute(pool)
        .await
        .unwrap();
    }

    /// The cap is enforced under concurrency: ten workers reserving against a
    /// budget of two get two reservations, and the month's count is two.
    #[tokio::test]
    #[ignore = "requires a migrated Postgres database (DATABASE_URL)"]
    async fn concurrent_reservations_never_exceed_the_monthly_budget() {
        let _serial = SERIAL.lock().await;
        let pool = pool().await;
        clear(&pool).await;
        let repo = Arc::new(PostgresPool::new(pool.clone()));

        let mut tasks = Vec::new();
        for _ in 0..10 {
            let repo = repo.clone();
            tasks.push(tokio::spawn(
                async move { repo.reserve_page(2).await.unwrap() },
            ));
        }
        let mut reserved = 0;
        for task in tasks {
            reserved += usize::from(task.await.unwrap());
        }
        assert_eq!(reserved, 2);
        assert_eq!(usage(&pool).await, (2, 0), "the count stopped at the cap");

        // A budget of 0 is no cap.
        clear(&pool).await;
        for _ in 0..5 {
            assert!(repo.reserve_page(0).await.unwrap());
        }
        assert_eq!(usage(&pool).await, (5, 0));

        // A request that never got a 2xx is refunded and counted as a failure.
        repo.record_failure(false).await.unwrap();
        assert_eq!(usage(&pool).await, (4, 1));

        // A page the vendor billed for stays counted, whatever came of it.
        repo.record_failure(true).await.unwrap();
        assert_eq!(usage(&pool).await, (4, 2));

        clear(&pool).await;
        pool.close().await;
    }
}
