//! Firecrawl adapter for the `PageScraper` port: one hosted `/scrape` call per
//! page and no crawl. The vendor renders the page from its own network and
//! answers with the HTML, which the app then extracts with its own Readability
//! pass.
//!
//! One format is requested (`rawHtml`): the cleaned `html` would double the
//! answer for text nobody reads, and the raw document is what carries the
//! page's own state. The response is read with a cap, and the page itself is
//! checked against the same ceiling as the direct fetch before it is parsed.
//!
//! A 429 is the vendor's account talking, not this page: it is reported as
//! `ScrapeError::RateLimited` with the wait the vendor named, and the fallback
//! decorator pauses every scrape for it and decides whether one more request
//! fits the job. The decorator also paces the calls themselves by
//! `PAGE_SCRAPER_CONCURRENCY` and `PAGE_SCRAPER_RATE_PER_MINUTE`, because the
//! plan limits both the renders in flight and the requests a minute.

use crate::adapters::article::readability::MAX_HTML_BYTES;
use crate::adapters::net::{CappedReadError, read_capped};
use crate::adapters::vendor_http::{KeyOwner, http_status_error, transport_error};
use crate::domain::error::DomainError;
use crate::domain::ports::page_scraper::{BoxFuture, PageScraper, ScrapeError, ScrapedPage};
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::time::Duration;

const VENDOR: &str = "Firecrawl";
const SCRAPE_PATH: &str = "/v2/scrape";
const DEFAULT_BASE_URL: &str = "https://api.firecrawl.dev";
/// The JSON envelope carries the HTML as an escaped string, so the body can be
/// larger than the page. Read with headroom, then check the page itself.
const MAX_RESPONSE_BYTES: usize = MAX_HTML_BYTES + 2 * 1024 * 1024;

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

    /// One scrape: the API's own status decides whether a page was billed,
    /// then the payload's `success`, the target page's status and the HTML
    /// itself decide what came back.
    ///
    /// A 429 is the account's rate limit, not this page's verdict: it comes
    /// back with the wait the vendor named, so the caller can pause every
    /// scrape for it and decide whether one more request fits its job.
    async fn scrape_once(&self, url: &str) -> Result<ScrapedPage, ScrapeError> {
        let body = json!({
            "url": url,
            "formats": ["rawHtml"],
            // The article's own body is what Readability needs; the vendor's
            // main-content guess would drop parts of it.
            "onlyMainContent": false,
        });
        let request = self
            .client
            .post(format!("{}{SCRAPE_PATH}", self.base_url))
            .bearer_auth(&self.api_key);
        let response = match request.json(&body).send().await {
            Ok(response) => response,
            // Nothing came back: no 2xx, so no page was billed.
            Err(error) => {
                return Err(match transport_error(VENDOR, &error) {
                    DomainError::InvalidInput(message) => ScrapeError::Final {
                        message,
                        billed: false,
                    },
                    DomainError::Internal(message) => ScrapeError::Retryable {
                        message,
                        billed: false,
                    },
                    other => ScrapeError::Final {
                        message: other.to_string(),
                        billed: false,
                    },
                });
            }
        };

        let status = response.status();
        if status == StatusCode::PAYMENT_REQUIRED {
            return Err(ScrapeError::OutOfCredits);
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            let asked = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok());
            return Err(ScrapeError::RateLimited {
                retry_after: retry_after_delay(asked),
            });
        }
        if !status.is_success() {
            return Err(
                match http_status_error(VENDOR, status, KeyOwner::Operator) {
                    DomainError::Internal(message) => ScrapeError::Retryable {
                        message,
                        billed: false,
                    },
                    other => ScrapeError::Final {
                        message: other.to_string(),
                        billed: false,
                    },
                },
            );
        }

        // From here on the request was accepted, so the page is paid for even
        // when this adapter cannot use it.
        let bytes = match read_capped(response, MAX_RESPONSE_BYTES).await {
            Ok(bytes) => bytes,
            Err(CappedReadError::TooBig) => {
                return Err(ScrapeError::Final {
                    message: format!("{VENDOR} response exceeded {MAX_RESPONSE_BYTES} bytes"),
                    billed: true,
                });
            }
            Err(CappedReadError::Read(_)) => {
                return Err(ScrapeError::Retryable {
                    message: format!("{VENDOR} response read failed"),
                    billed: true,
                });
            }
        };
        match serde_json::from_slice(&bytes) {
            Ok(payload) => Self::interpret(payload, url),
            Err(_) => Err(ScrapeError::Final {
                message: format!("{VENDOR} returned a malformed response"),
                billed: true,
            }),
        }
    }

    /// The page from a 2xx payload. Every failure here is `billed: true`.
    fn interpret(payload: Value, requested_url: &str) -> Result<ScrapedPage, ScrapeError> {
        let billed = true;
        let parsed: ScrapeResponse =
            serde_json::from_value(payload).map_err(|_| ScrapeError::Final {
                message: format!("{VENDOR} returned a malformed response"),
                billed,
            })?;
        if parsed.success == Some(false) {
            return Err(ScrapeError::Final {
                message: format!("{VENDOR} refused the scrape"),
                billed,
            });
        }
        let data = parsed.data.ok_or_else(|| ScrapeError::Final {
            message: format!("{VENDOR} returned no page"),
            billed,
        })?;
        let metadata = data.metadata;
        if let Some(status) = metadata.as_ref().and_then(|m| m.status_code)
            && !(200..300).contains(&status)
            && status != 304
        {
            return Err(ScrapeError::Final {
                message: format!("{VENDOR} page returned HTTP {status}"),
                billed,
            });
        }
        let html = data
            .raw_html
            .filter(|html| !html.trim().is_empty())
            .ok_or_else(|| ScrapeError::Final {
                message: format!("{VENDOR} returned no HTML for the page"),
                billed,
            })?;
        if html.len() > MAX_HTML_BYTES {
            return Err(ScrapeError::Final {
                message: format!("{VENDOR} page exceeded {MAX_HTML_BYTES} bytes"),
                billed,
            });
        }
        let locked = metadata.as_ref().and_then(|m| m.is_locked) == Some(true);
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
    fn scrape(&self, url: &str) -> BoxFuture<'_, Result<ScrapedPage, ScrapeError>> {
        let url = url.to_string();
        Box::pin(async move { self.scrape_once(&url).await })
    }
}

