use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::bookmark::*;
use crate::domain::error::DomainError;
use crate::web::extractors::AuthUser;
use crate::web::state::{AppState, Bookmarks};

/// Map DomainError to HTTP status + JSON body.
fn error_response(err: DomainError) -> impl IntoResponse {
    let (status, message) = match &err {
        DomainError::NotFound => (StatusCode::NOT_FOUND, "not found"),
        DomainError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
        DomainError::AlreadyExists => (StatusCode::CONFLICT, "already exists"),
        DomainError::InvalidInput(_) => (StatusCode::BAD_REQUEST, "invalid input"),
        DomainError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
    };
    (status, Json(ErrorBody { error: message.to_string() }))
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
            tags: p.tags.map(|t| t.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()),
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

async fn create_bookmark(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Json(input): Json<CreateBookmark>,
) -> impl IntoResponse {
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
    Json(input): Json<UpdateBookmark>,
) -> impl IntoResponse {
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
        .route("/{id}", get(get_bookmark).put(update_bookmark).delete(delete_bookmark))
        .route("/metadata", post(extract_metadata))
}
