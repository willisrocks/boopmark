use askama::Template;
use axum::Form;
use axum::Router;
use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse, Redirect};
use serde::Deserialize;

use crate::domain::error::DomainError;
use crate::domain::llm_settings::ANTHROPIC_MODEL_OPTIONS;
use crate::web::extractors::AuthUser;
use crate::web::pages::shared::UserView;
use crate::web::state::AppState;

struct ModelOptionView {
    label: String,
    value: String,
    selected: bool,
}

struct ApiKeyView {
    id: String,
    name: String,
    created_at: String,
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
    created_key: Option<String>,
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
    created_key: Option<String>,
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

fn url_encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

async fn settings_page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(query): Query<SettingsQuery>,
) -> axum::response::Response {
    match state.settings.load(user.id).await {
        Ok(settings) => {
            let email = user.email.clone();
            let anthropic_model = settings.anthropic_model;

            let api_keys_result = state.auth.list_api_keys(user.id).await.unwrap_or_default();
            let api_keys: Vec<ApiKeyView> = api_keys_result
                .iter()
                .map(|k| ApiKeyView {
                    id: k.id.to_string(),
                    name: k.name.clone(),
                    created_at: k.created_at.format("%Y-%m-%d").to_string(),
                })
                .collect();

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
                created_key: query.created_key,
            })
        }
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
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

#[derive(Deserialize)]
struct CreateApiKeyForm {
    key_name: String,
}

async fn create_settings_api_key(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Form(form): Form<CreateApiKeyForm>,
) -> axum::response::Response {
    match state.auth.create_api_key(user.id, &form.key_name).await {
        Ok(key) => {
            let encoded = url_encode(&key);
            Redirect::to(&format!("/settings?created_key={encoded}")).into_response()
        }
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Deserialize)]
struct DeleteApiKeyForm {
    key_id: String,
}

async fn delete_settings_api_key(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Form(form): Form<DeleteApiKeyForm>,
) -> axum::response::Response {
    let id = match form.key_id.parse::<uuid::Uuid>() {
        Ok(id) => id,
        Err(_) => return axum::http::StatusCode::BAD_REQUEST.into_response(),
    };
    match state.auth.delete_api_key(id, user.id).await {
        Ok(()) => Redirect::to("/settings?saved=1").into_response(),
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn legacy_api_keys_redirect(AuthUser(_user): AuthUser) -> Redirect {
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
        .route(
            "/settings/api-keys/create",
            axum::routing::post(create_settings_api_key),
        )
        .route(
            "/settings/api-keys/delete",
            axum::routing::post(delete_settings_api_key),
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