/// The wait a 429 asked for, when the vendor named one in seconds. An
/// `Retry-After` the app cannot read (an HTTP date, a malformed value) and a
/// missing one both come back as `None`, and the caller applies its own
/// default: the ask is the account's, so it must not be guessed here.
fn retry_after_delay(asked: Option<&str>) -> Option<Duration> {
    asked
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

/// The request body, so a test asserts it without restating it.
#[cfg(test)]
pub(crate) fn expected_request_body(url: &str) -> Value {
    json!({"url": url, "formats": ["rawHtml"], "onlyMainContent": false})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::vendor_http::test_support::{SECRET_BODY, closed_port, serve};
    use axum::extract::Json as AxumJson;
    use axum::http::HeaderMap;
    use axum::response::IntoResponse;
    use axum::routing::post;
    use axum::{Router, http::StatusCode};
    use std::sync::{Arc, Mutex};

    const KEY: &str = "sk-test-firecrawl";

    /// One request the fake vendor received: its authorization header, and
    /// its JSON body.
    type SeenRequest = (Option<String>, Value);
    type Recorder = Arc<Mutex<Vec<SeenRequest>>>;

    fn scraper(base_url: String, timeout: Duration) -> FirecrawlScraper {
        FirecrawlScraper::with_base_url(KEY.to_string(), base_url, timeout)
    }

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

    /// One scripted answer of the fake vendor: its status, the `Retry-After`
    /// it carries, and its body.
    #[derive(Clone)]
    struct Scripted {
        status: StatusCode,
        retry_after: Option<&'static str>,
        body: Value,
    }

    /// A fake vendor that answers each request in turn; the last answer
    /// repeats, so a test can count the requests it took.
    fn scripted(answers: Vec<Scripted>) -> (Router, Recorder) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = seen.clone();
        let queue = Arc::new(Mutex::new(std::collections::VecDeque::from(answers)));
        let router = Router::new().route(
            SCRAPE_PATH,
            post(
                move |headers: HeaderMap, AxumJson(request): AxumJson<Value>| {
                    let recorder = recorder.clone();
                    let queue = queue.clone();
                    async move {
                        recorder.lock().unwrap().push((
                            headers
                                .get("authorization")
                                .and_then(|v| v.to_str().ok())
                                .map(str::to_string),
                            request,
                        ));
                        let answer = {
                            let mut queue = queue.lock().unwrap();
                            if queue.len() > 1 {
                                queue.pop_front().unwrap()
                            } else {
                                queue.front().cloned().expect("a scripted answer")
                            }
                        };
                        let mut response = (answer.status, AxumJson(answer.body)).into_response();
                        if let Some(value) = answer.retry_after {
                            response.headers_mut().insert(
                                axum::http::header::RETRY_AFTER,
                                axum::http::HeaderValue::from_str(value).unwrap(),
                            );
                        }
                        response
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

    fn assert_retryable(result: Result<ScrapedPage, ScrapeError>, needle: &str, billed: bool) {
        match result {
            Err(ScrapeError::Retryable {
                message,
                billed: got,
            }) => {
                assert_eq!(got, billed, "billed");
                assert!(message.contains(needle), "{message}");
                assert!(!message.contains(SECRET_BODY));
                assert!(!message.contains("sk-test"));
            }
            other => panic!("expected a retryable error, got {other:?}"),
        }
    }

    fn assert_final(result: Result<ScrapedPage, ScrapeError>, needle: &str, billed: bool) {
        match result {
            Err(ScrapeError::Final {
                message,
                billed: got,
            }) => {
                assert_eq!(got, billed, "billed");
                assert!(message.contains(needle), "{message}");
                assert!(!message.contains(SECRET_BODY));
                assert!(!message.contains("sk-test"));
            }
            other => panic!("expected a final error, got {other:?}"),
        }
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

    #[tokio::test]
    async fn the_vendors_lock_flag_is_reported_as_a_preview() {
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

    /// A 401 is the operator's key: retryable, like every other operator-key
    /// adapter, because the key is rotated out of band.
    #[tokio::test]
    async fn a_rejected_key_is_retryable_and_names_only_the_status() {
        let (router, _seen) = fake(
            StatusCode::UNAUTHORIZED,
            json!({"success": false, "error": SECRET_BODY}),
        );
        let base = serve(router).await;
        assert_retryable(
            scraper(base, Duration::from_secs(5))
                .scrape("https://example.com/a")
                .await,
            "401",
            false,
        );
    }

    #[tokio::test]
    async fn vendor_failures_are_retryable() {
        for code in [
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            let (router, _seen) = fake(code, json!({"success": false, "error": SECRET_BODY}));
            let base = serve(router).await;
            assert_retryable(
                scraper(base, Duration::from_secs(5))
                    .scrape("https://example.com/a")
                    .await,
                &code.as_u16().to_string(),
                false,
            );
        }
    }

    /// A 429 is the account's limit, not this page's verdict: the error
    /// carries the wait the vendor named, and nothing was billed.
    #[tokio::test]
    async fn a_rate_limit_reports_the_vendors_ask() {
        let (router, seen) = scripted(vec![Scripted {
            status: StatusCode::TOO_MANY_REQUESTS,
            retry_after: Some("30"),
            body: json!({"success": false, "error": SECRET_BODY}),
        }]);
        let base = serve(router).await;
        let result = scraper(base, Duration::from_secs(5))
            .scrape("https://medium.com/@writer/a-post")
            .await;
        assert_eq!(
            result,
            Err(ScrapeError::RateLimited {
                retry_after: Some(Duration::from_secs(30)),
            })
        );
        assert_eq!(
            seen.lock().unwrap().len(),
            1,
            "the adapter sends one request; the job decides whether to retry"
        );
    }

    /// A 429 without a wait the app can read is still the account's limit.
    #[tokio::test]
    async fn a_rate_limit_without_a_retry_after_names_no_wait() {
        for retry_after in [None, Some("Wed, 21 Oct 2015 07:28:00 GMT")] {
            let (router, _seen) = scripted(vec![Scripted {
                status: StatusCode::TOO_MANY_REQUESTS,
                retry_after,
                body: json!({"success": false, "error": SECRET_BODY}),
            }]);
            let base = serve(router).await;
            let result = scraper(base, Duration::from_secs(5))
                .scrape("https://example.com/a")
                .await;
            match result {
                Err(ScrapeError::RateLimited { retry_after }) => {
                    assert_eq!(retry_after, None, "{retry_after:?}");
                }
                other => panic!("expected a rate-limited error, got {other:?}"),
            }
        }
    }

    #[test]
    fn retry_after_seconds_are_read_and_nothing_else_is_guessed() {
        assert_eq!(retry_after_delay(Some("3")), Some(Duration::from_secs(3)));
        assert_eq!(retry_after_delay(Some(" 0 ")), Some(Duration::ZERO));
        assert_eq!(retry_after_delay(None), None);
        // HTTP dates are not parsed: the caller applies its own default.
        assert_eq!(
            retry_after_delay(Some("Wed, 21 Oct 2015 07:28:00 GMT")),
            None
        );
        assert_eq!(retry_after_delay(Some("soon")), None);
    }

    /// Out of credits is not a per-page failure: no scrape can work until the
    /// account is topped up, so the fallback has to switch itself off.
    #[tokio::test]
    async fn out_of_credits_is_reported_as_such() {
        let (router, _seen) = fake(
            StatusCode::PAYMENT_REQUIRED,
            json!({"success": false, "error": SECRET_BODY}),
        );
        let base = serve(router).await;
        let error = scraper(base, Duration::from_secs(5))
            .scrape("https://example.com/a")
            .await
            .unwrap_err();
        assert_eq!(error, ScrapeError::OutOfCredits);
        assert!(!error.billed());
    }

    #[tokio::test]
    async fn a_bad_request_is_final() {
        let (router, _seen) = fake(
            StatusCode::BAD_REQUEST,
            json!({"success": false, "error": SECRET_BODY}),
        );
        let base = serve(router).await;
        assert_final(
            scraper(base, Duration::from_secs(5))
                .scrape("https://example.com/a")
                .await,
            "400",
            false,
        );
    }

    /// A timeout got no answer at all, so no page was billed.
    #[tokio::test]
    async fn a_timeout_is_retryable_and_unbilled() {
        let router = Router::new().route(
            SCRAPE_PATH,
            post(|| async {
                tokio::time::sleep(Duration::from_secs(30)).await;
                AxumJson(json!({}))
            }),
        );
        let base = serve(router).await;
        let result = scraper(base, Duration::from_millis(200))
            .scrape("https://example.com/a")
            .await;
        assert_retryable(result, "timed out", false);
    }

    #[tokio::test]
    async fn a_refusal_or_a_message_without_html_is_final_but_billed() {
        for body in [
            json!({"success": false}),
            json!({"success": true, "data": {"metadata": {}}}),
            json!({"success": true, "data": {"rawHtml": "  "}}),
            json!({"nonsense": []}),
        ] {
            let (router, _seen) = fake(StatusCode::OK, body);
            let base = serve(router).await;
            let result = scraper(base, Duration::from_secs(5))
                .scrape("https://example.com/a")
                .await;
            match result {
                Err(ScrapeError::Final { billed, .. }) => assert!(billed, "a 2xx was paid for"),
                other => panic!("expected a final error, got {other:?}"),
            }
        }
    }

    /// The vendor succeeded, but the page itself answered 404: an error page,
    /// not an article, and no retry can change it.
    #[tokio::test]
    async fn a_page_error_status_is_final_and_billed() {
        let mut body = page("<html><h1>Not Found</h1></html>");
        body["data"]["metadata"]["statusCode"] = json!(404);
        let (router, _seen) = fake(StatusCode::OK, body);
        let base = serve(router).await;
        assert_final(
            scraper(base, Duration::from_secs(5))
                .scrape("https://example.com/gone")
                .await,
            "404",
            true,
        );
    }

    /// A page over the shared ceiling is refused before anyone parses it.
    #[tokio::test]
    async fn an_oversized_page_is_final_and_billed() {
        let huge = "x".repeat(MAX_HTML_BYTES + 1);
        let (router, _seen) = fake(StatusCode::OK, page(&huge));
        let base = serve(router).await;
        assert_final(
            scraper(base, Duration::from_secs(30))
                .scrape("https://example.com/huge")
                .await,
            "exceeded",
            true,
        );
    }

    #[tokio::test]
    async fn a_closed_port_is_retryable_and_unbilled() {
        let base = closed_port().await;
        assert_retryable(
            scraper(base, Duration::from_millis(500))
                .scrape("https://example.com/a")
                .await,
            "Firecrawl",
            false,
        );
    }
}
