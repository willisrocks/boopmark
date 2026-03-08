use axum::Router;
use tower_http::services::ServeDir;

use crate::web::state::AppState;

pub fn create_router(state: AppState) -> Router {
    Router::new()
        // API routes
        .nest("/api/v1", super::api::routes())
        // Page routes
        .merge(super::pages::routes())
        // Static files
        .nest_service("/static", ServeDir::new("static"))
        // Health check
        .route("/health", axum::routing::get(|| async { "ok" }))
        .with_state(state)
}
