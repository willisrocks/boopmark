pub mod auth;
pub mod bookmarks;
pub mod image_fix;
pub mod page_capture;
pub mod transfer;

use crate::web::state::AppState;
use axum::Router;

pub fn routes() -> Router<AppState> {
    Router::new()
        .nest(
            "/bookmarks",
            bookmarks::routes()
                .merge(transfer::routes())
                .merge(image_fix::routes())
                .merge(page_capture::routes()),
        )
        .nest("/auth", auth::routes())
}
