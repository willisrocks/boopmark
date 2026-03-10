use askama::Template;
use axum::Form;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Redirect};
use serde::Deserialize;
use uuid::Uuid;

use crate::domain::error::DomainError;
use crate::domain::llm_settings::ANTHROPIC_MODEL_OPTIONS;
use crate::web::extractors::AuthUser;
use crate::web::pages::shared::UserView;
use crate::web::state::AppState;

struct ApiKeyView {
    id: String,
    name: String,
    created_at_display: String,
}

impl From<crate::domain::ports::api_key_repo::ApiKey> for ApiKeyView {
    fn from(k: crate::domain::ports::api_key_repo::ApiKey) -> Self {
        Self {
            id: k.id.to_string(),
            name: k.name,
            created_at_display: k.created_at.format("%b %d, %Y").to_string(),
        }
    }
}

#[derive(Template)]
#[template(path = "settings/api_keys_list.html")]
struct ApiKeysListFragment {
    api_keys: Vec<ApiKeyView>,
}

#[derive(Template)]
#[template(path = "settings/api_keys_created.html")]
struct ApiKeysCreatedFragment {
    raw_key: String,
    api_keys: Vec<ApiKeyView>,
}

#[derive(Deserialize)]
struct CreateApiKeyForm {
    name: String,
}

struct ModelOptionView {
    label: String,
    value: String,
    selected: bool,
}

#[derive(Template)]
#[template(path = "settings/index.html")]
struct SettingsPage {
    user: Option<UserView>,
    header_shows_bookmark_actions: bool,
    email: String,
    llm_enabled: bool,
    has_anthropic_api_key: bool,
    anthropic_model_options: Vec<ModelOptionView>,
    success_message: Option<String>,
    api_keys: Vec<ApiKeyView>,
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
    delete_anthropic_api_key: Option<String>,
    anthropic_api_key: Option<String>,
    anthropic_model: Option<String>,
}

fn build_model_option_views(current_model: &str) -> Vec<ModelOptionView> {
    let mut options = Vec::new();
    let is_official = ANTHROPIC_MODEL_OPTIONS
        .iter()
        .any(|option| option.value == current_model);

    if !is_official {
        options.push(ModelOptionView {
            label: format!("Keep current saved model ({current_model})"),
            value: current_model.to_string(),
            selected: true,
        });
    }

    options.extend(
        ANTHROPIC_MODEL_OPTIONS
            .iter()
            .map(|option| ModelOptionView {
                label: option.label.to_string(),
                value: option.value.to_string(),
                selected: option.value == current_model,
            }),
    );

    options
}

async fn settings_page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(query): Query<SettingsQuery>,
) -> axum::response::Response {
    let settings_result = state.settings.load(user.id).await;
    let keys_result = state.auth.list_api_keys(user.id).await;

    match (settings_result, keys_result) {
        (Ok(settings), Ok(keys)) => {
            let email = user.email.clone();
            let anthropic_model = settings.anthropic_model;
            let api_keys: Vec<ApiKeyView> = keys.into_iter().map(Into::into).collect();

            render(&SettingsPage {
                user: Some(user.into()),
                header_shows_bookmark_actions: false,
                email,
                llm_enabled: settings.enabled,
                has_anthropic_api_key: settings.has_anthropic_api_key,
                anthropic_model_options: build_model_option_views(&anthropic_model),
                success_message: query
                    .saved
                    .filter(|value| value == "1")
                    .map(|_| "Settings saved".to_string()),
                api_keys,
            })
        }
        _ => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn save_settings(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Form(form): Form<SettingsForm>,
) -> axum::response::Response {
    let enabled = form.llm_enabled.is_some();
    let delete_key = form.delete_anthropic_api_key.is_some();
    let submitted_api_key = form
        .anthropic_api_key
        .filter(|value| !value.trim().is_empty());

    let (anthropic_api_key, clear_anthropic_api_key) = if delete_key {
        (None, true)
    } else {
        (submitted_api_key, false)
    };

    match state
        .settings
        .save(
            user.id,
            crate::app::settings::SaveLlmSettingsInput {
                enabled,
                anthropic_api_key,
                clear_anthropic_api_key,
                anthropic_model: form.anthropic_model,
            },
        )
        .await
    {
        Ok(_) => Redirect::to("/settings?saved=1").into_response(),
        Err(DomainError::InvalidInput(_)) => axum::http::StatusCode::BAD_REQUEST.into_response(),
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn create_api_key_htmx(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Form(form): Form<CreateApiKeyForm>,
) -> axum::response::Response {
    let name = form.name.trim().to_string();
    if name.is_empty() {
        return axum::http::StatusCode::BAD_REQUEST.into_response();
    }

    match state.auth.create_api_key(user.id, &name).await {
        Ok(raw_key) => {
            let keys = state.auth.list_api_keys(user.id).await.unwrap_or_default();
            let api_keys: Vec<ApiKeyView> = keys.into_iter().map(Into::into).collect();
            render(&ApiKeysCreatedFragment { raw_key, api_keys })
        }
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn delete_api_key_htmx(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> axum::response::Response {
    match state.auth.delete_api_key(id, user.id).await {
        Ok(()) => {
            let keys = state.auth.list_api_keys(user.id).await.unwrap_or_default();
            let api_keys: Vec<ApiKeyView> = keys.into_iter().map(Into::into).collect();
            render(&ApiKeysListFragment { api_keys })
        }
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/settings",
            axum::routing::get(settings_page).post(save_settings),
        )
        .route(
            "/settings/api-keys",
            axum::routing::post(create_api_key_htmx),
        )
        .route(
            "/settings/api-keys/{id}",
            axum::routing::delete(delete_api_key_htmx),
        )
}

#[cfg(test)]
mod tests {
    use super::build_model_option_views;

    #[test]
    fn official_models_render_only_the_three_official_options() {
        let options = build_model_option_views("claude-sonnet-4-6");

        assert_eq!(options.len(), 3);
        assert_eq!(options[0].label, "Claude Opus 4.6");
        assert_eq!(options[0].value, "claude-opus-4-6");
        assert!(!options[0].selected);
        assert_eq!(options[1].label, "Claude Sonnet 4.6");
        assert_eq!(options[1].value, "claude-sonnet-4-6");
        assert!(options[1].selected);
        assert_eq!(options[2].label, "Claude Haiku 4.5");
        assert_eq!(options[2].value, "claude-haiku-4-5-20251001");
        assert!(!options[2].selected);
    }

    #[test]
    fn legacy_saved_model_gets_one_preservation_option_plus_the_official_options() {
        let options = build_model_option_views("claude-3-7-sonnet-latest");

        assert_eq!(options.len(), 4);
        assert_eq!(
            options[0].label,
            "Keep current saved model (claude-3-7-sonnet-latest)"
        );
        assert_eq!(options[0].value, "claude-3-7-sonnet-latest");
        assert!(options[0].selected);
        assert_eq!(options[1].value, "claude-opus-4-6");
        assert_eq!(options[2].value, "claude-sonnet-4-6");
        assert_eq!(options[3].value, "claude-haiku-4-5-20251001");
    }
}
