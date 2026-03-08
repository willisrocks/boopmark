use askama::Template;
use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse, Redirect};
use axum::Router;
use axum_extra::extract::cookie::{Cookie, CookieJar};
use serde::Deserialize;

use crate::web::state::AppState;

#[derive(Template)]
#[template(path = "auth/login.html")]
struct LoginPage;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login", axum::routing::get(login_page))
        .route("/auth/google", axum::routing::get(google_redirect))
        .route("/auth/google/callback", axum::routing::get(google_callback))
        .route("/auth/logout", axum::routing::post(logout))
}

async fn login_page() -> impl IntoResponse {
    match LoginPage.render() {
        Ok(body) => Html(body).into_response(),
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Redirect the user to Google's OAuth consent screen.
async fn google_redirect(State(state): State<AppState>) -> Redirect {
    let config = &state.config;
    let redirect_uri = format!("{}/auth/google/callback", config.app_url);
    let url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope=openid%20email%20profile",
        urlencoding(&config.google_client_id),
        urlencoding(&redirect_uri),
    );
    Redirect::temporary(&url)
}

/// Exchange the authorization code for tokens, fetch user info, upsert user, create session.
async fn google_callback(
    State(state): State<AppState>,
    Query(params): Query<CallbackParams>,
    jar: CookieJar,
) -> Result<(CookieJar, Redirect), (axum::http::StatusCode, String)> {
    let config = &state.config;
    let redirect_uri = format!("{}/auth/google/callback", config.app_url);

    // Exchange code for tokens
    let client = reqwest::Client::new();
    let token_res = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("code", params.code.as_str()),
            ("client_id", config.google_client_id.as_str()),
            ("client_secret", config.google_client_secret.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e.to_string()))?;

    if !token_res.status().is_success() {
        let body = token_res.text().await.unwrap_or_default();
        return Err((
            axum::http::StatusCode::BAD_GATEWAY,
            format!("Google token exchange failed: {body}"),
        ));
    }

    let tokens: TokenResponse = token_res
        .json()
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e.to_string()))?;

    // Fetch user info
    let userinfo: GoogleUserInfo = client
        .get("https://www.googleapis.com/oauth2/v3/userinfo")
        .bearer_auth(&tokens.access_token)
        .send()
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e.to_string()))?
        .json()
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_GATEWAY, e.to_string()))?;

    // Upsert user and create session
    let user = state
        .auth
        .upsert_user(userinfo.email, userinfo.name, userinfo.picture)
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let session_token = state
        .auth
        .create_session(user.id)
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let cookie = Cookie::build(("session", session_token))
        .path("/")
        .http_only(true)
        .secure(true)
        .same_site(axum_extra::extract::cookie::SameSite::Lax)
        .build();

    Ok((jar.add(cookie), Redirect::to("/")))
}

/// Delete the session and clear the cookie.
async fn logout(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<(CookieJar, Redirect), (axum::http::StatusCode, String)> {
    if let Some(cookie) = jar.get("session") {
        let _ = state.auth.delete_session(cookie.value()).await;
    }

    let removal = Cookie::build(("session", ""))
        .path("/")
        .build();

    Ok((jar.remove(removal), Redirect::to("/")))
}

#[derive(Deserialize)]
struct CallbackParams {
    code: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct GoogleUserInfo {
    email: String,
    name: Option<String>,
    picture: Option<String>,
}

fn urlencoding(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
