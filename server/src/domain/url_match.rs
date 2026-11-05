//! Whether two URLs name the same page.
//!
//! Used by the page-capture PUT (the `409` check) and by the `?url=` lookup,
//! so both agree on what "the same page" means. A capture never rewrites the
//! stored URL: only the comparison is normalized.

use url::Url;

/// Query parameters that never change what a page shows: they are dropped
/// from both sides before the remaining pairs are compared.
///
/// `ref` and `source` are deliberately absent: Medium's `?source=` changes
/// what the page shows.
const TRACKING_PARAMS: &[&str] = &[
    "fbclid", "gclid", "gbraid", "wbraid", "msclkid", "yclid", "dclid", "mc_cid", "mc_eid",
    "igshid", "_hsenc", "_hsmi", "ref_src",
];

/// A page URL as the capture endpoint accepts it: absolute, `http` or
/// `https`. `None` for anything else, which the route reports as
/// `400 invalid_page_url`.
pub fn parse_web_url(raw: &str) -> Option<Url> {
    let url = Url::parse(raw.trim()).ok()?;
    matches!(url.scheme(), "http" | "https").then_some(url)
}

/// Whether `page_url` and `bookmark_url` are the same page:
///
/// - scheme: case-insensitive, and `http` equals `https` (saves are often
///   stored pre-redirect);
/// - host: as the parser normalizes it (lowercase, IDNA), default port
///   removed, so a non-default port must match;
/// - userinfo: must be absent in both;
/// - path: one trailing `/` ignored, otherwise byte for byte (and
///   case-sensitive);
/// - query: tracking parameters dropped, the rest compared as a sorted set
///   of `(name, value)` pairs, so order does not matter;
/// - fragment: ignored, it is never part of a page's identity.
pub fn same_page(bookmark_url: &str, page_url: &str) -> bool {
    let (Some(bookmark), Some(page)) = (parse_web_url(bookmark_url), parse_web_url(page_url))
    else {
        return false;
    };
    if has_userinfo(&bookmark) || has_userinfo(&page) {
        return false;
    }
    bookmark.host_str() == page.host_str()
        && bookmark.port() == page.port()
        && page_path(&bookmark) == page_path(&page)
        && query_pairs(&bookmark) == query_pairs(&page)
}

fn has_userinfo(url: &Url) -> bool {
    !url.username().is_empty() || url.password().is_some()
}

fn page_path(url: &Url) -> &str {
    let path = url.path();
    path.strip_suffix('/').unwrap_or(path)
}

fn query_pairs(url: &Url) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(name, _)| !is_tracking(name))
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    pairs.sort();
    pairs
}

/// Tracking parameters by exact name, plus every `utm_*`.
fn is_tracking(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with("utm_") || TRACKING_PARAMS.contains(&name.as_str())
}

