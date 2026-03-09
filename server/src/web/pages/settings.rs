use askama::Template;
use axum::Router;
use axum::response::{Html, IntoResponse};

use crate::web::extractors::AuthUser;
use crate::web::state::AppState;

#[derive(Template)]
#[template(path = "settings/api_keys.html")]
struct ApiKeysPage {
    email: String,
}

fn render(t: &impl Template) -> axum::response::Response {
    match t.render() {
        Ok(body) => Html(body).into_response(),
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn api_keys_page(AuthUser(user): AuthUser) -> axum::response::Response {
    render(&ApiKeysPage { email: user.email })
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/settings/api-keys", axum::routing::get(api_keys_page))
}
