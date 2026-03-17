use super::PostgresPool;
use crate::domain::bookmark::*;
use crate::domain::error::DomainError;
use crate::domain::ports::bookmark_repo::BookmarkRepository;
use uuid::Uuid;

impl BookmarkRepository for PostgresPool {
    async fn create(&self, user_id: Uuid, input: CreateBookmark) -> Result<Bookmark, DomainError> {
        let tags = input.tags.unwrap_or_default();
        sqlx::query_as::<_, Bookmark>(
            "INSERT INTO bookmarks (user_id, url, title, description, image_url, domain, tags)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             RETURNING id, user_id, url, title, description, image_url, domain, tags, created_at, updated_at",
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

    async fn get(&self, id: Uuid, user_id: Uuid) -> Result<Bookmark, DomainError> {
        sqlx::query_as::<_, Bookmark>(
            "SELECT id, user_id, url, title, description, image_url, domain, tags, created_at, updated_at
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
            "SELECT id, user_id, url, title, description, image_url, domain, tags, created_at, updated_at FROM bookmarks WHERE user_id = $1",
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
             RETURNING id, user_id, url, title, description, image_url, domain, tags, created_at, updated_at",
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
            "SELECT id, user_id, url, title, description, image_url, domain, tags, created_at, updated_at
             FROM bookmarks WHERE user_id = $1 ORDER BY created_at DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))
    }

    async fn find_by_url(
        &self,
        user_id: Uuid,
        url: &str,
    ) -> Result<Option<Bookmark>, DomainError> {
        sqlx::query_as::<_, Bookmark>(
            "SELECT id, user_id, url, title, description, image_url, domain, tags, created_at, updated_at
             FROM bookmarks WHERE user_id = $1 AND url = $2",
        )
        .bind(user_id)
        .bind(url)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))
    }

    async fn insert_with_id(&self, bookmark: Bookmark) -> Result<Bookmark, DomainError> {
        sqlx::query_as::<_, Bookmark>(
            "INSERT INTO bookmarks (id, user_id, url, title, description, image_url, domain, tags, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             RETURNING id, user_id, url, title, description, image_url, domain, tags, created_at, updated_at",
        )
        .bind(bookmark.id)
        .bind(bookmark.user_id)
        .bind(&bookmark.url)
        .bind(&bookmark.title)
        .bind(&bookmark.description)
        .bind(&bookmark.image_url)
        .bind(&bookmark.domain)
        .bind(&bookmark.tags)
        .bind(bookmark.created_at)
        .bind(bookmark.updated_at)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))
    }

    async fn upsert_full(&self, bookmark: Bookmark) -> Result<Bookmark, DomainError> {
        sqlx::query_as::<_, Bookmark>(
            "INSERT INTO bookmarks (id, user_id, url, title, description, image_url, domain, tags, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
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
             RETURNING id, user_id, url, title, description, image_url, domain, tags, created_at, updated_at",
        )
        .bind(bookmark.id)
        .bind(bookmark.user_id)
        .bind(&bookmark.url)
        .bind(&bookmark.title)
        .bind(&bookmark.description)
        .bind(&bookmark.image_url)
        .bind(&bookmark.domain)
        .bind(&bookmark.tags)
        .bind(bookmark.created_at)
        .bind(bookmark.updated_at)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))
    }
}
