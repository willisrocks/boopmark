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
        download_and_store_avatar(picture_url, &client, &state).await
    } else {
        None
    };

    // Look up the user's current avatar so we can clean up the old one after upsert.
    //
    // NOTE: There is a known race condition here. If two concurrent Google logins
    // for the same email happen simultaneously, both will see the same old avatar URL,
    // both will store a new avatar, and one upsert will overwrite the other. The
    // "losing" avatar file becomes an orphan in storage. This is accepted as the
    // probability is extremely low (same user logging in concurrently) and the cost
    // is a small orphaned image file.
    let old_avatar_url = state
        .auth
        .find_user_by_email(&userinfo.email)
        .await
        .ok()
        .flatten()
        .and_then(|u| u.image);

    // Upsert user and create session.
    //
    // NOTE: The upsert SQL uses `COALESCE($3, users.image)`, meaning if `stored_image`
    // is `None` (avatar download failed), the existing `users.image` value is preserved.
    // If a user's first login occurred before this feature was deployed, their `image`
    // column may contain a raw Google URL (lh3.googleusercontent.com). A failed avatar
    // download on subsequent logins will preserve that stale Google URL rather than
    // clearing it. This is acceptable as a transitional state — the next successful
    // login will replace it with a locally-stored avatar.
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

/// Recognized image formats with their magic bytes.
/// Returns the format name (used as file extension) if bytes match a known image format.
fn detect_image_format(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() < 6 {
        return None;
    }
    // JPEG: FF D8 FF
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("jpg");
    }
    // PNG: 89 50 4E 47
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        return Some("png");
    }
    // GIF: GIF87a or GIF89a (must check full 6-byte signature)
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("gif");
    }
    // WebP: RIFF....WEBP
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("webp");
    }
    None
}

/// Download a Google avatar image, validate it, and store it in the images bucket.
/// Returns `None` on any failure (graceful degradation — the Google URL is not stored
/// as a fallback because we want to avoid hotlinking).
///
/// Reuses the caller's `reqwest::Client` (which handles Google API calls) but applies
/// a per-request timeout and HTTPS-only restriction to mitigate SSRF risks.
async fn download_and_store_avatar(
    picture_url: &str,
    client: &reqwest::Client,
    state: &AppState,
) -> Option<String> {
    // Only allow HTTPS URLs to prevent SSRF via redirects to internal HTTP endpoints
    if !picture_url.starts_with("https://") {
        tracing::warn!("Rejecting non-HTTPS avatar URL: {picture_url}");
        return None;
    }

    let resp = match client
        .get(picture_url)
        .timeout(AVATAR_DOWNLOAD_TIMEOUT)
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => {
            // After following redirects, verify we're still on HTTPS
            let final_url = r.url().as_str();
            if !final_url.starts_with("https://") {
                tracing::warn!("Avatar redirect landed on non-HTTPS URL: {final_url}");
                return None;
            }
            r
        }
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

    // Validate magic bytes and derive the file extension from actual content,
    // not the Content-Type header (which could be wrong or misleading)
    let ext = match detect_image_format(&bytes) {
        Some(fmt) => fmt,
        None => {
            tracing::warn!("Avatar response does not look like a valid image, skipping");
            return None;
        }
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
    fn detect_image_format_identifies_jpeg() {
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        assert_eq!(detect_image_format(&jpeg), Some("jpg"));
    }

    #[test]
    fn detect_image_format_identifies_png() {
        let png = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        assert_eq!(detect_image_format(&png), Some("png"));
    }

    #[test]
    fn detect_image_format_identifies_gif() {
        assert_eq!(detect_image_format(b"GIF89a..."), Some("gif"));
        assert_eq!(detect_image_format(b"GIF87a..."), Some("gif"));
    }

    #[test]
    fn detect_image_format_rejects_invalid_gif_version() {
        // GIF80a is not a valid GIF version
        assert_eq!(detect_image_format(b"GIF80a..."), None);
    }

    #[test]
    fn detect_image_format_identifies_webp() {
        let mut webp = vec![0u8; 12];
        webp[..4].copy_from_slice(b"RIFF");
        webp[8..12].copy_from_slice(b"WEBP");
        assert_eq!(detect_image_format(&webp), Some("webp"));
    }

    #[test]
    fn detect_image_format_rejects_html() {
        assert_eq!(detect_image_format(b"<html>.."), None);
    }

    #[test]
    fn detect_image_format_rejects_short_input() {
        assert_eq!(detect_image_format(&[0xFF, 0xD8, 0xFF]), None);
        assert_eq!(detect_image_format(&[]), None);
    }
}
