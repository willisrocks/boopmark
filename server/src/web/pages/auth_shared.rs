use axum::http::HeaderMap;
use axum_extra::extract::cookie::Cookie;

/// Derive the origin (scheme + host) from request headers, falling back to config.app_url.
pub fn origin_from_headers(headers: &HeaderMap, config: &crate::config::Config) -> String {
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get("host"))
        .and_then(|v| v.to_str().ok());

    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");

    match host {
        Some(h) => format!("{proto}://{h}"),
        None => config.app_url.clone(),
    }
}

pub fn build_session_cookie(origin: &str, token: String) -> Cookie<'static> {
    Cookie::build(("session", token))
        .path("/")
        .http_only(true)
        .secure(origin.starts_with("https://"))
        .same_site(axum_extra::extract::cookie::SameSite::Lax)
        .build()
}
