//! Firecrawl adapter for the `PageScraper` port: one hosted `/scrape` call per
//! page and no crawl. The vendor renders the page from its own network and
//! answers with the HTML, which the app then extracts with its own Readability
//! pass.
//!
//! `formats: ["rawHtml", "html"]` is one credit either way; the raw HTML is
//! what holds Medium's page state, and therefore the lock marker, so it is
//! preferred and the cleaned `html` is the fallback.

use crate::adapters::vendor_http::{KeyOwner, post_json};
use crate::domain::error::DomainError;
use crate::domain::ports::page_scraper::{BoxFuture, PageScraper, ScrapedPage};
use serde_json::{Value, json};
use std::time::Duration;

const VENDOR: &str = "Firecrawl";
const SCRAPE_PATH: &str = "/v2/scrape";
pub const DEFAULT_BASE_URL: &str = "https://api.firecrawl.dev";

/// Medium marks a member-only story in the page state it embeds, so the HTML
/// of a signed-out reader holds the preview, not the article. The eval found
/// this marker necessary: without it 49 previews of 1,500–2,400 characters
/// counted as full articles.
pub const LOCKED_MARKER: &str = "\"isLocked\":true";

pub struct FirecrawlScraper {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
}

/// Hand-written so the operator key never reaches a log line.
impl std::fmt::Debug for FirecrawlScraper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FirecrawlScraper")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, serde::Deserialize)]
struct ScrapeResponse {
    /// Absent on some payloads; `false` means the vendor refused the request.
    success: Option<bool>,
    data: Option<ScrapeData>,
}

#[derive(Debug, serde::Deserialize)]
struct ScrapeData {
    #[serde(rename = "rawHtml")]
    raw_html: Option<String>,
    html: Option<String>,
    metadata: Option<ScrapeMetadata>,
}

#[derive(Debug, serde::Deserialize)]
struct ScrapeMetadata {
    /// The page's URL after redirects.
    #[serde(rename = "sourceURL")]
    source_url: Option<String>,
    /// The HTTP status the target page returned, which is separate from the
    /// API call's own status.
    #[serde(rename = "statusCode")]
    status_code: Option<u16>,
    /// Some vendors report the paywall as metadata rather than in the HTML.
    #[serde(rename = "isLocked")]
    is_locked: Option<bool>,
}

impl FirecrawlScraper {
    pub fn new(api_key: String, timeout: Duration) -> Self {
        Self::with_base_url(api_key, DEFAULT_BASE_URL.to_string(), timeout)
    }

    fn with_base_url(api_key: String, base_url: String, timeout: Duration) -> Self {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("failed to build the Firecrawl HTTP client");
        Self {
            client,
            api_key,
            base_url,
        }
    }

    /// The page's HTML plus the vendor's verdict on it.
    ///
    /// The API's own status is checked first (the `post_json` mapping, with
    /// the operator owning the key); then `success: false` and a payload
    /// without HTML are final, since the same request cannot return a page
    /// later. A target page that answered a non-2xx status is final too: a
    /// "404 Not Found" body is not an article.
    fn interpret(payload: Value, requested_url: &str) -> Result<ScrapedPage, DomainError> {
        let parsed: ScrapeResponse = serde_json::from_value(payload).map_err(|_| {
            DomainError::Internal(format!("{VENDOR} returned a malformed response"))
        })?;
        if parsed.success == Some(false) {
            return Err(DomainError::InvalidInput(format!(
                "{VENDOR} refused the scrape"
            )));
        }
        let data = parsed
            .data
            .ok_or_else(|| DomainError::Internal(format!("{VENDOR} returned no page")))?;
        let metadata = data.metadata;
        if let Some(status) = metadata.as_ref().and_then(|m| m.status_code)
            && !(200..300).contains(&status)
            && status != 304
        {
            return Err(DomainError::InvalidInput(format!(
                "{VENDOR} page returned HTTP {status}"
            )));
        }
        let html = data
            .raw_html
            .or(data.html)
            .filter(|html| !html.trim().is_empty())
            .ok_or_else(|| {
                DomainError::InvalidInput(format!("{VENDOR} returned no HTML for the page"))
            })?;
        let locked = metadata.as_ref().and_then(|m| m.is_locked) == Some(true)
            || html.contains(LOCKED_MARKER);
        let final_url = metadata
            .and_then(|m| m.source_url)
            .unwrap_or_else(|| requested_url.to_string());
        Ok(ScrapedPage {
            html_bytes: html.len(),
            html,
            final_url,
            locked,
        })
    }
}

