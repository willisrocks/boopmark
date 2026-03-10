use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
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
struct ApiKeyListItem {
    id: Uuid,
    name: String,
    created_at: DateTime<Utc>,
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
        Ok(keys) => {
            let items: Vec<ApiKeyListItem> = keys
                .into_iter()
                .map(|k| ApiKeyListItem {
                    id: k.id,
                    name: k.name,
                    created_at: k.created_at,
                })
                .collect();
            Ok(Json(items))
        }
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

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/keys", post(create_api_key).get(list_api_keys))
        .route("/keys/{id}", delete(delete_api_key))
}
