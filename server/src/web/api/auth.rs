use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::error::DomainError;
use crate::web::extractors::AuthUser;
use crate::web::state::AppState;

#[derive(Deserialize)]
struct CreateApiKeyRequest {
    name: String,
}

#[derive(Serialize)]
struct CreateApiKeyResponse {
    key: String,
}

#[derive(Serialize)]
struct ApiKeyView {
    id: Uuid,
    name: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

/// Map a DomainError to an appropriate HTTP status + JSON body for the auth API.
fn auth_error_response(err: DomainError) -> (StatusCode, Json<ErrorBody>) {
    let (status, message) = match &err {
        DomainError::InvalidInput(detail) => {
            (StatusCode::BAD_REQUEST, format!("invalid input: {detail}"))
        }
        DomainError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
        DomainError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".to_string()),
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal error".to_string(),
        ),
    };
    (status, Json(ErrorBody { error: message }))
}

async fn create_api_key(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Json(input): Json<CreateApiKeyRequest>,
) -> impl IntoResponse {
    match state.auth.create_api_key(user.id, &input.name).await {
        Ok(key) => Ok((StatusCode::CREATED, Json(CreateApiKeyResponse { key }))),
        Err(e) => Err(auth_error_response(e)),
    }
}

async fn list_api_keys(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    match state.auth.list_api_keys(user.id).await {
        Ok(keys) => Ok(Json(
            keys.into_iter()
                .map(|k| ApiKeyView {
                    id: k.id,
                    name: k.name,
                    created_at: k.created_at,
                })
                .collect::<Vec<_>>(),
        )),
        Err(e) => Err(auth_error_response(e)),
    }
}

async fn delete_api_key(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match state.auth.delete_api_key(id, user.id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(e) => Err(auth_error_response(e)),
    }
}

/// **TEST-ONLY endpoint.** Creates (or upserts) a hard-coded E2E test user and
/// returns a fresh session token as JSON. This exists solely for Playwright /
/// CLI E2E tests that need programmatic auth without browser-based OAuth.
///
/// Gated behind `ENABLE_E2E_AUTH=1` — returns 404 when the flag is off.
/// Must NEVER be enabled in production.
async fn test_token(State(state): State<AppState>) -> impl IntoResponse {
    if !state.config.enable_e2e_auth {
        return Err(StatusCode::NOT_FOUND);
    }

    tracing::warn!(
        "test-token endpoint called — this creates/upserts an E2E user and should only be used in test environments"
    );

    let user = state
        .auth
        .upsert_user(
            "e2e@boopmark.local".to_string(),
            Some("Boopmark E2E".to_string()),
            None,
        )
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let token = state
        .auth
        .create_session(user.id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({"session_token": token})))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/keys", get(list_api_keys).post(create_api_key))
        .route("/keys/{id}", delete(delete_api_key))
        .route("/test-token", post(test_token))
}
