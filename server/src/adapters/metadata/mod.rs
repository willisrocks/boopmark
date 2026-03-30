pub mod fallback;
pub mod html;
pub mod iframely;
pub mod opengraph_io;

use crate::domain::error::DomainError;

/// Reject URLs that should not be forwarded to third-party metadata APIs.
/// Only allows public http/https URLs; strips credentials and fragments.
fn validate_public_url(url: &str) -> Result<String, DomainError> {
    let parsed = url::Url::parse(url)
        .map_err(|e| DomainError::InvalidInput(format!("invalid URL: {e}")))?;

    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(DomainError::InvalidInput(format!(
                "unsupported scheme for metadata fallback: {other}"
            )));
        }
    }

    if parsed.username() != "" || parsed.password().is_some() {
        return Err(DomainError::InvalidInput(
            "URL contains credentials; refusing to forward to third-party API".to_string(),
        ));
    }

    if let Some(host) = parsed.host_str()
        && (host == "localhost"
            || host == "127.0.0.1"
            || host == "::1"
            || host.ends_with(".local")
            || host.ends_with(".internal")
            || host.starts_with("10.")
            || host.starts_with("192.168.")
            || host.starts_with("172.16."))
        {
            return Err(DomainError::InvalidInput(
                "private/local URL; refusing to forward to third-party API".to_string(),
            ));
        }

    // Return URL without fragment
    let mut clean = parsed.clone();
    clean.set_fragment(None);
    Ok(clean.to_string())
}
