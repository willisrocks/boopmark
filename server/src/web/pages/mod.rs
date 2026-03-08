mod auth;

use axum::Router;
use crate::web::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(auth::routes())
}
