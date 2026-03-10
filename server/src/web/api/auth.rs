use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

async fn create_api_key(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Json(input): Json<CreateApiKeyRequest>,
) -> impl IntoResponse {
    match state.auth.create_api_key(user.id, &input.name).await {
        Ok(key) => Ok((StatusCode::CREATED, Json(CreateApiKeyResponse { key }))),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
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
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn delete_api_key(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match state.auth.delete_api_key(id, user.id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn test_token(State(state): State<AppState>) -> impl IntoResponse {
    if !state.config.enable_e2e_auth {
        return Err(StatusCode::NOT_FOUND);
    }

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
