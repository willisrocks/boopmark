use askama::Template;
use axum::extract::{Path, State};
use axum::response::{Html, IntoResponse};
use axum_extra::extract::cookie::{Cookie, CookieJar};

use crate::web::state::AppState;

#[derive(Template)]
#[template(path = "auth/invite.html")]
struct InvitePage {
    provider_name: String,
}

#[derive(Template)]
#[template(path = "auth/invite_invalid.html")]
struct InviteInvalidPage;

pub async fn invite_landing(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(token): Path<String>,
) -> impl IntoResponse {
    match state.invites.validate_token(&token).await {
        Ok(Some(_invite)) => {
            let cookie = Cookie::build(("invite_token", token))
                .path("/")
                .http_only(true)
                .secure(state.config.app_url.starts_with("https://"))
                .same_site(axum_extra::extract::cookie::SameSite::Lax)
                .build();
            let provider_name = state.login_provider.login_page_context().provider_name;
            let page = InvitePage { provider_name };
            match page.render() {
                Ok(body) => (jar.add(cookie), Html(body)).into_response(),
                Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
        _ => {
            let page = InviteInvalidPage;
            match page.render() {
                Ok(body) => Html(body).into_response(),
                Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
    }
}
