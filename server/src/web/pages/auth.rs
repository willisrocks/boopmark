use askama::Template;
use axum::Router;
use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse, Redirect};
use axum_extra::extract::cookie::{Cookie, CookieJar};
use serde::Deserialize;

use crate::web::pages::auth_shared::build_session_cookie;
use crate::web::state::AppState;

#[derive(Template)]
#[template(path = "auth/login.html")]
struct LoginPage {
    enable_e2e_auth: bool,
    provider_name: String,
    login_error: Option<String>,
}

#[derive(Deserialize)]
struct LoginQueryParams {
    #[serde(default)]
    error: Option<String>,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login", axum::routing::get(login_page))
        .route("/auth/test-login", axum::routing::post(test_login))
        .route("/auth/logout", axum::routing::post(logout))
}

async fn login_page(
    State(state): State<AppState>,
    Query(params): Query<LoginQueryParams>,
) -> impl IntoResponse {
    let provider_name = state.login_provider.login_page_context().provider_name;
    let page = LoginPage {
        enable_e2e_auth: state.config.enable_e2e_auth,
        provider_name,
        login_error: params.error,
    };

    match page.render() {
        Ok(body) => Html(body).into_response(),
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn test_login(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<(CookieJar, Redirect), axum::http::StatusCode> {
    if !state.config.enable_e2e_auth {
        return Err(axum::http::StatusCode::NOT_FOUND);
    }

    let user = state
        .auth
        .upsert_user(
            "e2e@boopmark.local".to_string(),
            Some("Boopmark E2E".to_string()),
            None,
        )
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    let token = state
        .auth
        .create_session(user.id)
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok((
        jar.add(build_session_cookie(&state.config.app_url, token)),
        Redirect::to("/"),
    ))
}

/// Delete the session and clear the cookie.
async fn logout(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<(CookieJar, Redirect), (axum::http::StatusCode, String)> {
    if let Some(cookie) = jar.get("session") {
        let _ = state.auth.delete_session(cookie.value()).await;
    }

    let removal = Cookie::build(("session", "")).path("/").build();

    Ok((jar.remove(removal), Redirect::to("/")))
}
