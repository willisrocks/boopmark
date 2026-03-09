pub mod auth;
pub mod bookmarks;

use axum::Router;
use crate::web::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .nest("/bookmarks", bookmarks::routes())
        .nest("/auth", auth::routes())
}
