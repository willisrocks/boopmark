pub mod api_key_repo;
pub mod bookmark_repo;
pub mod invite_repo;
pub mod llm_settings_repo;
pub mod session_repo;
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