/// Whether `raw` is hosted on Medium: `medium.com` or any subdomain of it.
// No caller yet; the helper lands ahead of its use (issue #51).
#[allow(dead_code)]
pub fn is_medium_url(raw: &str) -> bool {
    let Some(url) = parse_web_url(raw) else {
        return false;
    };
    url.host_str().is_some_and(|host| {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        host == "medium.com" || host.ends_with(".medium.com")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOOKMARK: &str = "https://example.com/a/b?x=1";

    #[test]
    fn scheme_case_and_http_versus_https_are_the_same_page() {
        assert!(same_page(BOOKMARK, "HTTPS://example.com/a/b?x=1"));
        assert!(same_page(BOOKMARK, "http://example.com/a/b?x=1"));
        assert!(same_page("http://Example.COM/a/b?x=1", BOOKMARK));
    }

    #[test]
    fn one_trailing_slash_is_ignored() {
        assert!(same_page(BOOKMARK, "https://example.com/a/b/?x=1"));
        assert!(same_page("https://example.com/a/b/?x=1", BOOKMARK));
        assert!(same_page("https://example.com/", "https://example.com"));
        // Exactly one trailing slash is ignored, not a run of them.
        assert!(!same_page(
            "https://example.com/a",
            "https://example.com/a//"
        ));
        assert!(!same_page(
            "https://example.com/a/",
            "https://example.com/A"
        ));
    }

    #[test]
    fn fragments_never_matter() {
        assert!(same_page(BOOKMARK, "https://example.com/a/b?x=1#section-3"));
        assert!(same_page(
            "https://example.com/a#top",
            "https://example.com/a"
        ));
    }

    #[test]
    fn tracking_parameters_are_dropped_and_real_ones_are_kept_in_any_order() {
        assert!(same_page(
            BOOKMARK,
            "https://example.com/a/b?utm_source=x&utm_medium=y&x=1&fbclid=abc"
        ));
        assert!(same_page(
            "https://example.com/a/b?a=1&b=2",
            "https://example.com/a/b?b=2&a=1"
        ));
        assert!(same_page(
            "https://example.com/a/b?utm_campaign=q&a=1&b=2&gclid=z",
            "https://example.com/a/b?b=2&a=1"
        ));
        // `source` and `ref` change what Medium shows, so they stay.
        assert!(!same_page(
            "https://example.com/a/b?source=home",
            "https://example.com/a/b?source=email"
        ));
        assert!(!same_page(
            "https://example.com/a/b?ref=home",
            "https://example.com/a/b"
        ));
        assert!(!same_page(
            "https://example.com/a?x=1",
            "https://example.com/a?x=2"
        ));
        assert!(!same_page(
            "https://example.com/a?x=1",
            "https://example.com/a?x=1&y=2"
        ));
    }

    #[test]
    fn paths_are_case_sensitive_and_ports_must_match() {
        assert!(!same_page("https://example.com/A", "https://example.com/a"));
        assert!(same_page(
            "https://example.com:443/a",
            "https://example.com/a"
        ));
        assert!(!same_page(
            "https://example.com:8443/a",
            "https://example.com/a"
        ));
        // 80 is http's default (so no explicit port) but not https's.
        assert!(!same_page(
            "http://example.com:80/a",
            "https://example.com:80/a"
        ));
    }

    #[test]
    fn different_hosts_and_userinfo_never_match() {
        assert!(!same_page(BOOKMARK, "https://www.example.com/a/b?x=1"));
        assert!(!same_page("https://example.com/a", "https://example.org/a"));
        assert!(!same_page(
            "https://user:pass@example.com/a",
            "https://example.com/a"
        ));
        assert!(crate::domain::url_match::parse_web_url("https://user@example.com/a").is_some());
        assert!(!has_userinfo(
            &parse_web_url("https://example.com/a").unwrap()
        ));
    }

    #[test]
    fn only_absolute_http_urls_are_page_urls() {
        assert!(parse_web_url("https://example.com/a").is_some());
        assert!(parse_web_url("http://example.com").is_some());
        for raw in [
            "",
            "  ",
            "example.com/a",
            "/a",
            "file:///etc/passwd",
            "ftp://example.com/a",
        ] {
            assert!(parse_web_url(raw).is_none(), "{raw}");
        }
        assert!(!same_page(BOOKMARK, "not a url"));
        assert!(!same_page("not a url", BOOKMARK));
    }

    #[test]
    fn a_default_port_does_not_match_a_non_default_one() {
        assert!(!same_page(
            "https://example.com/a",
            "https://example.com:8443/a"
        ));
    }

    #[test]
    fn medium_hosted_matches_medium_and_subdomains_only() {
        assert!(is_medium_url("https://medium.com/@a/post-1"));
        assert!(is_medium_url("https://Blog.Medium.com/post"));
        assert!(is_medium_url("http://www.medium.com/"));
        assert!(!is_medium_url("https://notmedium.com/a"));
        assert!(!is_medium_url("https://medium.com.evil.example/a"));
        assert!(!is_medium_url("https://example.com/medium.com"));
        assert!(!is_medium_url("not a url"));
    }
}
