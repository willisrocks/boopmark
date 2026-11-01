use crate::adapters::net::{
    Blocked, Lookup, SystemLookup, UrlGuard, guarded_client_builder, guarded_redirect_policy,
};
use crate::domain::bookmark::UrlMetadata;
use crate::domain::error::{CF_CHALLENGE_MSG, DomainError};
use crate::domain::ports::metadata::MetadataExtractor;
use ::scraper::{Html, Selector};
use std::future::Future;
use std::pin::Pin;
use url::Url;

#[derive(Clone)]
pub struct HtmlMetadataExtractor {
    client: reqwest::Client,
}

/// The guard's DNS and redirect checks apply to every hop (see `adapters::net`).
const MAX_REDIRECTS: usize = 10;

impl HtmlMetadataExtractor {
    pub fn new() -> Self {
        Self::with_lookup(UrlGuard::public(), SystemLookup)
    }

    fn with_lookup(guard: UrlGuard, lookup: impl Lookup) -> Self {
        Self {
            client: guarded_client_builder(
                guard,
                lookup,
                guarded_redirect_policy(guard, MAX_REDIRECTS),
            )
            .timeout(std::time::Duration::from_secs(10))
            .user_agent("Boopmark/1.0 (+https://boopmark.app)")
            .build()
            .unwrap(),
        }
    }
}

impl MetadataExtractor for HtmlMetadataExtractor {
    fn extract(
        &self,
        url_str: &str,
    ) -> Pin<Box<dyn Future<Output = Result<UrlMetadata, DomainError>> + Send + '_>> {
        let url_str = url_str.to_string();
        Box::pin(async move {
            let safe_url = crate::adapters::net::validate_public_url(&url_str)?;
            let parsed_url = Url::parse(&safe_url)
                .map_err(|e| DomainError::InvalidInput(format!("invalid URL: {e}")))?;

            let domain = parsed_url.host_str().map(|h| h.to_string());

            let resp =
                self.client.get(&safe_url).send().await.map_err(|e| {
                    match Blocked::in_chain(&e) {
                        Some(blocked) => DomainError::InvalidInput(blocked.to_string()),
                        None => DomainError::Internal(format!("fetch error: {e}")),
                    }
                })?;

            // Check the CF-Mitigated header before consuming the body
            if resp
                .headers()
                .get("cf-mitigated")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.eq_ignore_ascii_case("challenge"))
            {
                return Err(DomainError::Internal(CF_CHALLENGE_MSG.to_string()));
            }

            let html = resp
                .text()
                .await
                .map_err(|e| DomainError::Internal(format!("read error: {e}")))?;

            if is_cloudflare_challenge(&html) {
                return Err(DomainError::Internal(CF_CHALLENGE_MSG.to_string()));
            }

            let document = Html::parse_document(&html);

            let title = select_meta(&document, "og:title").or_else(|| {
                let sel = Selector::parse("title").ok()?;
                document
                    .select(&sel)
                    .next()
                    .map(|el| el.text().collect::<String>())
            });

            let description = select_meta(&document, "og:description")
                .or_else(|| select_meta_name(&document, "description"));

            let image_url = extract_image_url(&document).map(|img| resolve_url(&safe_url, &img));

            Ok(UrlMetadata {
                title,
                description,
                image_url,
                domain,
            })
        })
    }
}

pub(crate) fn is_cloudflare_challenge(body: &str) -> bool {
    // Check for the specific CF challenge title (not body text, which could appear in articles)
    body.contains("<title>Just a moment...</title>")
        || body.contains("Performing security verification")
}

fn select_meta(document: &Html, property: &str) -> Option<String> {
    let selector = Selector::parse(&format!("meta[property=\"{property}\"]")).ok()?;
    document
        .select(&selector)
        .next()?
        .value()
        .attr("content")
        .map(|s| s.to_string())
}

fn select_meta_name(document: &Html, name: &str) -> Option<String> {
    let selector = Selector::parse(&format!("meta[name=\"{name}\"]")).ok()?;
    document
        .select(&selector)
        .next()?
        .value()
        .attr("content")
        .map(|s| s.to_string())
}

fn extract_image_url(document: &Html) -> Option<String> {
    select_meta(document, "og:image")
        .or_else(|| select_meta_name(document, "og:image"))
        .or_else(|| select_meta(document, "twitter:image"))
        .or_else(|| select_meta_name(document, "twitter:image"))
}

