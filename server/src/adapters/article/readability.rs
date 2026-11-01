use crate::adapters::metadata::html::is_cloudflare_challenge;
use crate::adapters::net::{
    Blocked, Lookup, PublicOnlyResolver, SystemLookup, UrlGuard, guarded_redirect_policy,
};
use crate::domain::error::CF_CHALLENGE_MSG;
use crate::domain::ports::article_fetcher::{ArticleFetchError, ArticleFetcher, FetchedArticle};
use crate::domain::search::ARTICLE_TEXT_CAP_CHARS;
use reqwest::StatusCode;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use url::Url;

/// Larger responses are skipped. The measured p90 page was 437 KB, the
/// largest 1.9 MB.
const MAX_HTML_BYTES: usize = 5 * 1024 * 1024;
const TOTAL_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_REDIRECTS: usize = 5;

/// Fetches a page and extracts its readable text with `dom_smoothie`, a
/// port of Mozilla's Readability.js.
///
/// Every request is guarded against SSRF: only public http(s) URLs, names
/// are resolved through `PublicOnlyResolver`, each redirect hop is checked
/// again, and proxies are disabled because a proxy would resolve names
/// itself.
pub struct HtmlArticleFetcher {
    client: reqwest::Client,
    guard: UrlGuard,
}

impl HtmlArticleFetcher {
    pub fn new() -> Self {
        Self::with_lookup(UrlGuard::public(), SystemLookup)
    }

    fn with_lookup(guard: UrlGuard, lookup: impl Lookup) -> Self {
        let client = reqwest::Client::builder()
            .user_agent("Boopmark/1.0 (+https://boopmark.app)")
            .timeout(TOTAL_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .no_proxy()
            .dns_resolver(Arc::new(PublicOnlyResolver::new(lookup, guard)))
            .redirect(guarded_redirect_policy(guard, MAX_REDIRECTS))
            .build()
            .expect("failed to build article HTTP client");
        Self { client, guard }
    }

    async fn fetch_article(&self, url: &str) -> Result<FetchedArticle, ArticleFetchError> {
        let parsed = Url::parse(url)
            .map_err(|e| ArticleFetchError::Permanent(format!("invalid URL: {e}")))?;
        self.guard
            .check(&parsed)
            .map_err(|e| ArticleFetchError::Permanent(e.to_string()))?;

        let mut resp = self
            .client
            .get(parsed)
            .send()
            .await
            .map_err(request_error)?;

        let status = resp.status();
        if !status.is_success() {
            return Err(status_error(status));
        }
        if resp
            .headers()
            .get("cf-mitigated")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.eq_ignore_ascii_case("challenge"))
        {
            return Err(ArticleFetchError::Permanent(CF_CHALLENGE_MSG.to_string()));
        }
        if let Some(content_type) = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            && !is_html(content_type)
        {
            return Err(ArticleFetchError::Skipped(format!(
                "not an HTML page: {content_type}"
            )));
        }
        if resp
            .content_length()
            .is_some_and(|len| len > MAX_HTML_BYTES as u64)
        {
            return Err(too_big());
        }

        let final_url = resp.url().to_string();
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(request_error)? {
            if body.len() + chunk.len() > MAX_HTML_BYTES {
                return Err(too_big());
            }
            body.extend_from_slice(&chunk);
        }
        let html_bytes = body.len();
        let html = String::from_utf8_lossy(&body).into_owned();
        if is_cloudflare_challenge(&html) {
            return Err(ArticleFetchError::Permanent(CF_CHALLENGE_MSG.to_string()));
        }

        let url_for_parse = final_url.clone();
        let text = tokio::task::spawn_blocking(move || extract_text(&html, &url_for_parse))
            .await
            .map_err(|e| ArticleFetchError::Transient(format!("extraction task failed: {e}")))??;

        Ok(FetchedArticle {
            text,
            final_url,
            html_bytes,
        })
    }
}

impl ArticleFetcher for HtmlArticleFetcher {
    fn fetch(
        &self,
        url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<FetchedArticle, ArticleFetchError>> + Send + '_>> {
        let url = url.to_string();
        Box::pin(async move { self.fetch_article(&url).await })
    }
}

fn is_html(content_type: &str) -> bool {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    mime == "text/html" || mime == "application/xhtml+xml"
}

fn too_big() -> ArticleFetchError {
    ArticleFetchError::Skipped(format!(
        "page is larger than {} MB",
        MAX_HTML_BYTES / (1024 * 1024)
    ))
}

fn status_error(status: StatusCode) -> ArticleFetchError {
    let message = format!("HTTP {}", status.as_u16());
    match status.as_u16() {
        408 | 425 | 429 | 500..=599 => ArticleFetchError::Transient(message),
        _ => ArticleFetchError::Permanent(message),
    }
}

