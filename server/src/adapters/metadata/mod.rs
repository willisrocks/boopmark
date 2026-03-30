pub mod fallback;
pub mod html;
pub mod iframely;
pub mod opengraph_io;

use crate::domain::error::DomainError;

/// Reject URLs that should not be forwarded to third-party metadata APIs.
/// Only allows public http/https URLs; strips credentials and fragments.
fn validate_public_url(url: &str) -> Result<String, DomainError> {
    let parsed =
        url::Url::parse(url).map_err(|e| DomainError::InvalidInput(format!("invalid URL: {e}")))?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_public_https_url() {
        let result = validate_public_url("https://medium.com/article");
        assert!(result.is_ok());
    }

    #[test]
    fn accepts_public_http_url() {
        let result = validate_public_url("http://example.com/page");
        assert!(result.is_ok());
    }

    #[test]
    fn rejects_ftp_scheme() {
        let result = validate_public_url("ftp://files.example.com/doc");
        assert!(result.is_err());
    }

    #[test]
    fn rejects_url_with_credentials() {
        let result = validate_public_url("https://user:pass@example.com/page");
        assert!(result.is_err());
    }

    #[test]
    fn rejects_localhost() {
        let result = validate_public_url("http://localhost:3000/api");
        assert!(result.is_err());
    }

    #[test]
    fn rejects_loopback_ip() {
        let result = validate_public_url("http://127.0.0.1:8080/path");
        assert!(result.is_err());
    }

    #[test]
    fn rejects_private_ip_10() {
        let result = validate_public_url("http://10.0.0.1/internal");
        assert!(result.is_err());
    }

    #[test]
    fn rejects_private_ip_192_168() {
        let result = validate_public_url("http://192.168.1.1/admin");
        assert!(result.is_err());
    }

    #[test]
    fn strips_fragment_from_url() {
        let result = validate_public_url("https://example.com/page#section").unwrap();
        assert_eq!(result, "https://example.com/page");
    }

    #[test]
    fn rejects_invalid_url() {
        let result = validate_public_url("not a url");
        assert!(result.is_err());
    }
}
