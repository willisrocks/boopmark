pub mod fallback;
pub mod html;
pub mod iframely;
pub mod opengraph_io;

use crate::domain::error::DomainError;

/// Reject URLs that should not be forwarded to third-party metadata APIs.
/// Only allows public http/https URLs; strips credentials and fragments.
pub(super) fn validate_public_url(url: &str) -> Result<String, DomainError> {
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

    if let Some(host) = parsed.host_str() {
        let normalized = host.trim_matches(['[', ']']).to_ascii_lowercase();
        let local_name = normalized == "localhost"
            || normalized.ends_with(".localhost")
            || normalized.ends_with(".local")
            || normalized.ends_with(".internal");
        let local_ip = normalized
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| match ip {
                std::net::IpAddr::V4(ip) => {
                    ip.is_private()
                        || ip.is_loopback()
                        || ip.is_link_local()
                        || ip.is_broadcast()
                        || ip.is_documentation()
                        || ip.is_unspecified()
                        || ip.is_multicast()
                }
                std::net::IpAddr::V6(ip) => {
                    let first = ip.segments()[0];
                    ip.is_loopback()
                        || ip.is_unspecified()
                        || ip.is_multicast()
                        || (first & 0xfe00) == 0xfc00 // unique-local fc00::/7
                        || (first & 0xffc0) == 0xfe80 // link-local fe80::/10
                }
            });
        if local_name || local_ip {
            return Err(DomainError::InvalidInput(
                "private/local URL; refusing to fetch or forward it".to_string(),
            ));
        }
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
    fn rejects_private_ip_across_the_full_172_16_range() {
        assert!(validate_public_url("http://172.31.255.254/internal").is_err());
    }

    #[test]
    fn rejects_ipv6_unique_local_and_link_local_addresses() {
        assert!(validate_public_url("http://[fc00::1]/internal").is_err());
        assert!(validate_public_url("http://[fe80::1]/internal").is_err());
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
