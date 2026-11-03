use crate::adapters::article::gzip::{GzipError, gunzip_capped};
use crate::adapters::metadata::html::{is_cloudflare_challenge, is_cloudflare_challenge_response};
use crate::adapters::net::{
    CappedReadError, GuardedGetError, HostThrottle, Lookup, SystemLookup, UrlGuard,
    guarded_client_builder, guarded_get, mime_type, read_capped,
};
use crate::domain::error::CF_CHALLENGE_MSG;
use crate::domain::ports::article_fetcher::{ArticleFetchError, ArticleFetcher, FetchedArticle};
use crate::domain::search::{
    ARTICLE_TEXT_CAP_BYTES, ARTICLE_TEXT_CAP_CHARS, SNIPPET_START, SNIPPET_STOP,
};
use encoding_rs::{Encoding, UTF_8};
use reqwest::StatusCode;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use url::Url;

/// Larger responses are skipped. The measured p90 page was 437 KB, the
/// largest 1.9 MB.
const MAX_HTML_BYTES: usize = 5 * 1024 * 1024;
/// Per request, so per redirect hop.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Readability on a hostile page has no natural bound. The blocking thread
/// cannot be cancelled, but the fetch gives up on it and moves on.
const EXTRACT_TIMEOUT: Duration = Duration::from_secs(20);
/// At most one request per host per interval, redirect hops included:
/// imports are often many links to one site.
const PER_HOST_INTERVAL: Duration = Duration::from_secs(1);

/// Fetches a page and extracts its readable text with `dom_smoothie`, a
/// port of Mozilla's Readability.js.
///
/// Every request is guarded against SSRF: only public http(s) URLs on web
/// ports, names are resolved through `PublicOnlyResolver`, proxies are
/// disabled, and `guarded_get` checks every redirect hop and makes it wait
/// for its host's slot.
pub struct HtmlArticleFetcher {
    client: reqwest::Client,
    guard: UrlGuard,
    throttle: HostThrottle,
    extractor: HtmlExtractor,
}

/// Decode, parse and cap an HTML body: the one Readability pass the app
/// runs, shared by the fetcher and the capture endpoint, so both have the
/// same `EXTRACT_TIMEOUT`, the same text caps and the same blocking pool.
///
/// Each holder has its own slots; a capture therefore parses even when the
/// ingest worker's extractor is busy or was never built
/// (`ARTICLE_INGEST_ENABLED=0`).
pub struct HtmlExtractor {
    /// Extractions running at once, timed-out ones included.
    slots: Arc<Semaphore>,
}

/// Why a captured body produced no text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureExtractError {
    /// The body is not usable gzip, or inflates past the cap.
    Gzip(GzipError),
    /// The parse found no article (or the document is unreadable).
    NoArticle(ArticleFetchError),
    /// The extraction did not finish: it timed out, or its task failed. The
    /// caller keeps the payload and the drain tries again.
    Unfinished(String),
}

impl HtmlExtractor {
    /// `slots` bounds the blocking threads that parse pages.
    pub fn new(slots: usize) -> Self {
        Self {
            slots: Arc::new(Semaphore::new(slots.max(1))),
        }
    }

    /// A captured page: inflate, decode and parse on one blocking slot, so
    /// no work for a 5 MiB body happens on an async worker thread. The body
    /// is shared, not moved, so a caller that gets [`CaptureExtractError::Unfinished`]
    /// can still keep it for the drain.
    ///
    /// There is no challenge check: if the user's browser rendered a
    /// challenge, there is no article, and saying so is honest.
    pub async fn extract_captured(
        &self,
        gz: Arc<Vec<u8>>,
        max_bytes: usize,
        content_type: Option<&str>,
        url: &str,
    ) -> Result<String, CaptureExtractError> {
        let content_type = content_type.map(str::to_string);
        let url = url.to_string();
        let parsed = run_bounded(&self.slots, EXTRACT_TIMEOUT, move || {
            let html = gunzip_capped(&gz, max_bytes).map_err(CaptureExtractError::Gzip)?;
            let html = decode_html(html, content_type.as_deref());
            extract_text(&html, &url).map_err(CaptureExtractError::NoArticle)
        })
        .await;
        match parsed {
            Ok(result) => result,
            Err(err) => Err(CaptureExtractError::Unfinished(err.to_string())),
        }
    }

