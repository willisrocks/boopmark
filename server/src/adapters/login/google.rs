use axum::Router;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::Redirect;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;

use crate::domain::ports::login_provider::{
    AuthenticatedIdentity, LoginPageContext, LoginProvider,
};
use crate::domain::ports::storage::ObjectStorage;
use crate::web::pages::auth_shared::{handle_authenticated_identity, origin_from_headers};
use crate::web::state::AppState;

#[allow(dead_code)]
pub struct GoogleLoginProvider {
    pub client_id: String,
    pub client_secret: String,
}

impl LoginProvider for GoogleLoginProvider {
    fn routes(&self) -> Router<AppState> {
        Router::new()
            .route("/auth/google", axum::routing::get(google_redirect))
            .route("/auth/google/callback", axum::routing::get(google_callback))
    }

    fn login_page_context(&self) -> LoginPageContext {
        LoginPageContext {
            provider_name: "google".to_string(),
        }
    }
}

/// Redirect the user to Google's OAuth consent screen.
async fn google_redirect(State(state): State<AppState>, headers: HeaderMap) -> Redirect {
    let config = &state.config;
    let origin = origin_from_headers(&headers, config);
    let redirect_uri = format!("{origin}/auth/google/callback");

    let client_id = config.google_client_id.as_deref().unwrap_or_default();

    let url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope=openid%20email%20profile",
        urlencoding(client_id),
        urlencoding(&redirect_uri),
    );
    Redirect::temporary(&url)
}

/// Exchange the authorization code for tokens, fetch user info, then delegate
/// to the shared post-auth logic which handles invite checks and session creation.
async fn google_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<CallbackParams>,
    jar: CookieJar,
) -> Result<impl axum::response::IntoResponse, (axum::http::StatusCode, String)> {
    let config = &state.config;
    let origin = origin_from_headers(&headers, config);
    let redirect_uri = format!("{origin}/auth/google/callback");

    let client_id = config.google_client_id.as_deref().unwrap_or_default();
    let client_secret = config.google_client_secret.as_deref().unwrap_or_default();

    // Exchange code for tokens
    let client = reqwest::Client::new();
    let token_res = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("code", params.code.as_str()),
            ("client_id", client_id),
            ("client_secret", client_secret),
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

    let identity = AuthenticatedIdentity {
        email: userinfo.email,
        name: userinfo.name,
        image: stored_image,
    };

    Ok(handle_authenticated_identity(&state, &origin, identity, jar).await)
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
/// Returns `(extension, mime_type)` if bytes match a known image format.
fn detect_image_format(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    if bytes.len() < 3 {
        return None;
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(("jpg", "image/jpeg"));
    }
    if bytes.len() >= 4 && bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        return Some(("png", "image/png"));
    }
    if bytes.len() >= 6 && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        return Some(("gif", "image/gif"));
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some(("webp", "image/webp"));
    }
    None
}

/// Allowed Google domains for avatar image URLs.
const ALLOWED_AVATAR_HOSTS: &[&str] = &[".googleusercontent.com", ".google.com"];

/// Check that a URL's host belongs to a known Google domain.
fn is_allowed_avatar_host(url_str: &str) -> bool {
    url::Url::parse(url_str)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_lowercase()))
        .is_some_and(|host| {
            ALLOWED_AVATAR_HOSTS
                .iter()
                .any(|suffix| host.ends_with(suffix) || host == suffix.trim_start_matches('.'))
        })
}

/// Maximum number of HTTP redirects to follow when downloading an avatar.
const MAX_AVATAR_REDIRECTS: usize = 5;

