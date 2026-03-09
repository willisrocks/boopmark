use askama::Template;
use axum::Form;
use axum::Router;
use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse, Redirect};
use serde::Deserialize;

use crate::web::extractors::AuthUser;
use crate::web::state::AppState;

#[derive(Template)]
#[template(path = "settings/index.html")]
struct SettingsPage {
    email: String,
    llm_enabled: bool,
    has_anthropic_api_key: bool,
    anthropic_model: String,
    success_message: Option<String>,
}

fn render(t: &impl Template) -> axum::response::Response {
    match t.render() {
        Ok(body) => Html(body).into_response(),
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Deserialize, Default)]
struct SettingsQuery {
    saved: Option<String>,
}

#[derive(Deserialize)]
struct SettingsForm {
    llm_enabled: Option<String>,
    anthropic_api_key: Option<String>,
    clear_anthropic_api_key: Option<String>,
    anthropic_model: Option<String>,
}

async fn settings_page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(query): Query<SettingsQuery>,
) -> axum::response::Response {
    match state.settings.load(user.id).await {
        Ok(settings) => render(&SettingsPage {
            email: user.email,
            llm_enabled: settings.enabled,
            has_anthropic_api_key: settings.has_anthropic_api_key,
            anthropic_model: settings.anthropic_model,
            success_message: query
                .saved
                .filter(|value| value == "1")
                .map(|_| "Settings saved".to_string()),
        }),
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn save_settings(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Form(form): Form<SettingsForm>,
) -> axum::response::Response {
    let enabled = form.llm_enabled.is_some();
    let clear_anthropic_api_key = form.clear_anthropic_api_key.is_some();

    match state
        .settings
        .save(
            user.id,
            crate::app::settings::SaveLlmSettingsInput {
                enabled,
                anthropic_api_key: form.anthropic_api_key,
                clear_anthropic_api_key,
                anthropic_model: form.anthropic_model,
            },
        )
        .await
    {
        Ok(_) => Redirect::to("/settings?saved=1").into_response(),
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn legacy_api_keys_redirect() -> Redirect {
    Redirect::to("/settings")
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/settings",
            axum::routing::get(settings_page).post(save_settings),
        )
        .route(
            "/settings/api-keys",
            axum::routing::get(legacy_api_keys_redirect),
        )
}
