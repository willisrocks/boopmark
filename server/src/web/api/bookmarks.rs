use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::app::enrichment::SuggestionResult;
use crate::domain::bookmark::*;
use crate::domain::error::DomainError;
use crate::domain::ports::bookmark_repo::{CreateIdempotency, CreateIdempotencyClaim};
use crate::web::extractors::AuthUser;
use crate::web::state::{AppState, Bookmarks};

const CREATE_FINGERPRINT_VERSION: i16 = 1;

#[derive(Debug, Default, Deserialize)]
struct EnrichParams {
    #[serde(default)]
    suggest: bool,
}

#[derive(Deserialize)]
struct SuggestRequest {
    url: String,
}

/// Map DomainError to HTTP status + JSON body.
fn error_response(err: DomainError) -> impl IntoResponse {
    let (status, message) = match &err {
        DomainError::NotFound => (StatusCode::NOT_FOUND, "not found"),
        DomainError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
        DomainError::AlreadyExists => (StatusCode::CONFLICT, "already exists"),
        DomainError::OperationInProgress => {
            (StatusCode::SERVICE_UNAVAILABLE, "operation in progress")
        }
        DomainError::InvalidInput(_) => (StatusCode::BAD_REQUEST, "invalid input"),
        DomainError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
    };
    (
        status,
        Json(ErrorBody {
            error: message.to_string(),
        }),
    )
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

// --- Dispatch macro to avoid duplicating match arms ---

macro_rules! with_bookmarks {
    ($bookmarks:expr, $svc:ident => $body:expr) => {
        match $bookmarks {
            Bookmarks::Local($svc) => $body,
            Bookmarks::S3($svc) => $body,
        }
    };
}

// --- Query params ---

#[derive(Debug, Deserialize)]
pub struct ListParams {
    pub search: Option<String>,
    pub tags: Option<String>,
    pub sort: Option<BookmarkSort>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

impl From<ListParams> for BookmarkFilter {
    fn from(p: ListParams) -> Self {
        BookmarkFilter {
            search: p.search,
            tags: p.tags.map(|t| {
                t.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            }),
            sort: p.sort,
            limit: p.limit,
            offset: p.offset,
        }
    }
}

// --- Request/response types ---

#[derive(Deserialize)]
struct MetadataRequest {
    url: String,
}

// --- Suggestion helpers ---

/// Apply enrichment suggestions to a create input, filling only missing fields.
fn apply_create_suggestions(input: &mut CreateBookmark, suggestions: SuggestionResult) {
    if input.title.is_none() {
        input.title = suggestions.title;
    }
    if input.description.is_none() {
        input.description = suggestions.description;
    }
    if input.tags.as_ref().is_none_or(|t| t.is_empty()) && !suggestions.tags.is_empty() {
        input.tags = Some(suggestions.tags);
    }
    if input.image_url.is_none() {
        input.image_url = suggestions.image_url;
    }
    if input.domain.is_none() {
        input.domain = suggestions.domain;
    }
}

/// Apply enrichment suggestions to an update input, filling only missing fields.
fn apply_update_suggestions(input: &mut UpdateBookmark, suggestions: SuggestionResult) {
    if input.title.is_none() {
        input.title = suggestions.title;
    }
    if input.description.is_none() {
        input.description = suggestions.description;
    }
    if input.tags.as_ref().is_none_or(|t| t.is_empty()) && !suggestions.tags.is_empty() {
        input.tags = Some(suggestions.tags);
    }
}

#[derive(Serialize)]
struct CreateFingerprintV1<'a> {
    url: &'a str,
    title: &'a Option<String>,
    description: &'a Option<String>,
    image_url: &'a Option<String>,
    domain: &'a Option<String>,
    tags: &'a Option<Vec<String>>,
}

/// Hash an explicit, versioned view of the reviewed request before any
/// server-side enrichment. Future CreateBookmark fields do not silently alter
/// version 1. Omitted and explicit-null optional fields intentionally
/// normalize to the same deserialized value.
fn create_fingerprint(input: &CreateBookmark) -> String {
    let canonical = CreateFingerprintV1 {
        url: &input.url,
        title: &input.title,
        description: &input.description,
        image_url: &input.image_url,
        domain: &input.domain,
        tags: &input.tags,
    };
    let encoded = serde_json::to_vec(&canonical).expect("fingerprint input is serializable");
    format!("{:x}", Sha256::digest(encoded))
}

fn idempotency_from_headers(
    headers: &HeaderMap,
    input: &CreateBookmark,
) -> Result<Option<CreateIdempotency>, DomainError> {
    let Some(value) = headers.get("idempotency-key") else {
        return Ok(None);
    };
    let raw = value
        .to_str()
        .map_err(|_| DomainError::InvalidInput("invalid Idempotency-Key".to_string()))?
        .trim();
    let key = Uuid::parse_str(raw)
        .map_err(|_| DomainError::InvalidInput("Idempotency-Key must be a UUID".to_string()))?;
    Ok(Some(CreateIdempotency {
        key,
        fingerprint_version: CREATE_FINGERPRINT_VERSION,
        fingerprint: create_fingerprint(input),
    }))
}

// --- Handlers ---

async fn list_bookmarks(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> impl IntoResponse {
    let filter = BookmarkFilter::from(params);
    let result = with_bookmarks!(&state.bookmarks, svc => svc.list(user.id, filter).await);
    match result {
        Ok(bookmarks) => Ok(Json(bookmarks)),
        Err(e) => Err(error_response(e)),
    }
}

async fn suggest(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Json(input): Json<SuggestRequest>,
) -> Result<Json<SuggestionResult>, impl IntoResponse> {
    if input.url.trim().is_empty() {
        return Err(error_response(DomainError::InvalidInput(
            "url is required".to_string(),
        )));
    }
    if url::Url::parse(&input.url).is_err() {
        return Err(error_response(DomainError::InvalidInput(
            "invalid URL format".to_string(),
        )));
    }
    let result = state.enrichment.suggest(user.id, &input.url, None).await;
    Ok(Json(result))
}

async fn create_bookmark(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<EnrichParams>,
    Json(mut input): Json<CreateBookmark>,
) -> impl IntoResponse {
    // Capture the client-reviewed values before optional AI enrichment mutates
    // the create input.  This is the value a Chromium transport replay must
    // match, not whichever metadata happens to be scraped on the second try.
    let operation = match idempotency_from_headers(&headers, &input) {
        Ok(operation) => operation,
        Err(error) => return Err(error_response(error)),
    };

    // Claim before optional API-level suggestion enrichment.  A completed,
    // pending, or conflicting replay must not invoke an AI provider or any
    // metadata/screenshot side effect.
    if let Some(operation) = operation.as_ref() {
        let claim = with_bookmarks!(&state.bookmarks, svc =>
            svc.claim_create(user.id, operation.clone()).await
        );
        match claim {
            Ok(CreateIdempotencyClaim::Acquired) => {}
            Ok(CreateIdempotencyClaim::Completed(bookmark)) => {
                return Ok((StatusCode::CREATED, Json(*bookmark)));
            }
            Ok(CreateIdempotencyClaim::Pending) => {
                return Err(error_response(DomainError::OperationInProgress));
            }
            Ok(CreateIdempotencyClaim::Conflict) => {
                return Err(error_response(DomainError::AlreadyExists));
            }
            Err(error) => return Err(error_response(error)),
        }
    }

    if params.suggest {
        let existing_tags = with_bookmarks!(&state.bookmarks, svc =>
            svc.tags_with_counts(user.id).await
        )
        .ok();
        let suggestions = state
            .enrichment
            .suggest(user.id, &input.url, existing_tags)
            .await;
        apply_create_suggestions(&mut input, suggestions);
        // Ensure domain is set from URL so BookmarkService doesn't re-scrape just for domain
        if input.domain.is_none()
            && let Ok(parsed) = url::Url::parse(&input.url)
        {
            input.domain = parsed.host_str().map(|h| h.to_string());
        }
    }

    let result = with_bookmarks!(&state.bookmarks, svc => {
        match operation {
            Some(operation) => svc.create_claimed(user.id, input, operation).await,
            None => svc.create(user.id, input).await,
        }
    });
    match result {
        Ok(bookmark) => Ok((StatusCode::CREATED, Json(bookmark))),
        Err(e) => Err(error_response(e)),
    }
}

async fn get_bookmark(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let result = with_bookmarks!(&state.bookmarks, svc => svc.get(id, user.id).await);
    match result {
        Ok(bookmark) => Ok(Json(bookmark)),
        Err(e) => Err(error_response(e)),
    }
}

async fn update_bookmark(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<EnrichParams>,
    Json(mut input): Json<UpdateBookmark>,
) -> impl IntoResponse {
    if params.suggest {
        let bookmark = with_bookmarks!(&state.bookmarks, svc => svc.get(id, user.id).await);
        match bookmark {
            Ok(bm) => {
                let existing_tags = with_bookmarks!(&state.bookmarks, svc =>
                    svc.tags_with_counts(user.id).await
                )
                .ok();
                let suggestions = state
                    .enrichment
                    .suggest(user.id, &bm.url, existing_tags)
                    .await;
                apply_update_suggestions(&mut input, suggestions);
            }
            Err(e) => return Err(error_response(e)),
        }
    }

    let result = with_bookmarks!(&state.bookmarks, svc => svc.update(id, user.id, input).await);
    match result {
        Ok(bookmark) => Ok(Json(bookmark)),
        Err(e) => Err(error_response(e)),
    }
}

async fn delete_bookmark(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let result = with_bookmarks!(&state.bookmarks, svc => svc.delete(id, user.id).await);
    match result {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(e) => Err(error_response(e)),
    }
}

async fn extract_metadata(
    AuthUser(_user): AuthUser,
    State(state): State<AppState>,
    Json(input): Json<MetadataRequest>,
) -> impl IntoResponse {
    let result = with_bookmarks!(&state.bookmarks, svc => svc.extract_metadata(&input.url).await);
    match result {
        Ok(meta) => Ok(Json(meta)),
        Err(e) => Err(error_response(e)),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_bookmarks).post(create_bookmark))
        .route(
            "/{id}",
            get(get_bookmark)
                .put(update_bookmark)
                .delete(delete_bookmark),
        )
        .route("/metadata", post(extract_metadata))
        .route("/suggest", post(suggest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn reviewed_input() -> CreateBookmark {
        serde_json::from_value(serde_json::json!({
            "url": "https://example.com/article?run=one#fragment",
            "title": "",
            "description": "",
            "tags": []
        }))
        .expect("reviewed create payload")
    }

    #[test]
    fn idempotency_fingerprint_is_stable_and_covers_reviewed_values() {
        let input = reviewed_input();
        let same = input.clone();
        let mut changed = input.clone();
        changed.title = Some("edited".to_string());

        assert_eq!(create_fingerprint(&input), create_fingerprint(&same));
        assert_ne!(create_fingerprint(&input), create_fingerprint(&changed));
    }

    #[test]
    fn idempotency_header_requires_uuid_and_preserves_missing_header_behavior() {
        let input = reviewed_input();
        let mut headers = HeaderMap::new();
        assert_eq!(idempotency_from_headers(&headers, &input).unwrap(), None);

        headers.insert(
            "idempotency-key",
            HeaderValue::from_static("4d8c0f1b-6e7a-4d5f-9a2b-1c3e5f7a9b0d"),
        );
        let operation = idempotency_from_headers(&headers, &input)
            .expect("valid UUID header")
            .expect("operation");
        assert_eq!(
            operation.key,
            Uuid::parse_str("4d8c0f1b-6e7a-4d5f-9a2b-1c3e5f7a9b0d").unwrap()
        );
        assert_eq!(operation.fingerprint, create_fingerprint(&input));

        headers.insert("idempotency-key", HeaderValue::from_static("not-a-uuid"));
        assert!(matches!(
            idempotency_from_headers(&headers, &input),
            Err(DomainError::InvalidInput(_))
        ));
    }
}