/// Download a Google avatar image, validate it, and store it in the images bucket.
/// Returns `None` on any failure (graceful degradation).
async fn download_and_store_avatar(picture_url: &str, state: &AppState) -> Option<String> {
    if !picture_url.starts_with("https://") {
        tracing::warn!("Rejecting non-HTTPS avatar URL: {picture_url}");
        return None;
    }
    if !is_allowed_avatar_host(picture_url) {
        tracing::warn!("Rejecting avatar URL from non-Google domain: {picture_url}");
        return None;
    }

    let client = reqwest::Client::builder()
        .timeout(AVATAR_DOWNLOAD_TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(MAX_AVATAR_REDIRECTS))
        .build()
        .ok()?;

    let mut resp = match client.get(picture_url).send().await {
        Ok(r) if r.status().is_success() => {
            let final_url = r.url().as_str();
            if !final_url.starts_with("https://") {
                tracing::warn!("Avatar redirect landed on non-HTTPS URL: {final_url}");
                return None;
            }
            if !is_allowed_avatar_host(final_url) {
                tracing::warn!("Avatar redirect landed on non-Google domain: {final_url}");
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

    if let Some(len) = resp.content_length()
        && len > MAX_AVATAR_BYTES
    {
        tracing::warn!("Avatar too large ({len} bytes), skipping");
        return None;
    }

    let mut body = Vec::new();
    while let Some(chunk) = match resp.chunk().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Failed to read avatar chunk: {e}");
            return None;
        }
    } {
        if body.len() + chunk.len() > MAX_AVATAR_BYTES as usize {
            tracing::warn!("Avatar exceeded {MAX_AVATAR_BYTES} bytes during streaming, aborting");
            return None;
        }
        body.extend_from_slice(&chunk);
    }

    let (ext, content_type) = match detect_image_format(&body) {
        Some(fmt) => fmt,
        None => {
            tracing::warn!("Avatar response does not look like a valid image, skipping");
            return None;
        }
    };

    let key = format!("avatars/{}.{}", uuid::Uuid::new_v4(), ext);

    match state.images_storage.put(&key, body, content_type).await {
        Ok(url) => Some(url),
        Err(e) => {
            tracing::warn!("Failed to store avatar: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_image_format_identifies_jpeg() {
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        assert_eq!(detect_image_format(&jpeg), Some(("jpg", "image/jpeg")));
    }

    #[test]
    fn detect_image_format_identifies_jpeg_minimal_3_bytes() {
        assert_eq!(
            detect_image_format(&[0xFF, 0xD8, 0xFF]),
            Some(("jpg", "image/jpeg"))
        );
    }

    #[test]
    fn detect_image_format_identifies_png() {
        let png = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        assert_eq!(detect_image_format(&png), Some(("png", "image/png")));
    }

    #[test]
    fn detect_image_format_identifies_gif() {
        assert_eq!(
            detect_image_format(b"GIF89a..."),
            Some(("gif", "image/gif"))
        );
        assert_eq!(
            detect_image_format(b"GIF87a..."),
            Some(("gif", "image/gif"))
        );
    }

    #[test]
    fn detect_image_format_rejects_invalid_gif_version() {
        assert_eq!(detect_image_format(b"GIF80a..."), None);
    }

    #[test]
    fn detect_image_format_identifies_webp() {
        let mut webp = vec![0u8; 12];
        webp[..4].copy_from_slice(b"RIFF");
        webp[8..12].copy_from_slice(b"WEBP");
        assert_eq!(detect_image_format(&webp), Some(("webp", "image/webp")));
    }

    #[test]
    fn detect_image_format_rejects_html() {
        assert_eq!(detect_image_format(b"<html>.."), None);
    }

    #[test]
    fn detect_image_format_rejects_too_short() {
        assert_eq!(detect_image_format(&[0xFF, 0xD8]), None);
        assert_eq!(detect_image_format(&[]), None);
    }

    #[test]
    fn is_allowed_avatar_host_accepts_google_domains() {
        assert!(is_allowed_avatar_host(
            "https://lh3.googleusercontent.com/a/photo.jpg"
        ));
        assert!(is_allowed_avatar_host(
            "https://www.google.com/images/photo.jpg"
        ));
        assert!(is_allowed_avatar_host(
            "https://googleusercontent.com/photo.jpg"
        ));
    }

    #[test]
    fn is_allowed_avatar_host_rejects_non_google_domains() {
        assert!(!is_allowed_avatar_host("https://evil.com/photo.jpg"));
        assert!(!is_allowed_avatar_host(
            "https://fakegoogleusercontent.com/photo.jpg"
        ));
        assert!(!is_allowed_avatar_host(
            "https://evil.com/googleusercontent.com"
        ));
    }
}
