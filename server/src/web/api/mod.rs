pub mod auth;
pub mod bookmarks;

use crate::web::state::AppState;
use axum::Router;

pub fn routes() -> Router<AppState> {
    Router::new()
        .nest("/bookmarks", bookmarks::routes())
        .nest("/auth", auth::routes())
}