    /// A fetched page: decode and parse, and report a challenge page as such
    /// rather than as "no readable article".
    pub async fn extract_fetched(
        &self,
        body: Vec<u8>,
        content_type: Option<&str>,
        url: &str,
    ) -> Result<String, ArticleFetchError> {
        let content_type = content_type.map(str::to_string);
        let url = url.to_string();
        run_bounded(&self.slots, EXTRACT_TIMEOUT, move || {
            let html = decode_html(body, content_type.as_deref());
            if is_cloudflare_challenge(&html) {
                return Err(ArticleFetchError::Permanent(CF_CHALLENGE_MSG.to_string()));
            }
            extract_text(&html, &url)
        })
        .await?
    }
}

impl HtmlArticleFetcher {
    /// `extraction_slots` bounds the blocking threads that parse pages; the
    /// ingest worker passes its concurrency.
    pub fn new(extraction_slots: usize) -> Self {
        Self {
            extractor: HtmlExtractor::new(extraction_slots),
            ..Self::with_lookup(UrlGuard::public(), SystemLookup, PER_HOST_INTERVAL)
        }
    }

    fn with_lookup(guard: UrlGuard, lookup: impl Lookup, per_host_interval: Duration) -> Self {
        let client = guarded_client_builder(guard, lookup)
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .expect("failed to build article HTTP client");
        Self {
            client,
            guard,
            throttle: HostThrottle::new(per_host_interval),
            extractor: HtmlExtractor::new(1),
        }
    }

