pub mod auth;
pub mod bookmarks;

use crate::web::middleware::api_error_json::json_error_layer;
use crate::web::state::AppState;
use axum::middleware;
use axum::Router;

pub fn routes() -> Router<AppState> {
    Router::new()
        .nest("/bookmarks", bookmarks::routes())
        .nest("/auth", auth::routes())
        .layer(middleware::from_fn(json_error_layer))
}
