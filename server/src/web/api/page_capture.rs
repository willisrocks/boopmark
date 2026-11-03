//! `PUT /api/v1/bookmarks/{id}/page`: the page HTML a browser rendered,
//! gzipped, for a bookmark the server could not fetch itself (spec:
//! docs/superpowers/specs/2026-11-03-page-capture-design.md).
//!
//! The body is the raw gzip stream, never multipart or JSON, so it is read
//! and inflated with a cap on both sides: a gzip bomb costs milliseconds of
//! CPU, not memory. No network request is made on this path, and
//! subresources of the captured HTML are never fetched.

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::put;
use serde::Serialize;
use tokio_stream::StreamExt;
use uuid::Uuid;

use crate::adapters::net::mime_type;
use crate::app::page_capture::{GzipError, gunzip_capped};
use crate::domain::bookmark::Bookmark;
use crate::domain::error::DomainError;
use crate::domain::ports::article_fetcher::ArticleFetchError;
use crate::domain::ports::page_capture_repo::NewCapture;
use crate::domain::url_match::{parse_web_url, same_page};
use crate::web::extractors::AuthUser;
use crate::web::state::{AppState, Bookmarks};

const PAGE_URL_HEADER: &str = "x-page-url";

#[derive(Serialize)]
struct CaptureAccepted {
    /// `indexed`: the text is stored, the lexical leg finds it now.
    /// `queued`: the payload is stored, the ingest worker parses it.
    status: &'static str,
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
}

/// Every way this route refuses a capture, with the status and
/// `{"error": ...}` body the client gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureError {
    /// No such bookmark for this user, and also the answer when the feature
    /// is switched off: a write surface a self-hoster does not want does not
    /// advertise itself.
    NotFound,
    InvalidPageUrl,
    UnsupportedContentType,
    UnsupportedEncoding,
    BadGzip,
    UrlMismatch,
    TooLarge,
    NoArticle,
    /// The capture queue is full; the client may retry later, never
    /// automatically.
    Busy,
    Internal,
}

impl CaptureError {
    fn status(self) -> StatusCode {
        match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::InvalidPageUrl
            | Self::UnsupportedContentType
            | Self::UnsupportedEncoding
            | Self::BadGzip => StatusCode::BAD_REQUEST,
            Self::UrlMismatch => StatusCode::CONFLICT,
            Self::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::NoArticle => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Busy => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn error(self) -> &'static str {
        match self {
            Self::NotFound => "not found",
            Self::InvalidPageUrl => "invalid_page_url",
            Self::UnsupportedContentType => "unsupported_content_type",
            Self::UnsupportedEncoding => "unsupported_encoding",
            Self::BadGzip => "bad_gzip",
            Self::UrlMismatch => "url_mismatch",
            Self::TooLarge => "too large",
            Self::NoArticle => "no readable article",
            Self::Busy => "busy",
            Self::Internal => "internal error",
        }
    }
}

impl IntoResponse for CaptureError {
    fn into_response(self) -> Response {
        (
            self.status(),
            Json(ErrorBody {
                error: self.error(),
            }),
        )
            .into_response()
    }
}

impl From<DomainError> for CaptureError {
    fn from(err: DomainError) -> Self {
        match err {
            DomainError::NotFound => Self::NotFound,
            _ => Self::Internal,
        }
    }
}

/// `X-Page-Url`: the URL the browser read from the tab. Absolute `http(s)`
/// only; anything else is `400 invalid_page_url`.
fn page_url(headers: &HeaderMap) -> Result<url::Url, CaptureError> {
    let raw = headers
        .get(PAGE_URL_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or(CaptureError::InvalidPageUrl)?;
    parse_web_url(raw).ok_or(CaptureError::InvalidPageUrl)
}

/// The body must declare an HTML document; the charset is irrelevant (the
/// extension sends the UTF-8 bytes of the DOM).
fn html_content_type(headers: &HeaderMap) -> Result<String, CaptureError> {
    let raw = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .ok_or(CaptureError::UnsupportedContentType)?;
    let mime = mime_type(raw);
    if mime != "text/html" && mime != "application/xhtml+xml" {
        return Err(CaptureError::UnsupportedContentType);
    }
    Ok(raw.to_string())
}

/// The extension always compresses; an identity body is a client bug, not a
/// page.
fn require_gzip(headers: &HeaderMap) -> Result<(), CaptureError> {
    let encoding = headers
        .get(header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_ascii_lowercase());
    match encoding.as_deref() {
        Some("gzip") => Ok(()),
        _ => Err(CaptureError::UnsupportedEncoding),
    }
}

/// `Content-Length`, when the client sent one, decides before a byte is read.
fn check_content_length(headers: &HeaderMap, cap: usize) -> Result<(), CaptureError> {
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok());
    match declared {
        Some(len) if len > cap as u64 => Err(CaptureError::TooLarge),
        _ => Ok(()),
    }
}