/// Refusals by the guard are permanent; everything else that can go wrong
/// in flight (timeouts, resets, DNS failures) is worth a retry. The message
/// names the failure without the URL, which the caller already has.
fn request_error(err: reqwest::Error) -> ArticleFetchError {
    if let Some(blocked) = Blocked::in_chain(&err) {
        return ArticleFetchError::Permanent(blocked.to_string());
    }
    if err.is_redirect() {
        return ArticleFetchError::Permanent("too many redirects".to_string());
    }
    let kind = if err.is_timeout() {
        "timed out"
    } else if err.is_connect() {
        "connection failed"
    } else if err.is_body() || err.is_decode() {
        "reading the response failed"
    } else {
        "request failed"
    };
    ArticleFetchError::Transient(kind.to_string())
}

/// The readable text of an HTML document, paragraphs separated by blank
/// lines, capped at `ARTICLE_TEXT_CAP_CHARS`.
pub(crate) fn extract_text(html: &str, url: &str) -> Result<String, ArticleFetchError> {
    let config = dom_smoothie::Config {
        text_mode: dom_smoothie::TextMode::Formatted,
        ..Default::default()
    };
    let article = dom_smoothie::Readability::new(html, Some(url), Some(config))
        .and_then(|mut readability| readability.parse())
        .map_err(|e| ArticleFetchError::Permanent(format!("no readable article: {e}")))?;
    let text = article.text_content.trim();
    if text.is_empty() {
        return Err(ArticleFetchError::Permanent(
            "no readable article: empty text".to_string(),
        ));
    }
    let end = text
        .char_indices()
        .nth(ARTICLE_TEXT_CAP_CHARS)
        .map_or(text.len(), |(i, _)| i);
    Ok(text[..end].trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::http::header;
    use axum::response::{IntoResponse, Redirect};
    use axum::routing::get;
    use std::net::SocketAddr;

    const ARTICLE: &str = r#"<!doctype html><html><head><title>Borrowing in Rust</title></head>
        <body>
          <nav><a href="/">Home</a> <a href="/about">About</a></nav>
          <article>
            <h1>Borrowing in Rust</h1>
            <p>The borrow checker enforces that every reference is valid for as long as it is
               used, and that mutable references are never aliased. This paragraph is long enough
               for a readability heuristic to pick it as the main content of the page.</p>
            <p>Lifetimes are the names the compiler gives to those regions of validity. Most of the
               time they are inferred, and you only write them when a function returns a
               reference derived from one of several arguments.</p>
            <p>Interior mutability, through Cell and RefCell, moves the aliasing check from compile
               time to run time, which is occasionally exactly what a data structure needs.</p>
          </article>
          <footer>Copyright footer text that should not matter</footer>
        </body></html>"#;

    /// Maps fixture host names to addresses; unknown names do not resolve.
    struct FixtureLookup(Vec<(&'static str, SocketAddr)>);

    impl Lookup for FixtureLookup {
        fn lookup(
            &self,
            host: String,
        ) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<SocketAddr>>> + Send>> {
            let addrs = self
                .0
                .iter()
                .filter(|(name, _)| *name == host)
                .map(|(_, addr)| *addr)
                .collect();
            Box::pin(async move { Ok(addrs) })
        }
    }

    /// Serves the fixture routes on 127.0.0.1 and returns a fetcher that
    /// reaches it as `fixture.test`, with `intranet.test` resolving to a
    /// private address.
    async fn fixture() -> (HtmlArticleFetcher, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base = format!("http://fixture.test:{}", addr.port());
        let intranet = format!("http://intranet.test:{}/article", addr.port());
        let app = Router::new()
            .route(
                "/article",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                        ARTICLE,
                    )
                }),
            )
            .route(
                "/pdf",
                get(|| async { ([(header::CONTENT_TYPE, "application/pdf")], "%PDF-1.7") }),
            )
            .route(
                "/huge",
                get(|| async {
                    let body = format!(
                        "<html><body><p>{}</p></body></html>",
                        "a ".repeat(3_000_000)
                    );
                    ([(header::CONTENT_TYPE, "text/html")], body)
                }),
            )
            .route("/missing", get(|| async { StatusCode::NOT_FOUND }))
            .route("/busy", get(|| async { StatusCode::SERVICE_UNAVAILABLE }))
            .route(
                "/slow-down",
                get(|| async { StatusCode::TOO_MANY_REQUESTS }),
            )
            .route(
                "/challenge",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "text/html")],
                        "<html><head><title>Just a moment...</title></head></html>",
                    )
                }),
            )
            .route(
                "/to-private-ip",
                get(|| async { Redirect::temporary("http://10.0.0.1/admin") }),
            )
            .route(
                "/to-private-name",
                get(move || {
                    let intranet = intranet.clone();
                    async move { Redirect::temporary(&intranet).into_response() }
                }),
            )
            .route(
                "/to-file",
                get(|| async { Redirect::temporary("file:///etc/passwd") }),
            )
            .route(
                "/to-article",
                get(|| async { Redirect::temporary("/article") }),
            )
            .route("/loop", get(|| async { Redirect::temporary("/loop") }));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let lookup = FixtureLookup(vec![
            ("fixture.test", addr),
            (
                "intranet.test",
                SocketAddr::new("10.0.0.7".parse().unwrap(), addr.port()),
            ),
        ]);
        (
            HtmlArticleFetcher::with_lookup(UrlGuard::allowing_loopback(), lookup),
            base,
        )
    }

    #[tokio::test]
    async fn fetches_and_extracts_the_article_text() {
        let (fetcher, base) = fixture().await;
        let article = fetcher.fetch(&format!("{base}/article")).await.unwrap();
        assert!(article.text.contains("The borrow checker enforces"));
        assert!(article.text.contains("Interior mutability"));
        assert!(!article.text.contains("Copyright footer"));
        assert!(!article.text.contains("About"));
        assert!(article.text.contains("\n\n"), "paragraphs are kept apart");
        assert_eq!(article.final_url, format!("{base}/article"));
        assert_eq!(article.html_bytes, ARTICLE.len());
    }

    #[tokio::test]
    async fn follows_safe_redirects() {
        let (fetcher, base) = fixture().await;
        let article = fetcher.fetch(&format!("{base}/to-article")).await.unwrap();
        assert_eq!(article.final_url, format!("{base}/article"));
    }

    #[tokio::test]
    async fn skips_non_html_and_oversized_pages() {
        let (fetcher, base) = fixture().await;
        assert!(matches!(
            fetcher.fetch(&format!("{base}/pdf")).await,
            Err(ArticleFetchError::Skipped(msg)) if msg.contains("application/pdf")
        ));
        assert!(matches!(
            fetcher.fetch(&format!("{base}/huge")).await,
            Err(ArticleFetchError::Skipped(msg)) if msg.contains("larger than 5 MB")
        ));
    }

    #[tokio::test]
    async fn classifies_http_errors() {
        let (fetcher, base) = fixture().await;
        assert_eq!(
            fetcher.fetch(&format!("{base}/missing")).await,
            Err(ArticleFetchError::Permanent("HTTP 404".into()))
        );
        assert_eq!(
            fetcher.fetch(&format!("{base}/busy")).await,
            Err(ArticleFetchError::Transient("HTTP 503".into()))
        );
        assert_eq!(
            fetcher.fetch(&format!("{base}/slow-down")).await,
            Err(ArticleFetchError::Transient("HTTP 429".into()))
        );
        assert_eq!(
            fetcher.fetch(&format!("{base}/challenge")).await,
            Err(ArticleFetchError::Permanent(CF_CHALLENGE_MSG.into()))
        );
    }

    #[tokio::test]
    async fn refuses_redirects_to_private_addresses_and_other_schemes() {
        let (fetcher, base) = fixture().await;
        for path in ["/to-private-ip", "/to-private-name"] {
            let result = fetcher.fetch(&format!("{base}{path}")).await;
            assert!(
                matches!(&result, Err(ArticleFetchError::Permanent(msg)) if msg.contains("refusing")),
                "{path}: {result:?}"
            );
        }
        // reqwest never follows a redirect to a non-http scheme; the 3xx is final.
        assert_eq!(
            fetcher.fetch(&format!("{base}/to-file")).await,
            Err(ArticleFetchError::Permanent("HTTP 307".into()))
        );
        assert_eq!(
            fetcher.fetch(&format!("{base}/loop")).await,
            Err(ArticleFetchError::Permanent("too many redirects".into()))
        );
    }

    #[tokio::test]
    async fn production_guard_refuses_private_targets_before_connecting() {
        let fetcher = HtmlArticleFetcher::new();
        for url in [
            "http://127.0.0.1/",
            "http://localhost:4000/",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]/",
            "ftp://example.com/",
        ] {
            assert!(
                matches!(
                    fetcher.fetch(url).await,
                    Err(ArticleFetchError::Permanent(_))
                ),
                "{url}"
            );
        }
    }

    #[tokio::test]
    async fn unresolvable_names_are_transient() {
        let (fetcher, _) = fixture().await;
        assert!(matches!(
            fetcher.fetch("http://unknown.test/").await,
            Err(ArticleFetchError::Transient(_))
        ));
    }

    #[test]
    fn extraction_caps_text_on_a_char_boundary() {
        let paragraph = format!("<p>{}</p>", "é".repeat(1000));
        let html = format!(
            "<html><body><article>{}</article></body></html>",
            paragraph.repeat(ARTICLE_TEXT_CAP_CHARS / 1000 + 20)
        );
        let text = extract_text(&html, "https://example.com/").unwrap();
        assert!(text.chars().count() <= ARTICLE_TEXT_CAP_CHARS);
        assert!(text.chars().count() > ARTICLE_TEXT_CAP_CHARS - 100);
    }

    #[test]
    fn extraction_fails_on_an_empty_document() {
        assert!(matches!(
            extract_text("<html><body></body></html>", "https://example.com/"),
            Err(ArticleFetchError::Permanent(_))
        ));
    }

    #[test]
    fn content_type_check_ignores_parameters_and_case() {
        assert!(is_html("text/html"));
        assert!(is_html("Text/HTML; charset=ISO-8859-1"));
        assert!(is_html("application/xhtml+xml"));
        assert!(!is_html("application/pdf"));
        assert!(!is_html("text/plain"));
    }
}
