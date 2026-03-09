mod auth;
pub mod bookmarks;

use axum::Router;
use axum::routing::{delete, get, post};

use crate::web::extractors::MaybeUser;
use crate::web::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(home))
        .route("/bookmarks", get(bookmarks::list).post(bookmarks::create))
        .route("/bookmarks/suggest", post(bookmarks::suggest))
        .route("/bookmarks/{id}", delete(bookmarks::delete))
        .merge(auth::routes())
}

async fn home(MaybeUser(user): MaybeUser) -> axum::response::Redirect {
    if user.is_some() {
        axum::response::Redirect::to("/bookmarks")
    } else {
        axum::response::Redirect::to("/auth/login")
    }
}
