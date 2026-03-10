use askama::Template;
use axum::Router;
use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse, Redirect};
use axum_extra::extract::cookie::{Cookie, CookieJar};
use serde::Deserialize;

use crate::domain::ports::storage::ObjectStorage;
use crate::web::state::AppState;

#[derive(Template)]
#[template(path = "auth/login.html")]
struct LoginPage {
    enable_e2e_auth: bool,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login", axum::routing::get(login_page))
        .route("/auth/test-login", axum::routing::post(test_login))
        .route("/auth/google", axum::routing::get(google_redirect))
        .route("/auth/google/callback", axum::routing::get(google_callback))
        .route("/auth/logout", axum::routing::post(logout))
}

async fn login_page(State(state): State<AppState>) -> impl IntoResponse {
    let page = LoginPage {
        enable_e2e_auth: state.config.enable_e2e_auth,
    };

    match page.render() {
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

    // Download and cache avatar image
    let stored_image = if let Some(ref picture_url) = userinfo.picture {
        download_and_store_avatar(picture_url, &state).await
    } else {
        None
    };

    // Look up the user's current avatar so we can clean up the old one after upsert
    let old_avatar_url = state
        .auth
        .find_user_by_email(&userinfo.email)
        .await
        .ok()
        .flatten()
        .and_then(|u| u.image);

    // Upsert user and create session
    let user = state
        .auth
        .upsert_user(userinfo.email, userinfo.name, stored_image.clone())
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Clean up old avatar from storage if it was replaced
    if let Some(ref old_url) = old_avatar_url {
        if stored_image.as_ref() != Some(old_url) {
            if let Some(old_key) = state.images_storage.key_from_url(old_url) {
                if let Err(e) = state.images_storage.delete(&old_key).await {
                    tracing::warn!("Failed to delete old avatar {old_key}: {e}");
                }
            }
        }
    }

    let session_token = state
        .auth
        .create_session(user.id)
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let cookie = build_session_cookie(&state.config, session_token);

    Ok((jar.add(cookie), Redirect::to("/")))
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
        jar.add(build_session_cookie(&state.config, token)),
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

/// Maximum avatar image size: 5 MB.
const MAX_AVATAR_BYTES: u64 = 5 * 1024 * 1024;

/// Download timeout for avatar fetch.
const AVATAR_DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// JPEG, PNG, GIF, and WebP magic bytes for content validation.
fn looks_like_image(bytes: &[u8]) -> bool {
    if bytes.len() < 4 {
        return false;
    }
    // JPEG: FF D8 FF
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return true;
    }
    // PNG: 89 50 4E 47
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        return true;
    }
    // GIF: GIF87a or GIF89a
    if bytes.starts_with(b"GIF8") {
        return true;
    }
    // WebP: RIFF....WEBP
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return true;
    }
    false
}

/// Download a Google avatar image, validate it, and store it in the images bucket.
/// Returns `None` on any failure (graceful degradation — the Google URL is not stored
/// as a fallback because we want to avoid hotlinking).
async fn download_and_store_avatar(picture_url: &str, state: &AppState) -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(AVATAR_DOWNLOAD_TIMEOUT)
        .build()
        .ok()?;

    let resp = match client.get(picture_url).send().await {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            tracing::warn!("Avatar download returned status {}", r.status());
            return None;
        }
        Err(e) => {
            tracing::warn!("Avatar download failed: {e}");
            return None;
        }
    };

    // Check Content-Length before reading the body
    if let Some(len) = resp.content_length() {
        if len > MAX_AVATAR_BYTES {
            tracing::warn!("Avatar too large ({len} bytes), skipping");
            return None;
        }
    }

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/jpeg")
        .to_string();

    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("Failed to read avatar bytes: {e}");
            return None;
        }
    };

    // Enforce size limit even if Content-Length was absent or wrong
    if bytes.len() as u64 > MAX_AVATAR_BYTES {
        tracing::warn!("Avatar too large ({} bytes), skipping", bytes.len());
        return None;
    }

    // Validate magic bytes to confirm the response is actually an image
    if !looks_like_image(&bytes) {
        tracing::warn!("Avatar response does not look like a valid image, skipping");
        return None;
    }

    let ext = match content_type.split(';').next().unwrap_or("").trim() {
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "jpg",
    };
    let key = format!("avatars/{}.{}", uuid::Uuid::new_v4(), ext);

    match state
        .images_storage
        .put(&key, bytes.to_vec(), &content_type)
        .await
    {
        Ok(url) => Some(url),
        Err(e) => {
            tracing::warn!("Failed to store avatar: {e}");
            None
        }
    }
}

fn build_session_cookie(config: &crate::config::Config, token: String) -> Cookie<'static> {
    Cookie::build(("session", token))
        .path("/")
        .http_only(true)
        .secure(config.app_url.starts_with("https://"))
        .same_site(axum_extra::extract::cookie::SameSite::Lax)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_like_image_detects_jpeg() {
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        assert!(looks_like_image(&jpeg));
    }

    #[test]
    fn looks_like_image_detects_png() {
        let png = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        assert!(looks_like_image(&png));
    }

    #[test]
    fn looks_like_image_detects_gif() {
        assert!(looks_like_image(b"GIF89a..."));
        assert!(looks_like_image(b"GIF87a..."));
    }

    #[test]
    fn looks_like_image_detects_webp() {
        let mut webp = vec![0u8; 12];
        webp[..4].copy_from_slice(b"RIFF");
        webp[8..12].copy_from_slice(b"WEBP");
        assert!(looks_like_image(&webp));
    }

    #[test]
    fn looks_like_image_rejects_html() {
        assert!(!looks_like_image(b"<html>"));
    }

    #[test]
    fn looks_like_image_rejects_short_input() {
        assert!(!looks_like_image(&[0xFF, 0xD8]));
        assert!(!looks_like_image(&[]));
    }
}
