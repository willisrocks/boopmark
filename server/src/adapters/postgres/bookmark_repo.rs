use super::PostgresPool;
use crate::domain::bookmark::*;
use crate::domain::error::DomainError;
use crate::domain::ports::bookmark_repo::{
    BookmarkRepository, CreateIdempotency, CreateIdempotencyClaim,
};
use uuid::Uuid;

impl BookmarkRepository for PostgresPool {
    async fn create(&self, user_id: Uuid, input: CreateBookmark) -> Result<Bookmark, DomainError> {
        let tags = input.tags.unwrap_or_default();
        sqlx::query_as::<_, Bookmark>(
            "INSERT INTO bookmarks (user_id, url, title, description, image_url, domain, tags)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             RETURNING id, user_id, url, title, description, image_url, override_image_url, domain, tags, created_at, updated_at",
        )
        .bind(user_id)
        .bind(&input.url)
        .bind(&input.title)
        .bind(&input.description)
        .bind(&input.image_url)
        .bind(&input.domain)
        .bind(&tags)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))
    }

    async fn claim_create(
        &self,
        user_id: Uuid,
        operation: CreateIdempotency,
    ) -> Result<CreateIdempotencyClaim, DomainError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| DomainError::Internal(e.to_string()))?;

        let acquired = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO bookmark_create_operations
                (user_id, idempotency_key, fingerprint_version, fingerprint, state)
             VALUES ($1, $2, $3, $4, 'pending')
             ON CONFLICT (user_id, idempotency_key) DO NOTHING
             RETURNING idempotency_key",
        )
        .bind(user_id)
        .bind(operation.key)
        .bind(operation.fingerprint_version)
        .bind(&operation.fingerprint)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?;

        if acquired.is_some() {
            tx.commit()
                .await
                .map_err(|e| DomainError::Internal(e.to_string()))?;
            return Ok(CreateIdempotencyClaim::Acquired);
        }

        let existing = sqlx::query_as::<_, (i16, String, String, Option<Uuid>)>(
            "SELECT fingerprint_version, fingerprint, state, bookmark_id
             FROM bookmark_create_operations
             WHERE user_id = $1 AND idempotency_key = $2
             FOR UPDATE",
        )
        .bind(user_id)
        .bind(operation.key)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?;

        let Some((fingerprint_version, fingerprint, state, bookmark_id)) = existing else {
            return Err(DomainError::Internal(
                "idempotency operation disappeared".to_string(),
            ));
        };
        if fingerprint_version != operation.fingerprint_version
            || fingerprint != operation.fingerprint
        {
            tx.commit()
                .await
                .map_err(|e| DomainError::Internal(e.to_string()))?;
            return Ok(CreateIdempotencyClaim::Conflict);
        }

        match state.as_str() {
            "pending" => {
                tx.commit()
                    .await
                    .map_err(|e| DomainError::Internal(e.to_string()))?;
                Ok(CreateIdempotencyClaim::Pending)
            }
            "completed" => {
                let Some(bookmark_id) = bookmark_id else {
                    return Err(DomainError::Internal(
                        "completed idempotency operation has no bookmark".to_string(),
                    ));
                };
                let bookmark = sqlx::query_as::<_, Bookmark>(
                    "SELECT id, user_id, url, title, description, image_url,
                            override_image_url, domain, tags, created_at, updated_at
                     FROM bookmarks
                     WHERE id = $1 AND user_id = $2",
                )
                .bind(bookmark_id)
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|e| DomainError::Internal(e.to_string()))?
                .ok_or_else(|| {
                    DomainError::Internal("idempotency bookmark disappeared".to_string())
                })?;
                tx.commit()
                    .await
                    .map_err(|e| DomainError::Internal(e.to_string()))?;
                Ok(CreateIdempotencyClaim::Completed(Box::new(bookmark)))
            }
            _ => Err(DomainError::Internal(
                "invalid idempotency operation state".to_string(),
            )),
        }
    }

    async fn create_claimed(
        &self,
        user_id: Uuid,
        input: CreateBookmark,
        operation: CreateIdempotency,
    ) -> Result<Bookmark, DomainError> {
        let tags = input.tags.unwrap_or_default();
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| DomainError::Internal(e.to_string()))?;
        let bookmark = sqlx::query_as::<_, Bookmark>(
            "INSERT INTO bookmarks
                (user_id, url, title, description, image_url, domain, tags)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             RETURNING id, user_id, url, title, description, image_url,
                       override_image_url, domain, tags, created_at, updated_at",
        )
        .bind(user_id)
        .bind(&input.url)
        .bind(&input.title)
        .bind(&input.description)
        .bind(&input.image_url)
        .bind(&input.domain)
        .bind(&tags)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?;

        let finalized = sqlx::query_scalar::<_, Uuid>(
            "UPDATE bookmark_create_operations
             SET state = 'completed', bookmark_id = $1, updated_at = now()
             WHERE user_id = $2
               AND idempotency_key = $3
               AND fingerprint_version = $4
               AND fingerprint = $5
               AND state = 'pending'
             RETURNING idempotency_key",
        )
        .bind(bookmark.id)
        .bind(user_id)
        .bind(operation.key)
        .bind(operation.fingerprint_version)
        .bind(&operation.fingerprint)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?;
        if finalized.is_none() {
            return Err(DomainError::Internal(
                "idempotency operation could not be finalized".to_string(),
            ));
        }
        tx.commit()
            .await
            .map_err(|e| DomainError::Internal(e.to_string()))?;
        Ok(bookmark)
    }

    async fn get(&self, id: Uuid, user_id: Uuid) -> Result<Bookmark, DomainError> {
        sqlx::query_as::<_, Bookmark>(
            "SELECT id, user_id, url, title, description, image_url, override_image_url, domain, tags, created_at, updated_at
             FROM bookmarks WHERE id = $1 AND user_id = $2",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?
        .ok_or(DomainError::NotFound)
    }

    async fn list(
        &self,
        user_id: Uuid,
        filter: BookmarkFilter,
    ) -> Result<Vec<Bookmark>, DomainError> {
        let limit = filter.limit.unwrap_or(50);
        let offset = filter.offset.unwrap_or(0);

        let order_clause = match filter.sort.unwrap_or_default() {
            BookmarkSort::Newest => "created_at DESC",
            BookmarkSort::Oldest => "created_at ASC",
            BookmarkSort::Title => "title ASC NULLS LAST",
            BookmarkSort::Domain => "domain ASC NULLS LAST",
        };

        // Build dynamic query since ORDER BY can't be parameterized
        let mut sql = String::from(
            "SELECT id, user_id, url, title, description, image_url, override_image_url, domain, tags, created_at, updated_at FROM bookmarks WHERE user_id = $1",
        );
        let mut param_idx = 2;

        if filter.search.is_some() {
            sql.push_str(&format!(
                " AND to_tsvector('english', coalesce(title, '') || ' ' || coalesce(description, '') || ' ' || url) @@ plainto_tsquery('english', ${param_idx})"
            ));
            param_idx += 1;
        }

        if filter.tags.is_some() {
            sql.push_str(&format!(" AND tags && ${param_idx}"));
            param_idx += 1;
        }

        sql.push_str(&format!(
            " ORDER BY {order_clause} LIMIT ${param_idx} OFFSET ${}",
            param_idx + 1
        ));

        let mut query = sqlx::query_as::<_, Bookmark>(&sql).bind(user_id);

        if let Some(ref search) = filter.search {
            query = query.bind(search);
        }
        if let Some(ref tags) = filter.tags {
            query = query.bind(tags);
        }

        query = query.bind(limit).bind(offset);

        query
            .fetch_all(&self.pool)
            .await
            .map_err(|e| DomainError::Internal(e.to_string()))
    }

    async fn update(
        &self,
        id: Uuid,
        user_id: Uuid,
        input: UpdateBookmark,
    ) -> Result<Bookmark, DomainError> {
        sqlx::query_as::<_, Bookmark>(
            "UPDATE bookmarks SET
                title = CASE WHEN $3 = '' THEN NULL ELSE COALESCE($3, title) END,
                description = CASE WHEN $4 = '' THEN NULL ELSE COALESCE($4, description) END,
                tags = COALESCE($5, tags),
                updated_at = now()
             WHERE id = $1 AND user_id = $2
             RETURNING id, user_id, url, title, description, image_url, override_image_url, domain, tags, created_at, updated_at",
        )
        .bind(id)
        .bind(user_id)
        .bind(&input.title)
        .bind(&input.description)
        .bind(input.tags.as_deref())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?
        .ok_or(DomainError::NotFound)
    }

    async fn delete(&self, id: Uuid, user_id: Uuid) -> Result<(), DomainError> {
        let result = sqlx::query("DELETE FROM bookmarks WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_err(|e| DomainError::Internal(e.to_string()))?;

        if result.rows_affected() == 0 {
            return Err(DomainError::NotFound);
        }
        Ok(())
    }

    async fn all_tags(&self, user_id: Uuid) -> Result<Vec<String>, DomainError> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT DISTINCT unnest(tags) AS tag FROM bookmarks WHERE user_id = $1 ORDER BY tag",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?;

        Ok(rows.into_iter().map(|(t,)| t).collect())
    }

    async fn tags_with_counts(&self, user_id: Uuid) -> Result<Vec<(String, i64)>, DomainError> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT unnest(tags) AS tag, COUNT(*) AS count FROM bookmarks WHERE user_id = $1 GROUP BY tag ORDER BY count DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?;

        Ok(rows)
    }

    async fn export_all(&self, user_id: Uuid) -> Result<Vec<Bookmark>, DomainError> {
        sqlx::query_as::<_, Bookmark>(
            "SELECT id, user_id, url, title, description, image_url, override_image_url, domain, tags, created_at, updated_at
             FROM bookmarks WHERE user_id = $1 ORDER BY created_at DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))
    }

    async fn find_by_url(&self, user_id: Uuid, url: &str) -> Result<Option<Bookmark>, DomainError> {
        sqlx::query_as::<_, Bookmark>(
            "SELECT id, user_id, url, title, description, image_url, override_image_url, domain, tags, created_at, updated_at
             FROM bookmarks WHERE user_id = $1 AND url = $2 ORDER BY created_at ASC, id ASC LIMIT 1",
        )
        .bind(user_id)
        .bind(url)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))
    }

    async fn insert_with_id(&self, bookmark: Bookmark) -> Result<Bookmark, DomainError> {
        sqlx::query_as::<_, Bookmark>(
            "INSERT INTO bookmarks (id, user_id, url, title, description, image_url, override_image_url, domain, tags, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
             RETURNING id, user_id, url, title, description, image_url, override_image_url, domain, tags, created_at, updated_at",
        )
        .bind(bookmark.id)
        .bind(bookmark.user_id)
        .bind(&bookmark.url)
        .bind(&bookmark.title)
        .bind(&bookmark.description)
        .bind(&bookmark.image_url)
        .bind(&bookmark.override_image_url)
        .bind(&bookmark.domain)
        .bind(&bookmark.tags)
        .bind(bookmark.created_at)
        .bind(bookmark.updated_at)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            // Unique constraint violation (PK collision from another user's row)
            if let sqlx::Error::Database(ref db_err) = e
                && db_err.code().as_deref() == Some("23505") {
                    return DomainError::AlreadyExists;
                }
            DomainError::Internal(e.to_string())
        })
    }

    async fn upsert_full(&self, bookmark: Bookmark) -> Result<Bookmark, DomainError> {
        sqlx::query_as::<_, Bookmark>(
            "INSERT INTO bookmarks (id, user_id, url, title, description, image_url, override_image_url, domain, tags, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
             ON CONFLICT (id) DO UPDATE SET
                url = EXCLUDED.url,
                title = EXCLUDED.title,
                description = EXCLUDED.description,
                image_url = EXCLUDED.image_url,
                domain = EXCLUDED.domain,
                tags = EXCLUDED.tags,
                created_at = EXCLUDED.created_at,
                updated_at = EXCLUDED.updated_at
             WHERE bookmarks.user_id = $2
             RETURNING id, user_id, url, title, description, image_url, override_image_url, domain, tags, created_at, updated_at",
        )
        .bind(bookmark.id)
        .bind(bookmark.user_id)
        .bind(&bookmark.url)
        .bind(&bookmark.title)
        .bind(&bookmark.description)
        .bind(&bookmark.image_url)
        .bind(&bookmark.override_image_url)
        .bind(&bookmark.domain)
        .bind(&bookmark.tags)
        .bind(bookmark.created_at)
        .bind(bookmark.updated_at)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            // RowNotFound means ON CONFLICT fired but the WHERE clause blocked
            // the update — the ID belongs to another user. Treat as a PK
            // collision to be handled as a row-level error by the caller.
            if matches!(e, sqlx::Error::RowNotFound) {
                return DomainError::AlreadyExists;
            }
            DomainError::Internal(e.to_string())
        })
    }

    async fn update_image_url(
        &self,
        id: Uuid,
        user_id: Uuid,
        image_url: &str,
    ) -> Result<(), DomainError> {
        let result = sqlx::query(
            "UPDATE bookmarks SET image_url = $1, updated_at = now() \
             WHERE id = $2 AND user_id = $3",
        )
        .bind(image_url)
        .bind(id)
        .bind(user_id)
        .execute(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?;

        if result.rows_affected() == 0 {
            Err(DomainError::NotFound)
        } else {
            Ok(())
        }
    }

    async fn replace_override_image_url(
        &self,
        id: Uuid,
        user_id: Uuid,
        image_url: Option<&str>,
    ) -> Result<Option<String>, DomainError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| DomainError::Internal(e.to_string()))?;
        let old = sqlx::query_scalar::<_, Option<String>>(
            "SELECT override_image_url FROM bookmarks WHERE id = $1 AND user_id = $2 FOR UPDATE",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?
        .ok_or(DomainError::NotFound)?;

        sqlx::query(
            "UPDATE bookmarks SET override_image_url = $1, updated_at = now()
             WHERE id = $2 AND user_id = $3",
        )
        .bind(image_url)
        .bind(id)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?;
        tx.commit()
            .await
            .map_err(|e| DomainError::Internal(e.to_string()))?;
        Ok(old)
    }

    async fn delete_with_override(
        &self,
        id: Uuid,
        user_id: Uuid,
    ) -> Result<Option<String>, DomainError> {
        let old = sqlx::query_scalar::<_, Option<String>>(
            "DELETE FROM bookmarks WHERE id = $1 AND user_id = $2 RETURNING override_image_url",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?
        .ok_or(DomainError::NotFound)?;
        Ok(old)
    }

    async fn tag_samples(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<crate::domain::ports::tag_consolidator::TagSample>, DomainError> {
        use crate::domain::ports::tag_consolidator::TagSample;

        let rows: Vec<(String, i64, Vec<String>)> = sqlx::query_as(
            "WITH expanded AS (
                 SELECT id, title, created_at, unnest(tags) AS tag
                 FROM bookmarks
                 WHERE user_id = $1
             ),
             counts AS (
                 SELECT tag, COUNT(*) AS count
                 FROM expanded
                 GROUP BY tag
             ),
             ranked AS (
                 SELECT
                     tag,
                     title,
                     ROW_NUMBER() OVER (PARTITION BY tag ORDER BY created_at DESC, id DESC) AS rn
                 FROM expanded
                 WHERE title IS NOT NULL AND title <> ''
             )
             SELECT
                 c.tag,
                 c.count,
                 COALESCE(
                     ARRAY_AGG(r.title ORDER BY r.rn) FILTER (WHERE r.rn IS NOT NULL),
                     ARRAY[]::TEXT[]
                 ) AS sample_titles
             FROM counts c
             LEFT JOIN ranked r ON r.tag = c.tag AND r.rn <= 3
             GROUP BY c.tag, c.count
             ORDER BY c.count DESC, c.tag ASC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|(tag, count, sample_titles)| TagSample {
                tag,
                count,
                sample_titles,
            })
            .collect())
    }

    async fn list_id_tags(&self, user_id: Uuid) -> Result<Vec<(Uuid, Vec<String>)>, DomainError> {
        let rows: Vec<(Uuid, Vec<String>)> =
            sqlx::query_as("SELECT id, tags FROM bookmarks WHERE user_id = $1")
                .bind(user_id)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| DomainError::Internal(e.to_string()))?;
        Ok(rows)
    }

    async fn update_tags_bulk(
        &self,
        user_id: Uuid,
        updates: &[(Uuid, Vec<String>)],
    ) -> Result<u64, DomainError> {
        if updates.is_empty() {
            return Ok(0);
        }

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| DomainError::Internal(e.to_string()))?;

        let mut rows: u64 = 0;
        for (id, new_tags) in updates {
            let r = sqlx::query(
                "UPDATE bookmarks
                 SET tags = $1, updated_at = now()
                 WHERE id = $2 AND user_id = $3",
            )
            .bind(new_tags)
            .bind(id)
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| DomainError::Internal(e.to_string()))?;
            rows += r.rows_affected();
        }

        tx.commit()
            .await
            .map_err(|e| DomainError::Internal(e.to_string()))?;
        Ok(rows)
    }
}