    async fn fetch_article(&self, url: &str) -> Result<FetchedArticle, ArticleFetchError> {
        let parsed = Url::parse(url)
            .map_err(|e| ArticleFetchError::Permanent(format!("invalid URL: {e}")))?;
        let resp = guarded_get(&self.client, self.guard, Some(&self.throttle), parsed)
            .await
            .map_err(|e| match e {
                GuardedGetError::Blocked(blocked) => {
                    ArticleFetchError::Permanent(blocked.to_string())
                }
                GuardedGetError::TooManyRedirects => {
                    ArticleFetchError::Permanent("too many redirects".to_string())
                }
                GuardedGetError::Request(e) => request_error(e),
            })?;

        let status = resp.status();
        if !status.is_success() {
            return Err(status_error(status));
        }
        if is_cloudflare_challenge_response(resp.headers()) {
            return Err(ArticleFetchError::Permanent(CF_CHALLENGE_MSG.to_string()));
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        if let Some(content_type) = &content_type
            && !is_html(content_type)
        {
            return Err(ArticleFetchError::Skipped(format!(
                "not an HTML page: {content_type}"
            )));
        }
        let final_url = resp.url().to_string();
        let body = read_capped(resp, MAX_HTML_BYTES)
            .await
            .map_err(|e| match e {
                CappedReadError::TooBig => too_big(),
                CappedReadError::Read(e) => request_error(e),
            })?;
        let html_bytes = body.len();
        // Decoding, the challenge check and extraction all scan the page, so
        // they run off the async threads.
        let text = self
            .extractor
            .extract_fetched(body, content_type.as_deref(), &final_url)
            .await?;

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

/// Run `work` on the blocking pool once one of `slots` is free, waiting at
/// most `timeout` in all. A blocking thread cannot be cancelled, so the
/// permit moves into the closure: a timed-out run keeps its slot until its
/// thread really ends, and hostile pages can never have more extractions
/// running than there are slots.
async fn run_bounded<T: Send + 'static>(
    slots: &Arc<Semaphore>,
    timeout: Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ArticleFetchError> {
    let slots = slots.clone();
    let run = async move {
        let permit = slots
            .acquire_owned()
            .await
            .expect("the extraction semaphore is never closed");
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            work()
        })
        .await
    };
    tokio::time::timeout(timeout, run)
        .await
        .map_err(|_| ArticleFetchError::Transient("extraction timed out".to_string()))?
        .map_err(|e| ArticleFetchError::Transient(format!("extraction task failed: {e}")))
}

fn is_html(content_type: &str) -> bool {
    let mime = mime_type(content_type);
    mime == "text/html" || mime == "application/xhtml+xml"
}

/// Decode an HTML body: a byte-order mark wins, then the `Content-Type`
/// charset, then a `charset=` declaration in the first 1 KB (a `<meta>`
/// tag), then UTF-8. Undecodable bytes become U+FFFD.
fn decode_html(body: Vec<u8>, content_type: Option<&str>) -> String {
    let declared = content_type
        .and_then(charset_label)
        .or_else(|| charset_label(&String::from_utf8_lossy(&body[..body.len().min(1024)])));
    let encoding = declared
        .and_then(|label| Encoding::for_label(label.as_bytes()))
        .unwrap_or(UTF_8);
    if encoding == UTF_8 && Encoding::for_bom(&body).is_none() {
        // The common case: reuse the buffer when it is valid UTF-8.
        return String::from_utf8(body)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
    }
    // `decode` sniffs the BOM itself and lets it override the label.
    encoding.decode(&body).0.into_owned()
}

/// The value of the first `charset = value` in `text` (whitespace around
/// `=` allowed), unquoted.
fn charset_label(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let value = lower
        .match_indices("charset")
        .find_map(|(i, word)| lower[i + word.len()..].trim_start().strip_prefix('='))?;
    let label: String = value
        .trim_start()
        .trim_start_matches(['"', '\''])
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
        .collect();
    (!label.is_empty()).then_some(label)
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

/// What goes wrong in flight (timeouts, resets, DNS failures) is worth a
/// retry; `guarded_get` has already turned the guard's refusals into
/// `Blocked`. The message names the failure without the URL, which the
/// caller already has.
fn request_error(err: reqwest::Error) -> ArticleFetchError {
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
/// lines, capped at `ARTICLE_TEXT_CAP_CHARS` and `ARTICLE_TEXT_CAP_BYTES`.
pub(crate) fn extract_text(html: &str, url: &str) -> Result<String, ArticleFetchError> {
    let config = dom_smoothie::Config {
        text_mode: dom_smoothie::TextMode::Formatted,
        ..Default::default()
    };
    let article = dom_smoothie::Readability::new(html, Some(url), Some(config))
        .and_then(|mut readability| readability.parse())
        .map_err(|e| ArticleFetchError::Permanent(format!("no readable article: {e}")))?;
    // NUL cannot be stored in a TEXT column, and the snippet markers must stay
    // unambiguous.
    let text: String = article
        .text_content
        .chars()
        .filter(|c| !matches!(*c, '\0' | SNIPPET_START | SNIPPET_STOP))
        .collect();
    let text = text.trim();
    if text.is_empty() {
        return Err(ArticleFetchError::Permanent(
            "no readable article: empty text".to_string(),
        ));
    }
    let mut end = text.len().min(ARTICLE_TEXT_CAP_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    if let Some((i, _)) = text[..end].char_indices().nth(ARTICLE_TEXT_CAP_CHARS) {
        end = i;
    }
    Ok(text[..end].trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::net::FakeLookup;
    use axum::Router;
    use axum::http::header;
    use axum::response::{IntoResponse, Redirect};
    use axum::routing::get;
    use std::collections::HashMap;

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

    /// The fixture article with an accented paragraph, encoded as Latin-1.
    fn latin1_article() -> Vec<u8> {
        let html = ARTICLE.replace(
            "<p>Lifetimes",
            "<p>Caf\u{e9} cr\u{e8}me na\u{ef}ve r\u{e9}sum\u{e9}, a paragraph about accents. </p><p>Lifetimes",
        );
        encoding_rs::WINDOWS_1252.encode(&html).0.into_owned()
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
            .route(
                "/latin1",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "text/html; charset=ISO-8859-1")],
                        latin1_article(),
                    )
                }),
            )
            .route(
                "/meta-charset",
                get(|| async {
                    let mut body = b"<html><head><meta charset=\"windows-1252\"></head>".to_vec();
                    body.extend(latin1_article());
                    ([(header::CONTENT_TYPE, "text/html")], body)
                }),
            )
            .route("/loop", get(|| async { Redirect::temporary("/loop") }));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let lookup = FakeLookup(HashMap::from([
            ("fixture.test", vec!["127.0.0.1"]),
            ("intranet.test", vec!["10.0.0.7"]),
        ]));
        (
            HtmlArticleFetcher::with_lookup(UrlGuard::allowing_loopback(), lookup, Duration::ZERO),
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
        assert_eq!(
            fetcher.fetch(&format!("{base}/to-file")).await,
            Err(ArticleFetchError::Permanent(
                "unsupported scheme: file".into()
            ))
        );
        assert_eq!(
            fetcher.fetch(&format!("{base}/loop")).await,
            Err(ArticleFetchError::Permanent("too many redirects".into()))
        );
    }

    #[tokio::test]
    async fn production_guard_refuses_private_targets_before_connecting() {
        let fetcher = HtmlArticleFetcher::new(1);
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
    fn extraction_caps_multibyte_text_by_bytes() {
        let paragraph = format!("<p>{}</p>", "\u{20ac}".repeat(1000));
        let html = format!(
            "<html><body><article>{}</article></body></html>",
            paragraph.repeat(ARTICLE_TEXT_CAP_CHARS / 1000)
        );
        let text = extract_text(&html, "https://example.com/").unwrap();
        assert!(text.len() <= ARTICLE_TEXT_CAP_BYTES);
        assert!(text.len() > ARTICLE_TEXT_CAP_BYTES - 4096);
    }

    #[tokio::test]
    async fn decodes_the_declared_charset() {
        let (fetcher, base) = fixture().await;
        for path in ["/latin1", "/meta-charset"] {
            let article = fetcher.fetch(&format!("{base}{path}")).await.unwrap();
            assert!(
                article
                    .text
                    .contains("Caf\u{e9} cr\u{e8}me na\u{ef}ve r\u{e9}sum\u{e9}"),
                "{path}: {}",
                article.text
            );
        }
    }

    #[test]
    fn charset_comes_from_bom_header_meta_then_utf8() {
        let latin1 = [b'c', b'a', b'f', 0xe9];
        assert_eq!(
            decode_html(latin1.to_vec(), Some("text/html; charset=latin1")),
            "caf\u{e9}"
        );
        assert_eq!(
            decode_html(latin1.to_vec(), Some("text/html")),
            "caf\u{fffd}"
        );
        let bom = [0xef, 0xbb, 0xbf, b'h', b'i'];
        assert_eq!(
            decode_html(bom.to_vec(), Some("text/html; charset=latin1")),
            "hi"
        );
        assert_eq!(
            charset_label(
                "<meta http-equiv=\"Content-Type\" content=\"text/html; charset=Shift_JIS\">"
            ),
            Some("shift_jis".into())
        );
        assert_eq!(
            charset_label("<meta charset='utf-8'>"),
            Some("utf-8".into())
        );
        assert_eq!(charset_label("<html>"), None);
        assert_eq!(
            charset_label(
                "<meta http-equiv=\"Content-Type\" content=\"text/html; charset = ISO-8859-1\">"
            ),
            Some("iso-8859-1".into())
        );
        assert_eq!(
            charset_label("text/html; charset =\"koi8-r\""),
            Some("koi8-r".into())
        );
        assert_eq!(
            charset_label("charset and more; charset=utf-8"),
            Some("utf-8".into())
        );
    }

    #[tokio::test]
    async fn redirect_hops_wait_for_their_host_slot() {
        let (_, base) = fixture().await;
        let lookup = FakeLookup(HashMap::from([("fixture.test", vec!["127.0.0.1"])]));
        let interval = Duration::from_millis(300);
        let fetcher =
            HtmlArticleFetcher::with_lookup(UrlGuard::allowing_loopback(), lookup, interval);
        let started = std::time::Instant::now();
        fetcher.fetch(&format!("{base}/to-article")).await.unwrap();
        assert!(started.elapsed() >= interval, "the hop to /article waited");
    }

    #[test]
    fn extraction_fails_on_an_empty_document() {
        assert!(matches!(
            extract_text("<html><body></body></html>", "https://example.com/"),
            Err(ArticleFetchError::Permanent(_))
        ));
    }

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    /// The capture path and the fetch path run the same parse: the text of a
    /// captured page is the text the fetcher would have stored.
    #[tokio::test]
    async fn a_captured_page_extracts_like_a_fetched_one() {
        let (fetcher, base) = fixture().await;
        let url = format!("{base}/article");
        let fetched = fetcher.fetch(&url).await.unwrap();
        let captured = HtmlExtractor::new(1)
            .extract_captured(
                Arc::new(gzip(ARTICLE.as_bytes())),
                5 * 1024 * 1024,
                Some("text/html"),
                &url,
            )
            .await
            .unwrap();
        assert_eq!(captured, fetched.text);
    }

    #[tokio::test]
    async fn a_capture_with_no_article_is_an_honest_refusal() {
        let extractor = HtmlExtractor::new(1);
        let cap = 5 * 1024 * 1024;
        let empty = extractor
            .extract_captured(
                Arc::new(gzip(b"<html><body></body></html>")),
                cap,
                Some("text/html"),
                "https://example.com/",
            )
            .await;
        assert!(matches!(empty, Err(CaptureExtractError::NoArticle(_))));
        // The browser rendered what it rendered: a challenge page from a
        // capture is reported as having no article, not as a fetch block.
        let challenge = extractor
            .extract_captured(
                Arc::new(gzip(
                    b"<html><head><title>Just a moment...</title></head></html>",
                )),
                cap,
                Some("text/html"),
                "https://example.com/",
            )
            .await;
        assert!(
            matches!(
                &challenge,
                Err(CaptureExtractError::NoArticle(ArticleFetchError::Permanent(msg)))
                    if !msg.contains(CF_CHALLENGE_MSG)
            ),
            "{challenge:?}"
        );
        // A body that is not gzip, or over the cap, never reaches the parse.
        assert_eq!(
            extractor
                .extract_captured(
                    Arc::new(b"not gzip".to_vec()),
                    cap,
                    Some("text/html"),
                    "https://example.com/"
                )
                .await,
            Err(CaptureExtractError::Gzip(GzipError::Corrupt))
        );
        assert_eq!(
            extractor
                .extract_captured(
                    Arc::new(gzip(b"<html><body><p>a</p></body></html>")),
                    4,
                    Some("text/html"),
                    "https://example.com/"
                )
                .await,
            Err(CaptureExtractError::Gzip(GzipError::TooLarge))
        );
    }

    #[test]
    fn content_type_check_ignores_parameters_and_case() {
        assert!(is_html("text/html"));
        assert!(is_html("Text/HTML; charset=ISO-8859-1"));
        assert!(is_html("application/xhtml+xml"));
        assert!(!is_html("application/pdf"));
        assert!(!is_html("text/plain"));
    }

    /// A run that outlives its timeout keeps its slot until its thread ends,
    /// so the next run waits instead of starting a second extraction.
    #[tokio::test]
    async fn a_timed_out_extraction_keeps_its_slot_until_it_ends() {
        let slots = Arc::new(Semaphore::new(1));
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let stuck = run_bounded(&slots, Duration::from_millis(50), move || {
            wait.recv().ok();
        })
        .await;
        assert_eq!(
            stuck,
            Err(ArticleFetchError::Transient("extraction timed out".into()))
        );
        assert_eq!(
            slots.available_permits(),
            0,
            "the stuck thread holds the slot"
        );
        let queued = run_bounded(&slots, Duration::from_millis(50), || 1).await;
        assert!(queued.is_err(), "no second extraction while the first runs");

        release.send(()).unwrap();
        let ran = run_bounded(&slots, Duration::from_secs(5), || 2).await;
        assert_eq!(ran, Ok(2));
        assert_eq!(slots.available_permits(), 1);
    }
}
