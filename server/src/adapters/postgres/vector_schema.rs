//! The vector leg's schema, created at startup instead of in a migration: a
//! managed database may not let the app role create the `vector` extension,
//! and the app must still boot and search lexically. Once the extension is
//! enabled, the next start creates the tables.

use sqlx::PgPool;

/// Whether the vector leg can run on this database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VectorStatus {
    Ready,
    Unavailable(String),
}

/// Idempotent: every statement is `IF NOT EXISTS`. Needs pgvector >= 0.8
/// (halfvec, hnsw.iterative_scan).
const VECTOR_DDL: &[&str] = &[
    // The embedded passages of each bookmark. An article of at least 300
    // characters is split into windows; anything else (a failed or skipped
    // fetch, a short page) has one chunk, ord 0, made of its metadata.
    "CREATE TABLE IF NOT EXISTS bookmark_chunks (
        bookmark_id UUID NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE,
        user_id     UUID NOT NULL,
        ord         INT  NOT NULL,
        text        TEXT NOT NULL,      -- the passage, shown as a snippet
        embedding   halfvec NOT NULL,   -- no fixed dimension: one schema for every backend
        model       TEXT NOT NULL,      -- Embedder::model_id() that produced this embedding
        PRIMARY KEY (bookmark_id, ord)
    )",
    "CREATE INDEX IF NOT EXISTS idx_bookmark_chunks_user ON bookmark_chunks(user_id)",
    // One partial expression index per supported size: text-embedding-3-large
    // at 1,024 (production, the openai default) and bge-small at 384 (local).
    // The vector leg casts to the configured size and filters on it, so it
    // uses the matching index.
    "CREATE INDEX IF NOT EXISTS idx_bookmark_chunks_hnsw_1024 ON bookmark_chunks
        USING hnsw ((embedding::halfvec(1024)) halfvec_cosine_ops)
        WHERE vector_dims(embedding) = 1024",
    "CREATE INDEX IF NOT EXISTS idx_bookmark_chunks_hnsw_384 ON bookmark_chunks
        USING hnsw ((embedding::halfvec(384)) halfvec_cosine_ops)
        WHERE vector_dims(embedding) = 384",
];

/// Create the `vector` extension (if it is missing and the role may) and the
/// chunks table. Never fails the boot: any error logs one warning and
/// returns `Unavailable`.
pub async fn ensure_vector_schema(pool: &PgPool) -> VectorStatus {
    let status = match try_ensure(pool).await {
        Ok(()) => VectorStatus::Ready,
        Err(e) => VectorStatus::Unavailable(unavailable_reason(
            e.as_database_error().and_then(|db| db.code()).as_deref(),
            &e.to_string(),
        )),
    };
    if let VectorStatus::Unavailable(reason) = &status {
        tracing::warn!("vector search unavailable: {reason}; lexical search only");
    }
    status
}

async fn try_ensure(pool: &PgPool) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    // Machines starting together would race on CREATE ... IF NOT EXISTS.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('boopmark.vector_schema', 0))")
        .execute(&mut *tx)
        .await?;
    let installed: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'vector')")
            .fetch_one(&mut *tx)
            .await?;
    if !installed {
        sqlx::query("CREATE EXTENSION IF NOT EXISTS vector")
            .execute(&mut *tx)
            .await?;
    }
    for ddl in VECTOR_DDL {
        sqlx::query(ddl).execute(&mut *tx).await?;
    }
    tx.commit().await
}

/// The reason shown to the owner, from the error's SQLSTATE and message.
fn unavailable_reason(code: Option<&str>, message: &str) -> String {
    match code {
        Some("42501") => "permission denied to create extension \"vector\"".to_string(),
        Some("0A000") => "the vector extension is not installed on the server".to_string(),
        _ => message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_mapping() {
        assert_eq!(
            unavailable_reason(Some("42501"), "error returned from database: ..."),
            "permission denied to create extension \"vector\""
        );
        assert_eq!(
            unavailable_reason(Some("0A000"), "extension \"vector\" is not available"),
            "the vector extension is not installed on the server"
        );
        assert_eq!(
            unavailable_reason(None, "pool timed out while waiting for an open connection"),
            "pool timed out while waiting for an open connection"
        );
    }
}

#[cfg(test)]
pub(crate) mod vector_schema_tests {
    use super::*;
    use crate::adapters::postgres::PostgresPool;
    use crate::adapters::postgres::search_repo::search_tests::{bookmark, pool, user};
    use crate::app::embedding::tests::DisabledStub;
    use crate::app::search::SearchService;
    use crate::domain::bookmark::BookmarkFilter;
    use std::sync::Arc;

    /// Run on a database WITHOUT pgvector available (stock postgres): startup
    /// succeeds, reports Unavailable, and lexical search and URL edits work.
    #[tokio::test]
    #[ignore = "requires DATABASE_URL on a Postgres without pgvector"]
    async fn without_vector_startup_succeeds_and_search_is_lexical() {
        let pool = pool().await;
        let status = ensure_vector_schema(&pool).await;
        assert!(matches!(status, VectorStatus::Unavailable(_)), "{status:?}");
        assert_eq!(ensure_vector_schema(&pool).await, status);

        let repo = Arc::new(PostgresPool::new(pool.clone()));
        let user_id = user(&pool).await;
        let saved = bookmark(
            &repo,
            user_id,
            "https://example.test/lexical",
            "Zebra husbandry handbook",
            None,
            &[],
        )
        .await;
        // The trigger skips bookmark_chunks while it does not exist.
        sqlx::query("UPDATE bookmarks SET url = $1 WHERE id = $2")
            .bind("https://example.test/lexical-moved")
            .bind(saved.id)
            .execute(&pool)
            .await
            .expect("a URL edit works without bookmark_chunks");

        let VectorStatus::Unavailable(reason) = status else {
            unreachable!()
        };
        let service = SearchService::new(repo.clone(), Arc::new(DisabledStub))
            .vector_unavailable(Some(reason.clone()));
        let hits = service
            .search(
                user_id,
                BookmarkFilter {
                    search: Some("zebra husbandry".into()),
                    ..Default::default()
                },
            )
            .await
            .expect("lexical search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].bookmark.id, saved.id);
        let line = service.deployment_line().await.unwrap();
        assert!(
            line.starts_with(&format!("vector search unavailable: {reason} · ")),
            "{line}"
        );
    }

    /// Run on a database WITH pgvector: Ready, and the tables are created
    /// idempotently.
    #[tokio::test]
    #[ignore = "requires DATABASE_URL on a Postgres with pgvector"]
    async fn with_vector_ready_is_idempotent() {
        let pool = pool().await;
        assert_eq!(ensure_vector_schema(&pool).await, VectorStatus::Ready);
        assert_eq!(ensure_vector_schema(&pool).await, VectorStatus::Ready);
        let indexes: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_indexes WHERE tablename = 'bookmark_chunks'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(indexes, 4, "primary key, user and two hnsw indexes");
    }
}
