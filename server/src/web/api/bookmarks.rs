use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::app::enrichment::SuggestionResult;
use crate::domain::bookmark::*;
use crate::domain::error::DomainError;
use crate::web::extractors::AuthUser;
use crate::web::state::{AppState, Bookmarks};

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
    Query(params): Query<EnrichParams>,
    Json(mut input): Json<CreateBookmark>,
) -> impl IntoResponse {
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

    let result = with_bookmarks!(&state.bookmarks, svc => svc.create(user.id, input).await);
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
