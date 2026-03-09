use axum::Router;
use tower_http::services::ServeDir;

use crate::web::state::AppState;

pub fn create_router(state: AppState) -> Router {
    Router::new()
        // API routes
        .nest("/api/v1", super::api::routes())
        // Page routes
        .merge(super::pages::routes())
        // Static files (checked-in assets: CSS, JS, etc.)
        .nest_service("/static", ServeDir::new("static"))
        // User-generated uploads (images, etc.)
        .nest_service("/uploads", ServeDir::new("uploads"))
        // Health check
        .route("/health", axum::routing::get(|| async { "ok" }))
        .with_state(state)
}