fn resolve_url(base: &str, relative: &str) -> String {
    // Absolute URLs (http, https, data, etc.) are returned as-is
    if Url::parse(relative).is_ok() {
        return relative.to_string();
    }
    // Relative URLs are resolved against the base
    Url::parse(base)
        .and_then(|b| b.join(relative))
        .map(|u| u.to_string())
        .unwrap_or_else(|_| relative.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every name resolves to one fixed address.
    struct ResolvesTo(&'static str);

    impl Lookup for ResolvesTo {
        fn lookup(
            &self,
            _host: String,
        ) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<std::net::SocketAddr>>> + Send>>
        {
            let addr = std::net::SocketAddr::new(self.0.parse().unwrap(), 80);
            Box::pin(async move { Ok(vec![addr]) })
        }
    }

    #[tokio::test]
    async fn refuses_names_that_resolve_to_private_addresses() {
        let extractor =
            HtmlMetadataExtractor::with_lookup(UrlGuard::public(), ResolvesTo("10.0.0.7"));
        let err = extractor
            .extract("http://intranet.example.com/admin")
            .await
            .unwrap_err();
        assert!(
            matches!(&err, DomainError::InvalidInput(msg) if msg.contains("private address (10.0.0.7)")),
            "{err}"
        );
    }

    #[test]
    fn extracts_og_image_from_property() {
        let html = Html::parse_document(
            r#"<html><head>
                <meta property="og:image" content="https://example.com/image.png">
                <meta property="og:title" content="Test Title">
                <meta property="og:description" content="Test Desc">
            </head><body></body></html>"#,
        );
        let img = select_meta(&html, "og:image");
        assert_eq!(img, Some("https://example.com/image.png".to_string()));
    }

    #[test]
    fn falls_back_to_name_attribute() {
        let html = Html::parse_document(
            r#"<html><head>
                <meta name="og:image" content="https://example.com/name.png">
            </head><body></body></html>"#,
        );
        // select_meta (property) should return None
        assert_eq!(select_meta(&html, "og:image"), None);
        // select_meta_name should find it
        let img = select_meta_name(&html, "og:image");
        assert_eq!(img, Some("https://example.com/name.png".to_string()));
    }

    #[test]
    fn falls_back_to_twitter_image() {
        let html = Html::parse_document(
            r#"<html><head>
                <meta name="twitter:image" content="https://example.com/tw.png">
            </head><body></body></html>"#,
        );
        assert_eq!(
            extract_image_url(&html),
            Some("https://example.com/tw.png".to_string())
        );
    }

    #[test]
    fn no_image_meta_returns_none() {
        let html = Html::parse_document(
            r#"<html><head><title>No images</title></head><body></body></html>"#,
        );
        assert_eq!(extract_image_url(&html), None);
    }

    #[test]
    fn resolve_url_handles_absolute() {
        assert_eq!(
            resolve_url(
                "https://example.com/page",
                "https://cdn.example.com/img.jpg"
            ),
            "https://cdn.example.com/img.jpg"
        );
    }

    #[test]
    fn resolve_url_handles_relative() {
        assert_eq!(
            resolve_url("https://example.com/page", "/img.jpg"),
            "https://example.com/img.jpg"
        );
    }

    #[test]
    fn og_image_takes_priority_over_twitter_image() {
        let html = Html::parse_document(
            r#"<html><head>
                <meta property="og:image" content="https://example.com/og.png">
                <meta name="twitter:image" content="https://example.com/tw.png">
            </head><body></body></html>"#,
        );
        assert_eq!(
            extract_image_url(&html),
            Some("https://example.com/og.png".to_string())
        );
    }

    #[test]
    fn resolve_url_handles_data_uri() {
        let data_uri = "data:image/png;base64,iVBORw0KGgo=";
        assert_eq!(resolve_url("https://example.com/page", data_uri), data_uri);
    }

    #[test]
    fn detects_cloudflare_challenge_by_title() {
        let html = r#"<html><head><title>Just a moment...</title></head>
            <body>Performing security verification</body></html>"#;
        assert!(is_cloudflare_challenge(html));
    }

    #[test]
    fn detects_cloudflare_challenge_by_verification_text() {
        let html = r#"<html><head><title>Some Site</title></head>
            <body>Performing security verification</body></html>"#;
        assert!(is_cloudflare_challenge(html));
    }

    #[test]
    fn does_not_flag_normal_page_as_challenge() {
        let html = r#"<html><head><title>My Blog</title></head>
            <body><p>Hello world</p></body></html>"#;
        assert!(!is_cloudflare_challenge(html));
    }

    #[test]
    fn does_not_flag_page_mentioning_moment_in_body() {
        let html = r#"<html><head><title>Blog Post</title></head>
            <body><p>Just a moment... let me explain.</p></body></html>"#;
        assert!(!is_cloudflare_challenge(html));
    }
}
