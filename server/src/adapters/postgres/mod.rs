pub mod api_key_repo;
pub mod bookmark_repo;
pub mod ingest_listener;
pub mod invite_repo;
pub mod llm_settings_repo;
pub mod search_repo;
pub mod session_repo;
// Merge and undo are called by the /tags page and review screen (later steps).
#[allow(dead_code)]
pub mod tag_alias_repo;
pub mod user_repo;

use sqlx::PgPool;

#[derive(Clone)]
pub struct PostgresPool {
    pub pool: PgPool,
}

impl PostgresPool {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// Serialize every tag change of one user (write-path resolution, merge,
/// undo, the legacy consolidation) for the rest of the transaction.
pub(super) async fn lock_user_tags(
    conn: &mut sqlx::PgConnection,
    user_id: uuid::Uuid,
) -> Result<(), crate::domain::error::DomainError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('boopmark.tags:' || $1::text, 0))")
        .bind(user_id)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(|e| crate::domain::error::DomainError::Internal(e.to_string()))
}