impl PageScraper for FirecrawlScraper {
    fn scrape(&self, url: &str) -> BoxFuture<'_, Result<ScrapedPage, DomainError>> {
        let url = url.to_string();
        Box::pin(async move {
            let body = json!({
                "url": url,
                "formats": ["rawHtml", "html"],
                // The article's own body is what Readability needs; the
                // vendor's main-content guess would drop parts of it.
                "onlyMainContent": false,
            });
            let request = self
                .client
                .post(format!("{}{SCRAPE_PATH}", self.base_url))
                .bearer_auth(&self.api_key);
            // A rejected operator key is retryable here as everywhere else
            // that uses vendor_http: the key is rotated out of band, and the
            // ingest retry schedule covers the wait.
            let payload = post_json(request, &body, VENDOR, KeyOwner::Operator).await?;
            Self::interpret(payload, &url)
        })
    }
}

/// Kept beside the adapter so a test can assert the request body without
/// restating it.
#[cfg(test)]
pub(crate) fn expected_request_body(url: &str) -> Value {
    json!({"url": url, "formats": ["rawHtml", "html"], "onlyMainContent": false})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::vendor_http::test_support::{
        SECRET_BODY, assert_error, closed_port, serve,
    };
    use axum::extract::Json as AxumJson;
    use axum::http::HeaderMap;
    use axum::routing::post;
    use axum::{Router, http::StatusCode};
    use std::sync::{Arc, Mutex};

    const KEY: &str = "sk-test-firecrawl";

    fn scraper(base_url: String, timeout: Duration) -> FirecrawlScraper {
        FirecrawlScraper::with_base_url(KEY.to_string(), base_url, timeout)
    }

    /// One request the fake vendor received: its authorization header, and
    /// its JSON body.
    type SeenRequest = (Option<String>, Value);
    type Recorder = Arc<Mutex<Vec<SeenRequest>>>;

    /// A fake vendor that records the authorization header and body it got,
    /// and answers with `response`.
    fn fake(status: StatusCode, response: Value) -> (Router, Recorder) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = seen.clone();
        let router = Router::new().route(
            SCRAPE_PATH,
            post(
                move |headers: HeaderMap, AxumJson(request): AxumJson<Value>| {
                    let recorder = recorder.clone();
                    let status = status;
                    let answer = response.clone();
                    async move {
                        recorder.lock().unwrap().push((
                            headers
                                .get("authorization")
                                .and_then(|v| v.to_str().ok())
                                .map(str::to_string),
                            request,
                        ));
                        (status, AxumJson(answer))
                    }
                },
            ),
        );
        (router, seen)
    }

    fn page(html: &str) -> Value {
        json!({
            "success": true,
            "data": {
                "rawHtml": html,
                "metadata": {
                    "sourceURL": "https://medium.com/@writer/a-post",
                    "statusCode": 200,
                    "contentType": "text/html"
                }
            }
        })
    }

    #[tokio::test]
    async fn a_scrape_posts_the_page_url_and_returns_the_vendors_html() {
        let article = "<html><body><article><p>The full text</p></article></body></html>";
        let (router, seen) = fake(StatusCode::OK, page(article));
        let base = serve(router).await;
        let page = scraper(base, Duration::from_secs(5))
            .scrape("https://medium.com/@writer/a-post")
            .await
            .unwrap();
        assert_eq!(page.html, article);
        assert_eq!(page.html_bytes, article.len());
        assert_eq!(page.final_url, "https://medium.com/@writer/a-post");
        assert!(!page.locked);

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0.as_deref(), Some("Bearer sk-test-firecrawl"));
        assert_eq!(
            seen[0].1,
            expected_request_body("https://medium.com/@writer/a-post")
        );
    }

    /// The lock marker in the HTML is what a member-only Medium preview
    /// carries; it must never be stored as the article.
    #[tokio::test]
    async fn the_medium_lock_marker_is_reported_as_a_preview() {
        let html = r#"<html><script>window.__APOLLO_STATE__={"isLocked":true}</script><p>teaser</p></html>"#;
        let (router, _seen) = fake(StatusCode::OK, page(html));
        let base = serve(router).await;
        let page = scraper(base, Duration::from_secs(5))
            .scrape("https://medium.com/@writer/a-post")
            .await
            .unwrap();
        assert!(page.locked);
    }

    #[tokio::test]
    async fn a_locked_metadata_flag_is_a_preview_too() {
        let mut body = page("<html><p>teaser</p></html>");
        body["data"]["metadata"]["isLocked"] = json!(true);
        let (router, _seen) = fake(StatusCode::OK, body);
        let base = serve(router).await;
        let page = scraper(base, Duration::from_secs(5))
            .scrape("https://example.com/a")
            .await
            .unwrap();
        assert!(page.locked);
    }

    #[tokio::test]
    async fn the_cleaned_html_is_used_when_raw_html_is_absent() {
        let mut body = page("<html><p>cleaned</p></html>");
        body["data"]["rawHtml"] = Value::Null;
        body["data"]["html"] = json!("<html><p>cleaned</p></html>");
        let (router, _seen) = fake(StatusCode::OK, body);
        let base = serve(router).await;
        let page = scraper(base, Duration::from_secs(5))
            .scrape("https://example.com/a")
            .await
            .unwrap();
        assert_eq!(page.html, "<html><p>cleaned</p></html>");
        assert_eq!(page.html_bytes, "<html><p>cleaned</p></html>".len());
    }

    /// A 401 is the operator's key: retryable, like every other operator-key
    /// adapter, because the key is rotated out of band.
    #[tokio::test]
    async fn a_rejected_key_is_retryable_and_names_only_the_status() {
        let (router, _seen) = fake(
            StatusCode::UNAUTHORIZED,
            json!({"success": false, "error": SECRET_BODY}),
        );
        let base = serve(router).await;
        assert_error(
            scraper(base, Duration::from_secs(5))
                .scrape("https://example.com/a")
                .await,
            true,
            "401",
        );
    }

    #[tokio::test]
    async fn rate_limits_and_vendor_failures_are_retryable() {
        for code in [
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            let (router, _seen) = fake(code, json!({"success": false, "error": SECRET_BODY}));
            let base = serve(router).await;
            assert_error(
                scraper(base, Duration::from_secs(5))
                    .scrape("https://example.com/a")
                    .await,
                true,
                &code.as_u16().to_string(),
            );
        }
    }

    #[tokio::test]
    async fn a_bad_request_is_final() {
        let (router, _seen) = fake(
            StatusCode::BAD_REQUEST,
            json!({"success": false, "error": SECRET_BODY}),
        );
        let base = serve(router).await;
        assert_error(
            scraper(base, Duration::from_secs(5))
                .scrape("https://example.com/a")
                .await,
            false,
            "400",
        );
    }

    #[tokio::test]
    async fn a_timeout_is_retryable() {
        let router = Router::new().route(
            SCRAPE_PATH,
            post(|| async {
                tokio::time::sleep(Duration::from_secs(30)).await;
                AxumJson(json!({}))
            }),
        );
        let base = serve(router).await;
        let error = scraper(base, Duration::from_millis(200))
            .scrape("https://example.com/a")
            .await
            .unwrap_err();
        assert!(matches!(error, DomainError::Internal(_)), "{error:?}");
        assert!(!error.to_string().contains(KEY));
    }

    #[tokio::test]
    async fn a_refusal_a_malformed_answer_and_a_missing_page_are_reported() {
        let cases = [
            json!({"success": false}),
            json!({"success": true, "data": {"metadata": {}}}),
            json!({"success": true, "data": {"rawHtml": "  "}}),
            json!({"nonsense": []}),
        ];
        for body in cases {
            let (router, _seen) = fake(StatusCode::OK, body);
            let base = serve(router).await;
            let error = scraper(base, Duration::from_secs(5))
                .scrape("https://example.com/a")
                .await
                .unwrap_err();
            assert!(
                matches!(
                    error,
                    DomainError::InvalidInput(_) | DomainError::Internal(_)
                ),
                "{error:?}"
            );
            assert!(!error.to_string().contains(SECRET_BODY));
        }
    }

    /// The vendor succeeded, but the page itself answered 404: an error page,
    /// not an article, and no retry can change it.
    #[tokio::test]
    async fn a_page_error_status_is_final() {
        let mut body = page("<html><h1>Not Found</h1></html>");
        body["data"]["metadata"]["statusCode"] = json!(404);
        let (router, _seen) = fake(StatusCode::OK, body);
        let base = serve(router).await;
        assert_error(
            scraper(base, Duration::from_secs(5))
                .scrape("https://example.com/gone")
                .await,
            false,
            "404",
        );
    }

    #[tokio::test]
    async fn a_closed_port_is_retryable() {
        let base = closed_port().await;
        assert_error(
            scraper(base, Duration::from_millis(500))
                .scrape("https://example.com/a")
                .await,
            true,
            "Firecrawl",
        );
    }
}
