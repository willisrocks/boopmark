pub mod auth;
pub mod bookmarks;

use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;

use crate::domain::error::DomainError;
use crate::web::middleware::api_error_json::json_error_layer;
use crate::web::state::AppState;
use axum::middleware;
use axum::Router;

#[derive(Serialize)]
pub struct ErrorBody {
    pub error: String,
}

/// Map a DomainError to an appropriate HTTP status + JSON body for API endpoints.
pub fn error_response(err: DomainError) -> (StatusCode, Json<ErrorBody>) {
    let (status, message) = match &err {
        DomainError::InvalidInput(detail) => {
            (StatusCode::BAD_REQUEST, format!("invalid input: {detail}"))
        }
        DomainError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
        DomainError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".to_string()),
        DomainError::AlreadyExists => (StatusCode::CONFLICT, "already exists".to_string()),
        DomainError::Internal(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal error".to_string(),
        ),
    };
    (status, Json(ErrorBody { error: message }))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .nest("/bookmarks", bookmarks::routes())
        .nest("/auth", auth::routes())
        .layer(middleware::from_fn(json_error_layer))
}
