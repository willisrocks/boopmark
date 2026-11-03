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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::article::readability::HtmlExtractor;
    use crate::adapters::embedder::DisabledEmbedder;
    use crate::adapters::image_fetch::GuardedImageFetcher;
    use crate::adapters::llm_client::{LlmClientConfig, LlmClientRouter};
    use crate::adapters::login::local_password::LocalPasswordLoginProvider;
    use crate::adapters::metadata::fallback::FallbackMetadataExtractor;
    use crate::adapters::metadata::html::HtmlMetadataExtractor;
    use crate::adapters::postgres::PostgresPool;
    use crate::adapters::postgres::search_repo::search_tests::{SERIAL, bookmark, pool, user};
    use crate::adapters::screenshot::noop::NoopScreenshot;
    use crate::adapters::storage::local::LocalStorage;
    use crate::app::auth::AuthService;
    use crate::app::bookmarks::BookmarkService;
    use crate::app::enrichment::EnrichmentService;
    use crate::app::invite::InviteService;
    use crate::app::page_capture::{CaptureService, CaptureWorker};
    use crate::app::search::SearchService;
    use crate::app::secrets::SecretBox;
    use crate::app::settings::SettingsService;
    use crate::app::tag_alias::TagAliasService;
    use crate::app::tag_consolidation::TagConsolidationService;
    use crate::app::tag_llm::LlmClientEnricher;
    use crate::config::{
        Config, LoginAdapter, MetadataFallbackBackend, ScreenshotBackend, StorageBackend,
    };
    use crate::domain::ports::embedder::Embedder;
    use crate::domain::ports::llm_client::LlmClient;
    use crate::domain::ports::tag_resolver::TagResolver;
    use crate::domain::ports::tag_vocabulary::TagVocabulary;
    use crate::web::state::{AppState, Bookmarks, ImageStorage};
    use std::io::Write;
    use std::sync::Arc;
    use tokio::net::TcpListener;

    /// 32 bytes of base64, as `LLM_SETTINGS_ENCRYPTION_KEY` requires.
    const KEY: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";
    const BOOKMARK_URL: &str = "https://medium.com/@writer/a-post";
    const ARTICLE: &str = r#"<!doctype html><html><head><title>Borrowing in Rust</title></head>
        <body><article><h1>Borrowing in Rust</h1>
        <p>The borrow checker enforces that every reference is valid for as long as it is used,
           and that mutable references are never aliased, which a readability heuristic can see.</p>
        <p>Lifetimes are the names the compiler gives to those regions of validity.</p>
        </article></body></html>"#;

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn config(page_capture_enabled: bool, page_capture_max_bytes: usize) -> Config {
        Config {
            database_url: String::new(),
            app_url: "http://127.0.0.1".into(),
            port: 0,
            llm_settings_encryption_key: KEY.into(),
            session_secret: "capture-test".into(),
            google_client_id: None,
            google_client_secret: None,
            enable_e2e_auth: false,
            login_adapter: LoginAdapter::LocalPassword,
            storage_backend: StorageBackend::Local,
            s3_endpoint: None,
            s3_access_key: None,
            s3_secret_key: None,
            s3_region: "auto".into(),
            s3_images_bucket: "boopmark-images".into(),
            s3_images_public_url: None,
            screenshot_backend: ScreenshotBackend::Disabled,
            screenshot_service_url: None,
            metadata_fallback_backend: MetadataFallbackBackend::None,
            iframely_api_key: None,
            opengraph_io_api_key: None,
            article_ingest_enabled: false,
            article_ingest_concurrency: 2,
            page_capture_enabled,
            page_capture_max_bytes,
        }
    }

    /// The whole app state the route needs, wired as `main.rs` wires it. The
    /// capture extractor holds its own slots, and no worker of any kind runs.
    fn state(db: Arc<PostgresPool>, config: Config) -> AppState {
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            Arc::new(SecretBox::new(&config.llm_settings_encryption_key)),
        ));
        let tag_aliases = Arc::new(TagAliasService::new(db.clone()));
        let tag_resolver: Arc<dyn TagResolver> = tag_aliases.clone();
        let tag_vocabulary: Arc<dyn TagVocabulary> = tag_aliases.clone();
        let metadata = Arc::new(FallbackMetadataExtractor::new(vec![Box::new(
            HtmlMetadataExtractor::new(),
        )]));
        let llm_client: Arc<dyn LlmClient> =
            Arc::new(LlmClientRouter::from_config(&LlmClientConfig::from_env()));
        let embedder: Arc<dyn Embedder> = Arc::new(DisabledEmbedder);
        let storage = Arc::new(LocalStorage::new(
            "./target/test-uploads".into(),
            "http://127.0.0.1/uploads".into(),
        ));
        let bookmarks = Bookmarks::Local(Arc::new(
            BookmarkService::new(
                db.clone(),
                metadata.clone(),
                storage,
                Arc::new(NoopScreenshot),
                Arc::new(GuardedImageFetcher::new()),
            )
            .with_tag_resolver(tag_resolver),
        ));
        AppState {
            bookmarks,
            auth: Arc::new(AuthService::new(db.clone(), db.clone(), db.clone())),
            settings: settings.clone(),
            config: Arc::new(config),
            enrichment: Arc::new(EnrichmentService::new(
                metadata,
                Arc::new(LlmClientEnricher::new(llm_client.clone())),
                settings.clone(),
                tag_vocabulary,
            )),
            tag_aliases: tag_aliases.clone(),
            tag_consolidation: Arc::new(TagConsolidationService::new(
                tag_aliases,
                db.clone(),
                settings,
                llm_client.clone(),
            )),
            images_storage: ImageStorage::Local(LocalStorage::new(
                "./target/test-uploads/images".into(),
                "http://127.0.0.1/uploads/images".into(),
            )),
            image_processing_slots: Arc::new(tokio::sync::Semaphore::new(1)),
            active_image_fix_jobs: Arc::new(
                std::sync::Mutex::new(std::collections::HashSet::new()),
            ),
            login_provider: Arc::new(LocalPasswordLoginProvider),
            invites: Arc::new(InviteService::new(db.clone())),
            llm_client,
            embedder: embedder.clone(),
            search: Arc::new(SearchService::new(db.clone(), embedder)),
            page_capture: Arc::new(CaptureService::new(db)),
            capture_extractor: Arc::new(HtmlExtractor::new(2)),
            embed_wake: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// Serve the real router on a loopback port and return its base URL.
    async fn serve(state: AppState) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = crate::web::router::create_router(state);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        base
    }

    /// A `PUT` with exactly the headers given; `None` leaves one out.
    async fn put(
        base: &str,
        id: Uuid,
        key: Option<&str>,
        page_url: Option<&str>,
        content_type: Option<&str>,
        encoding: Option<&str>,
        body: Vec<u8>,
    ) -> (reqwest::StatusCode, String) {
        let mut request = reqwest::Client::new()
            .put(format!("{base}/api/v1/bookmarks/{id}/page"))
            .body(body);
        if let Some(key) = key {
            request = request.bearer_auth(key);
        }
        if let Some(value) = page_url {
            request = request.header("x-page-url", value);
        }
        if let Some(value) = content_type {
            request = request.header("content-type", value);
        }
        if let Some(value) = encoding {
            request = request.header("content-encoding", value);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        (status, response.text().await.unwrap())
    }

    /// The headers the extension sends, on a URL that matches the bookmark
    /// after normalization.
    async fn put_good(
        base: &str,
        id: Uuid,
        key: &str,
        html: &[u8],
    ) -> (reqwest::StatusCode, String) {
        put(
            base,
            id,
            Some(key),
            Some("https://medium.com/@writer/a-post/?utm_source=chrome#top"),
            Some("text/html; charset=utf-8"),
            Some("gzip"),
            gzip(html),
        )
        .await
    }

    async fn contents(pool: &sqlx::PgPool, id: Uuid) -> (String, Option<String>, Option<String>) {
        sqlx::query_as("SELECT status, source, text FROM bookmark_contents WHERE bookmark_id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[test]
    fn every_refusal_has_its_own_status_and_error_body() {
        assert_eq!(CaptureError::NotFound.status(), StatusCode::NOT_FOUND);
        assert_eq!(CaptureError::NotFound.error(), "not found");
        assert_eq!(CaptureError::UrlMismatch.status(), StatusCode::CONFLICT);
        assert_eq!(CaptureError::UrlMismatch.error(), "url_mismatch");
        assert_eq!(
            CaptureError::TooLarge.status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(CaptureError::TooLarge.error(), "too large");
        assert_eq!(
            CaptureError::NoArticle.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(CaptureError::NoArticle.error(), "no readable article");
        assert_eq!(CaptureError::Busy.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(CaptureError::Busy.error(), "busy");
        for err in [
            CaptureError::InvalidPageUrl,
            CaptureError::UnsupportedContentType,
            CaptureError::UnsupportedEncoding,
            CaptureError::BadGzip,
        ] {
            assert_eq!(err.status(), StatusCode::BAD_REQUEST);
        }
    }

    #[test]
    fn headers_are_checked_before_any_body_is_read() {
        let mut headers = HeaderMap::new();
        assert_eq!(page_url(&headers), Err(CaptureError::InvalidPageUrl));
        headers.insert("x-page-url", "https://example.com/a".parse().unwrap());
        assert_eq!(
            page_url(&headers).unwrap().as_str(),
            "https://example.com/a"
        );
        headers.insert("x-page-url", "not a url".parse().unwrap());
        assert_eq!(page_url(&headers), Err(CaptureError::InvalidPageUrl));

        let mut headers = HeaderMap::new();
        assert_eq!(
            html_content_type(&headers),
            Err(CaptureError::UnsupportedContentType)
        );
        for good in [
            "text/html",
            "Text/HTML; charset=UTF-8",
            "application/xhtml+xml",
        ] {
            headers.insert(header::CONTENT_TYPE, good.parse().unwrap());
            assert_eq!(html_content_type(&headers).unwrap(), good);
        }
        headers.insert(header::CONTENT_TYPE, "application/pdf".parse().unwrap());
        assert_eq!(
            html_content_type(&headers),
            Err(CaptureError::UnsupportedContentType)
        );

        let mut headers = HeaderMap::new();
        assert_eq!(
            require_gzip(&headers),
            Err(CaptureError::UnsupportedEncoding)
        );
        headers.insert(header::CONTENT_ENCODING, "GZIP".parse().unwrap());
        assert_eq!(require_gzip(&headers), Ok(()));
        headers.insert(header::CONTENT_ENCODING, "identity".parse().unwrap());
        assert_eq!(
            require_gzip(&headers),
            Err(CaptureError::UnsupportedEncoding)
        );

        let mut headers = HeaderMap::new();
        assert_eq!(check_content_length(&headers, 10), Ok(()));
        headers.insert(header::CONTENT_LENGTH, "11".parse().unwrap());
        assert_eq!(
            check_content_length(&headers, 10),
            Err(CaptureError::TooLarge)
        );
        headers.insert(header::CONTENT_LENGTH, "10".parse().unwrap());
        assert_eq!(check_content_length(&headers, 10), Ok(()));
    }

    /// Every answer the interface fixes, against the real router and a real
    /// database, in the check order the spec sets.
    #[tokio::test]
    #[ignore = "requires a migrated Postgres database (DATABASE_URL)"]
    async fn the_capture_route_answers_every_status_in_order() {
        let _serial = SERIAL.lock().await;
        let pool = pool().await;
        let db = Arc::new(PostgresPool::new(pool.clone()));
        let owner = user(&pool).await;
        let other = user(&pool).await;
        let page = bookmark(&db, owner, BOOKMARK_URL, "A post", None, &[]).await;
        let theirs = bookmark(
            &db,
            other,
            "https://medium.com/@writer/theirs",
            "Theirs",
            None,
            &[],
        )
        .await;
        let key = AuthService::new(db.clone(), db.clone(), db.clone())
            .create_api_key(owner, "capture test")
            .await
            .unwrap();

        let state = state(db.clone(), config(true, 5 * 1024 * 1024));
        let wake = state.embed_wake.clone();
        let base = serve(state).await;

        // 401: no credentials, from the existing extractor.
        let (status, body) = put_good(&base, page.id, "", ARTICLE.as_bytes()).await;
        assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED, "{body}");

        // 202 indexed: the text is stored as captured, and the embed worker
        // is woken.
        let woke = tokio::spawn({
            let wake = wake.clone();
            async move { wake.notified().await }
        });
        let (status, body) = put_good(&base, page.id, &key, ARTICLE.as_bytes()).await;
        assert_eq!(status, reqwest::StatusCode::ACCEPTED, "{body}");
        assert_eq!(body, r#"{"status":"indexed"}"#);
        tokio::time::timeout(std::time::Duration::from_secs(5), woke)
            .await
            .expect("the embed worker is woken")
            .unwrap();
        let (state_now, source, text) = contents(&pool, page.id).await;
        assert_eq!(state_now, "fetched");
        assert_eq!(source.as_deref(), Some("captured"));
        assert!(text.unwrap().contains("The borrow checker enforces"));

        // 404: another user's bookmark, and one that does not exist.
        for id in [theirs.id, Uuid::new_v4()] {
            let (status, body) = put_good(&base, id, &key, ARTICLE.as_bytes()).await;
            assert_eq!(status, reqwest::StatusCode::NOT_FOUND, "{body}");
            assert_eq!(body, r#"{"error":"not found"}"#);
        }

        // 400: a page URL that is missing or not absolute http(s).
        for page_url in [None, Some("not a url"), Some("file:///etc/passwd")] {
            let (status, body) = put(
                &base,
                page.id,
                Some(&key),
                page_url,
                Some("text/html"),
                Some("gzip"),
                gzip(ARTICLE.as_bytes()),
            )
            .await;
            assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(body, r#"{"error":"invalid_page_url"}"#);
        }

        // 409 before the body is read: a mismatched URL with a 6 MiB body
        // costs nothing.
        let huge = gzip(
            format!(
                "<html><body><p>{}</p></body></html>",
                "x".repeat(6 * 1024 * 1024)
            )
            .as_bytes(),
        );
        let (status, body) = put(
            &base,
            page.id,
            Some(&key),
            Some("https://elsewhere.example/a-post"),
            Some("text/html"),
            Some("gzip"),
            huge.clone(),
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::CONFLICT, "{body}");
        assert_eq!(body, r#"{"error":"url_mismatch"}"#);

        // 400: wrong content type, missing encoding, a body that is not gzip.
        let (status, body) = put(
            &base,
            page.id,
            Some(&key),
            Some(BOOKMARK_URL),
            Some("application/pdf"),
            Some("gzip"),
            gzip(b"%PDF-1.7"),
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body, r#"{"error":"unsupported_content_type"}"#);

        let (status, body) = put(
            &base,
            page.id,
            Some(&key),
            Some(BOOKMARK_URL),
            Some("text/html"),
            None,
            gzip(ARTICLE.as_bytes()),
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body, r#"{"error":"unsupported_encoding"}"#);

        for body_bytes in [b"not gzip".to_vec(), Vec::new(), gzip(b"").to_vec()] {
            let (status, body) = put(
                &base,
                page.id,
                Some(&key),
                Some(BOOKMARK_URL),
                Some("text/html"),
                Some("gzip"),
                body_bytes,
            )
            .await;
            assert_eq!(status, reqwest::StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(body, r#"{"error":"bad_gzip"}"#);
        }

        // 413: the decompressed page is over the cap.
        let (status, body) = put(
            &base,
            page.id,
            Some(&key),
            Some(BOOKMARK_URL),
            Some("text/html"),
            Some("gzip"),
            huge,
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::PAYLOAD_TOO_LARGE, "{body}");
        assert_eq!(body, r#"{"error":"too large"}"#);

        // 422: Readability found no article (a challenge page included).
        for html in [
            "<html><body></body></html>",
            "<html><head><title>Just a moment...</title></head></html>",
        ] {
            let (status, body) = put_good(&base, page.id, &key, html.as_bytes()).await;
            assert_eq!(
                status,
                reqwest::StatusCode::UNPROCESSABLE_ENTITY,
                "{html}: {body}"
            );
            assert_eq!(body, r#"{"error":"no readable article"}"#);
        }
        // None of the refusals touched the stored text.
        assert_eq!(
            contents(&pool, page.id).await.1.as_deref(),
            Some("captured"),
            "a refused capture leaves the stored article alone"
        );

        // A capture of a page whose text was stored by a fetch replaces it.
        sqlx::query("UPDATE bookmark_contents SET source = 'fetched' WHERE bookmark_id = $1")
            .bind(page.id)
            .execute(&pool)
            .await
            .unwrap();
        let (status, _) = put_good(&base, page.id, &key, ARTICLE.as_bytes()).await;
        assert_eq!(status, reqwest::StatusCode::ACCEPTED);
        assert_eq!(
            contents(&pool, page.id).await.1.as_deref(),
            Some("captured")
        );

        sqlx::query("DELETE FROM users WHERE id IN ($1, $2)")
            .bind(owner)
            .bind(other)
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }

    /// The compressed cap, and the switch a self-hoster uses to turn the
    /// write surface off.
    #[tokio::test]
    #[ignore = "requires a migrated Postgres database (DATABASE_URL)"]
    async fn caps_and_the_off_switch_are_enforced() {
        let _serial = SERIAL.lock().await;
        let pool = pool().await;
        let db = Arc::new(PostgresPool::new(pool.clone()));
        let owner = user(&pool).await;
        let page = bookmark(&db, owner, BOOKMARK_URL, "A post", None, &[]).await;
        let key = AuthService::new(db.clone(), db.clone(), db.clone())
            .create_api_key(owner, "capture caps")
            .await
            .unwrap();

        // A 1 KiB cap, so both caps can be reached without a large body.
        let base = serve(state(db.clone(), config(true, 1024))).await;
        // Compressed over the cap: hard-to-compress bytes.
        let incompressible: Vec<u8> = (0..4096u32).map(|n| (n % 251) as u8).collect();
        let (status, body) = put(
            &base,
            page.id,
            Some(&key),
            Some(BOOKMARK_URL),
            Some("text/html"),
            Some("gzip"),
            gzip(&incompressible),
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::PAYLOAD_TOO_LARGE, "{body}");
        // Decompressed over the cap: a small gzip of a larger page.
        let page_over_cap = format!("<html><body><p>{}</p></body></html>", "a ".repeat(2048));
        let (status, body) = put(
            &base,
            page.id,
            Some(&key),
            Some(BOOKMARK_URL),
            Some("text/html"),
            Some("gzip"),
            gzip(page_over_cap.as_bytes()),
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::PAYLOAD_TOO_LARGE, "{body}");

        // PAGE_CAPTURE_ENABLED=0: the route answers 404, like a missing
        // bookmark, and stores nothing.
        let off = serve(state(db.clone(), config(false, 5 * 1024 * 1024))).await;
        let (status, body) = put_good(&off, page.id, &key, ARTICLE.as_bytes()).await;
        assert_eq!(status, reqwest::StatusCode::NOT_FOUND, "{body}");
        assert_eq!(contents(&pool, page.id).await.1, None);

        // No CORS grant: a foreign origin's preflight gets no allow-header,
        // so a cross-site page cannot send this PUT at all.
        let response = reqwest::Client::new()
            .request(
                reqwest::Method::OPTIONS,
                format!("{base}/api/v1/bookmarks/{}/page", page.id),
            )
            .header("origin", "https://evil.example")
            .header("access-control-request-method", "PUT")
            .header(
                "access-control-request-headers",
                "content-encoding, x-page-url",
            )
            .send()
            .await
            .unwrap();
        assert!(
            response
                .headers()
                .get("access-control-allow-origin")
                .is_none(),
            "{:?}",
            response.headers()
        );

        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(owner)
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }

    /// The `queued` answer's other half: the ingest worker parses the
    /// payload the route stored and stores the text.
    #[tokio::test]
    #[ignore = "requires a migrated Postgres database (DATABASE_URL)"]
    async fn a_queued_capture_is_parsed_by_the_drain() {
        let _serial = SERIAL.lock().await;
        let pool = pool().await;
        let db = Arc::new(PostgresPool::new(pool.clone()));
        let owner = user(&pool).await;
        let page = bookmark(&db, owner, BOOKMARK_URL, "A post", None, &[]).await;

        let capture = NewCapture {
            bookmark_id: page.id,
            user_id: owner,
            page_url: BOOKMARK_URL.to_string(),
            html_gz: gzip(ARTICLE.as_bytes()),
        };
        let service = CaptureService::new(db.clone());
        assert!(service.queue_capture(&capture).await.unwrap());
        assert_eq!(contents(&pool, page.id).await.1, None, "nothing stored yet");

        let worker =
            CaptureWorker::new(db.clone(), Arc::new(HtmlExtractor::new(1)), 5 * 1024 * 1024);
        assert_eq!(worker.drain_once(8).await.unwrap(), 1);
        let (status, source, text) = contents(&pool, page.id).await;
        assert_eq!(status, "fetched");
        assert_eq!(source.as_deref(), Some("captured"));
        assert!(text.unwrap().contains("Lifetimes are the names"));
        let queued: i64 = sqlx::query_scalar("SELECT count(*) FROM bookmark_page_captures")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(queued, 0, "the drain took the payload");

        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(owner)
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }
}
