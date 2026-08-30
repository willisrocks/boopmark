use askama::Template;
use axum::Form;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::response::{Html, IntoResponse, Redirect};
use serde::Deserialize;
use std::convert::Infallible;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::app::bookmarks::ProgressEvent;
use crate::domain::error::DomainError;
use crate::domain::llm_settings::{
    ANTHROPIC_MODEL_OPTIONS, IMAGE_GENERATION_MODEL_OPTIONS, OPENAI_MODEL_OPTIONS,
};
use crate::web::extractors::AuthUser;
use crate::web::pages::shared::UserView;
use crate::web::state::{AppState, Bookmarks};

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

#[derive(Template)]
#[template(path = "settings/tag_consolidation_result.html")]
struct TagConsolidationResultFragment {
    success_message: Option<String>,
    error_message: Option<String>,
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

struct ProviderOptionView {
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
    metadata_provider: String,
    has_anthropic_api_key: bool,
    anthropic_model_options: Vec<ModelOptionView>,
    has_openai_api_key: bool,
    openai_model: String,
    openai_model_options: Vec<ModelOptionView>,
    image_enabled: bool,
    image_model: String,
    image_model_options: Vec<ModelOptionView>,
    metadata_provider_options: Vec<ProviderOptionView>,
    success_message: Option<String>,
    api_keys: Vec<ApiKeyView>,
}

fn render(t: &impl Template) -> axum::response::Response {
    match t.render() {
        Ok(body) => Html(body).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Deserialize, Default)]
struct SettingsQuery {
    saved: Option<String>,
}

#[derive(Deserialize)]
struct SettingsForm {
    llm_enabled: Option<String>,
    metadata_provider: Option<String>,
    delete_anthropic_api_key: Option<String>,
    anthropic_api_key: Option<String>,
    anthropic_model: Option<String>,
    delete_openai_api_key: Option<String>,
    openai_api_key: Option<String>,
    openai_model: Option<String>,
    /// `image_enabled`/`image_model` are the current template names. The
    /// `image_generation_*` aliases keep older clients/forms compatible.
    image_enabled: Option<String>,
    image_model: Option<String>,
    image_generation_enabled: Option<String>,
    image_generation_model: Option<String>,
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

fn build_openai_model_option_views(current_model: &str) -> Vec<ModelOptionView> {
    let mut options = Vec::new();
    let is_official = OPENAI_MODEL_OPTIONS
        .iter()
        .any(|option| option.value == current_model);

    if !is_official {
        options.push(ModelOptionView {
            label: format!("Keep current saved model ({current_model})"),
            value: current_model.to_string(),
            selected: true,
        });
    }

    options.extend(OPENAI_MODEL_OPTIONS.iter().map(|option| ModelOptionView {
        label: option.label.to_string(),
        value: option.value.to_string(),
        selected: option.value == current_model,
    }));
    options
}

fn build_image_model_option_views(current_model: &str) -> Vec<ModelOptionView> {
    let mut options = Vec::new();
    let is_official = IMAGE_GENERATION_MODEL_OPTIONS
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
        IMAGE_GENERATION_MODEL_OPTIONS
            .iter()
            .map(|option| ModelOptionView {
                label: option.label.to_string(),
                value: option.value.to_string(),
                selected: option.value == current_model,
            }),
    );
    options
}

fn build_provider_option_views(current_provider: &str) -> Vec<ProviderOptionView> {
    [("Anthropic", "anthropic"), ("OpenAI", "openai")]
        .into_iter()
        .map(|(label, value)| ProviderOptionView {
            label: label.to_string(),
            value: value.to_string(),
            selected: value == current_provider,
        })
        .collect()
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
            let openai_model = settings.openai_model.clone();
            let image_model = settings.image_generation_model.clone();
            let metadata_provider = settings.metadata_provider.clone();
            let api_keys: Vec<ApiKeyView> = keys.into_iter().map(Into::into).collect();

            render(&SettingsPage {
                user: Some(user.into()),
                header_shows_bookmark_actions: false,
                email,
                llm_enabled: settings.enabled,
                metadata_provider: metadata_provider.clone(),
                has_anthropic_api_key: settings.has_anthropic_api_key,
                anthropic_model_options: build_model_option_views(&anthropic_model),
                has_openai_api_key: settings.has_openai_api_key,
                openai_model: openai_model.clone(),
                openai_model_options: build_openai_model_option_views(&openai_model),
                image_enabled: settings.image_generation_enabled,
                image_model: image_model.clone(),
                image_model_options: build_image_model_option_views(&image_model),
                metadata_provider_options: build_provider_option_views(&metadata_provider),
                success_message: query
                    .saved
                    .filter(|value| value == "1")
                    .map(|_| "Settings saved".to_string()),
                api_keys,
            })
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn save_settings(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Form(form): Form<SettingsForm>,
) -> axum::response::Response {
    let enabled = form.llm_enabled.is_some();
    let metadata_provider = form.metadata_provider;
    let delete_key = form.delete_anthropic_api_key.is_some();
    let submitted_api_key = form
        .anthropic_api_key
        .filter(|value| !value.trim().is_empty());

    let (anthropic_api_key, clear_anthropic_api_key) = if delete_key {
        (None, true)
    } else {
        (submitted_api_key, false)
    };

    let delete_openai_key = form.delete_openai_api_key.is_some();
    let submitted_openai_api_key = form.openai_api_key.filter(|value| !value.trim().is_empty());
    let (openai_api_key, clear_openai_api_key) = if delete_openai_key {
        (None, true)
    } else {
        (submitted_openai_api_key, false)
    };
    let image_enabled = form
        .image_generation_enabled
        .or(form.image_enabled)
        .is_some();
    let image_model = form.image_generation_model.or(form.image_model);

    match state
        .settings
        .save(
            user.id,
            crate::app::settings::SaveLlmSettingsInput {
                enabled,
                metadata_provider,
                anthropic_api_key,
                clear_anthropic_api_key,
                anthropic_model: form.anthropic_model,
                openai_api_key,
                clear_openai_api_key,
                openai_model: form.openai_model,
                image_generation_enabled: image_enabled,
                image_generation_model: image_model,
            },
        )
        .await
    {
        Ok(_) => Redirect::to("/settings?saved=1").into_response(),
        Err(DomainError::InvalidInput(_)) => StatusCode::BAD_REQUEST.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn create_api_key_htmx(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Form(form): Form<CreateApiKeyForm>,
) -> axum::response::Response {
    let name = form.name.trim().to_string();
    if name.is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }

    match state.auth.create_api_key(user.id, &name).await {
        Ok(raw_key) => {
            let keys = state.auth.list_api_keys(user.id).await.unwrap_or_default();
            let api_keys: Vec<ApiKeyView> = keys.into_iter().map(Into::into).collect();
            render(&ApiKeysCreatedFragment { raw_key, api_keys })
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
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
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn fix_images_stream(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> axum::response::Response {
    let user_id = user.id;

    {
        let mut jobs = state.active_image_fix_jobs.lock().unwrap();
        if jobs.contains(&user_id) {
            return StatusCode::CONFLICT.into_response();
        }
        jobs.insert(user_id);
    }

    let (tx, rx) = mpsc::channel::<ProgressEvent>(32);
    let jobs = state.active_image_fix_jobs.clone();

    tokio::spawn(async move {
        match &state.bookmarks {
            Bookmarks::Local(svc) => svc.fix_missing_images(user_id, tx).await,
            Bookmarks::S3(svc) => svc.fix_missing_images(user_id, tx).await,
        }
        jobs.lock().unwrap().remove(&user_id);
    });

    let stream = ReceiverStream::new(rx).map(|event| {
        let json = serde_json::to_string(&event).unwrap_or_default();
        Ok::<_, Infallible>(Event::default().data(json))
    });

    Sse::new(stream).into_response()
}

async fn consolidate_tags_htmx(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> axum::response::Response {
    let user_id = user.id;

    {
        let mut jobs = state.active_tag_consolidation_jobs.lock().unwrap();
        if jobs.contains(&user_id) {
            return StatusCode::CONFLICT.into_response();
        }
        jobs.insert(user_id);
    }

    let result = state.tag_consolidation.consolidate(user_id).await;

    state
        .active_tag_consolidation_jobs
        .lock()
        .unwrap()
        .remove(&user_id);

    match result {
        Ok(stats) => render(&TagConsolidationResultFragment {
            success_message: Some(format!(
                "Consolidated {} tag{tplural} into {} across {} bookmark{bplural}.",
                stats.tags_before,
                stats.tags_after,
                stats.bookmarks_changed,
                tplural = if stats.tags_before == 1 { "" } else { "s" },
                bplural = if stats.bookmarks_changed == 1 {
                    ""
                } else {
                    "s"
                },
            )),
            error_message: None,
        }),
        Err(DomainError::InvalidInput(msg)) => render(&TagConsolidationResultFragment {
            success_message: None,
            error_message: Some(msg),
        }),
        Err(_) => render(&TagConsolidationResultFragment {
            success_message: None,
            error_message: Some("Consolidation failed. Try again.".to_string()),
        }),
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
        .route(
            "/settings/fix-images/stream",
            axum::routing::get(fix_images_stream),
        )
        .route(
            "/settings/consolidate-tags",
            axum::routing::post(consolidate_tags_htmx),
        )
}

#[cfg(test)]
mod tests {
    use super::{
        build_image_model_option_views, build_model_option_views, build_openai_model_option_views,
        build_provider_option_views,
    };

    #[test]
    fn official_models_render_only_the_three_official_options() {
        let options = build_model_option_views("claude-sonnet-5");

        assert_eq!(options.len(), 3);
        assert_eq!(options[0].label, "Claude Opus 5");
        assert_eq!(options[0].value, "claude-opus-5");
        assert!(!options[0].selected);
        assert_eq!(options[1].label, "Claude Sonnet 5");
        assert_eq!(options[1].value, "claude-sonnet-5");
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
        assert_eq!(options[1].value, "claude-opus-5");
        assert_eq!(options[2].value, "claude-sonnet-5");
        assert_eq!(options[3].value, "claude-haiku-4-5-20251001");
    }

    #[test]
    fn openai_models_render_the_three_requested_gpt_56_options() {
        let options = build_openai_model_option_views("gpt-5.6-terra");

        assert_eq!(options.len(), 3);
        assert_eq!(options[0].value, "gpt-5.6-luna");
        assert!(!options[0].selected);
        assert_eq!(options[1].value, "gpt-5.6-terra");
        assert!(options[1].selected);
        assert_eq!(options[2].value, "gpt-5.6-sol");
        assert!(!options[2].selected);
    }

    #[test]
    fn image_models_render_gpt_image_2() {
        let options = build_image_model_option_views("gpt-image-2");
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].value, "gpt-image-2");
        assert!(options[0].selected);
    }

    #[test]
    fn provider_options_include_anthropic_and_openai() {
        let options = build_provider_option_views("openai");
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].value, "anthropic");
        assert!(!options[0].selected);
        assert_eq!(options[1].value, "openai");
        assert!(options[1].selected);
    }
}
