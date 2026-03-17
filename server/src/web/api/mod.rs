pub mod auth;
pub mod bookmarks;
pub mod transfer;

use crate::web::state::AppState;
use axum::Router;

pub fn routes() -> Router<AppState> {
    Router::new()
        .nest("/bookmarks", bookmarks::routes().merge(transfer::routes()))
        .nest("/auth", auth::routes())
}