#[cfg(test)]
mod idempotency_tests {
    use super::*;
    use crate::domain::ports::bookmark_repo::BookmarkRepository;
    use sqlx::postgres::PgPoolOptions;
    use std::env;

    fn input(url: &str, title: &str) -> CreateBookmark {
        CreateBookmark {
            url: url.to_string(),
            title: Some(title.to_string()),
            description: Some("durable operation test".to_string()),
            image_url: None,
            domain: Some("example.com".to_string()),
            tags: Some(vec!["idempotency".to_string()]),
        }
    }

    fn operation(key: Uuid, fingerprint: &str) -> CreateIdempotency {
        CreateIdempotency {
            key,
            fingerprint_version: 1,
            fingerprint: fingerprint.to_string(),
        }
    }

    /// This test is intentionally backed by the same Postgres URL used by the
    /// dedicated E2E server. Run it explicitly with `--ignored` after starting
    /// devproxy; it must never report a silent pass without a database.
    #[tokio::test]
    #[ignore = "requires the devproxy-managed migrated Postgres database"]
    async fn durable_claims_are_atomic_scoped_and_headerless_creates_remain_distinct() {
        let database_url = env::var("DATABASE_URL")
            .expect("DATABASE_URL is required for the ignored Postgres idempotency test");
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(&database_url)
            .await
            .expect("DATABASE_URL must point at the migrated E2E database");
        let repo = PostgresPool::new(pool.clone());
        let user_a = Uuid::new_v4();
        let user_b = Uuid::new_v4();
        let email_a = format!("idempotency-{user_a}@example.test");
        let email_b = format!("idempotency-{user_b}@example.test");

        for (user_id, email) in [(user_a, &email_a), (user_b, &email_b)] {
            sqlx::query("INSERT INTO users (id, email, name) VALUES ($1, $2, $3)")
                .bind(user_id)
                .bind(email)
                .bind("idempotency test")
                .execute(&pool)
                .await
                .expect("test users can be inserted");
        }

        let key = Uuid::new_v4();
        let op = operation(key, "reviewed-payload-a");

        // Two transactions claiming a fresh key concurrently produce exactly
        // one owner. The loser observes pending and must not create anything.
        let (left, right) = tokio::join!(
            repo.claim_create(user_a, op.clone()),
            repo.claim_create(user_a, op.clone()),
        );
        let claims = [left.expect("first claim"), right.expect("second claim")];
        assert_eq!(
            claims
                .iter()
                .filter(|claim| matches!(claim, CreateIdempotencyClaim::Acquired))
                .count(),
            1
        );
        assert_eq!(
            claims
                .iter()
                .filter(|claim| matches!(claim, CreateIdempotencyClaim::Pending))
                .count(),
            1
        );

        let created = repo
            .create_claimed(
                user_a,
                input("https://example.com/idempotent-a", "A"),
                op.clone(),
            )
            .await
            .expect("acquired claim can be finalized");

        // A sequential replay returns the original row, while changing the
        // reviewed payload is a conflict before any bookmark insert occurs.
        match repo
            .claim_create(user_a, op.clone())
            .await
            .expect("completed replay claim")
        {
            CreateIdempotencyClaim::Completed(replayed) => assert_eq!(replayed.id, created.id),
            other => panic!("expected completed replay, got {other:?}"),
        }
        let before_conflict: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM bookmarks WHERE user_id = $1 AND url = $2")
                .bind(user_a)
                .bind("https://example.com/idempotent-a")
                .fetch_one(&pool)
                .await
                .expect("count before conflict");
        assert!(matches!(
            repo.claim_create(user_a, operation(key, "different-reviewed-payload"))
                .await
                .expect("mismatch claim"),
            CreateIdempotencyClaim::Conflict
        ));
        let after_conflict: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM bookmarks WHERE user_id = $1 AND url = $2")
                .bind(user_a)
                .bind("https://example.com/idempotent-a")
                .fetch_one(&pool)
                .await
                .expect("count after conflict");
        assert_eq!(before_conflict, after_conflict);

        // The same UUID is independent for another account (tenant scope).
        assert!(matches!(
            repo.claim_create(user_b, op.clone())
                .await
                .expect("other-user claim"),
            CreateIdempotencyClaim::Acquired
        ));
        let other = repo
            .create_claimed(
                user_b,
                input("https://example.com/idempotent-b", "B"),
                op.clone(),
            )
            .await
            .expect("other-user finalize");
        assert_ne!(created.id, other.id);
        assert!(matches!(
            repo.claim_create(user_b, op)
                .await
                .expect("other-user replay"),
            CreateIdempotencyClaim::Completed(_)
        ));

        // Headerless callers bypass the operation table and retain the
        // historical behavior that duplicate URLs are ordinary new rows.
        let first = repo
            .create(user_a, input("https://example.com/headerless", "one"))
            .await
            .expect("first headerless create");
        let second = repo
            .create(user_a, input("https://example.com/headerless", "one"))
            .await
            .expect("second headerless create");
        assert_ne!(first.id, second.id);

        sqlx::query("DELETE FROM users WHERE id IN ($1, $2)")
            .bind(user_a)
            .bind(user_b)
            .execute(&pool)
            .await
            .expect("cleanup idempotency test users");
        pool.close().await;
    }
}
