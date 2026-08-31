mod admin;
mod auth;
pub mod auth_shared;
pub mod bookmarks;
mod invite;
mod legal;
mod settings;
pub(crate) mod shared;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post};

use crate::web::extractors::MaybeUser;
use crate::web::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(home))
        .route("/privacy", get(legal::privacy))
        .route("/support", get(legal::support))
        .route("/bookmarks", get(bookmarks::list).post(bookmarks::create))
        .route("/bookmarks/suggest", post(bookmarks::suggest))
        .route(
            "/bookmarks/{id}",
            delete(bookmarks::delete).put(bookmarks::update),
        )
        .route("/bookmarks/{id}/edit", get(bookmarks::edit))
        .route(
            "/bookmarks/{id}/image",
            post(bookmarks::upload_image)
                .delete(bookmarks::remove_image)
                .layer(DefaultBodyLimit::max(
                    bookmarks::MAX_IMAGE_UPLOAD_BYTES + 1024 * 1024,
                )),
        )
        .route(
            "/bookmarks/{id}/image/generate",
            post(bookmarks::generate_image),
        )
        .route("/bookmarks/{id}/suggest", post(bookmarks::edit_suggest))
        .route("/invite/{token}", get(invite::invite_landing))
        .route("/admin", get(admin::admin_page))
        .route("/admin/invites", post(admin::create_invite))
        .route("/admin/invites/{id}/revoke", post(admin::revoke_invite))
        .route("/admin/users/{id}/role", post(admin::update_user_role))
        .route("/admin/users/{id}/deactivate", post(admin::deactivate_user))
        .merge(auth::routes())
        .merge(settings::routes())
}

async fn home(MaybeUser(user): MaybeUser) -> axum::response::Redirect {
    if user.is_some() {
        axum::response::Redirect::to("/bookmarks")
    } else {
        axum::response::Redirect::to("/auth/login")
    }
}
