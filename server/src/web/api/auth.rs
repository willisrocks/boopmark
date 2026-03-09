use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

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

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/keys", post(create_api_key))
}