/// Read the compressed body, stopping as soon as it passes `cap`.
async fn read_capped(body: Body, cap: usize) -> Result<Vec<u8>, CaptureError> {
    let mut stream = body.into_data_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        // A body that stops mid-stream is the client's truncated gzip.
        let chunk = chunk.map_err(|_| CaptureError::BadGzip)?;
        if body.len() + chunk.len() > cap {
            return Err(CaptureError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// The bookmark this user owns: another user's id is indistinguishable from
/// a missing one, which does not disclose their bookmark ids.
async fn load_bookmark(
    state: &AppState,
    id: Uuid,
    user_id: Uuid,
) -> Result<Bookmark, CaptureError> {
    let loaded = match &state.bookmarks {
        Bookmarks::Local(service) => service.get(id, user_id).await,
        Bookmarks::S3(service) => service.get(id, user_id).await,
    };
    loaded.map_err(CaptureError::from)
}

/// The capture flow, in the order the spec fixes: cheapest and least
/// disclosing first, so a mismatched URL with a huge body costs nothing.
async fn capture(
    state: &AppState,
    user_id: Uuid,
    id: Uuid,
    headers: &HeaderMap,
    body: Body,
) -> Result<&'static str, CaptureError> {
    if !state.config.page_capture_enabled {
        return Err(CaptureError::NotFound);
    }
    let bookmark = load_bookmark(state, id, user_id).await?;
    let page = page_url(headers)?;
    if !same_page(&bookmark.url, page.as_str()) {
        return Err(CaptureError::UrlMismatch);
    }
    let content_type = html_content_type(headers)?;
    require_gzip(headers)?;
    let cap = state.config.page_capture_max_bytes;
    check_content_length(headers, cap)?;

    let gz = read_capped(body, cap).await?;
    let html = gunzip_capped(&gz, cap).map_err(|e| match e {
        GzipError::TooLarge => CaptureError::TooLarge,
        GzipError::Corrupt => CaptureError::BadGzip,
    })?;

    match state
        .capture_extractor
        .extract(html, Some(&content_type), page.as_str())
        .await
    {
        Ok(text) => {
            // `store_capture` clears any claim in flight and drops the old
            // text; the embed worker re-chunks when the text changed.
            if !state.page_capture.store_capture(id, user_id, &text).await? {
                return Err(CaptureError::NotFound);
            }
            state.embed_wake.notify_one();
            Ok("indexed")
        }
        // The parse did not fit the request's budget: keep the payload and
        // let the ingest worker parse it.
        Err(ArticleFetchError::Transient(_)) => {
            let capture = NewCapture {
                bookmark_id: id,
                user_id,
                page_url: page.to_string(),
                html_gz: gz,
            };
            if state.page_capture.queue_capture(&capture).await? {
                Ok("queued")
            } else {
                Err(CaptureError::Busy)
            }
        }
        // Readability found no article, or the document is unreadable.
        Err(ArticleFetchError::Permanent(_)) | Err(ArticleFetchError::Skipped(_)) => {
            Err(CaptureError::NoArticle)
        }
    }
}

async fn capture_page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    match capture(&state, user.id, id, &headers, body).await {
        Ok(status) => (StatusCode::ACCEPTED, Json(CaptureAccepted { status })).into_response(),
        Err(err) => err.into_response(),
    }
}

pub fn routes() -> axum::Router<AppState> {
    axum::Router::new().route("/{id}/page", put(capture_page))
}
