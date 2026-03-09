use askama::Template;
use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse};
use serde::Deserialize;
use uuid::Uuid;

use crate::domain::bookmark::{Bookmark, BookmarkFilter, BookmarkSort, CreateBookmark, UrlMetadata};
use crate::domain::ports::llm_enricher::{EnrichmentInput, EnrichmentOutput};
use crate::web::extractors::AuthUser;
use crate::web::middleware::auth::is_htmx;
use crate::web::pages::shared::UserView;
use crate::web::state::{AppState, Bookmarks};

fn non_empty(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

macro_rules! with_bookmarks {
    ($bookmarks:expr, $svc:ident => $body:expr) => {
        match $bookmarks {
            Bookmarks::Local($svc) => $body,
            Bookmarks::S3($svc) => $body,
        }
    };
}

/// Pre-computed view of a bookmark for templates.
struct BookmarkView {
    id: Uuid,
    url: String,
    title: Option<String>,
    description: Option<String>,
    image_url: Option<String>,
    tags: Vec<String>,
    created_at_display: String,
}

impl From<Bookmark> for BookmarkView {
    fn from(b: Bookmark) -> Self {
        Self {
            id: b.id,
            url: b.url,
            title: b.title,
            description: b.description,
            image_url: b.image_url,
            tags: b.tags,
            created_at_display: b.created_at.format("%b %d, %Y").to_string(),
        }
    }
}

/// Tag with pre-computed active state for the filter bar.
struct TagView {
    name: String,
    active: bool,
}

fn render(t: &impl Template) -> axum::response::Response {
    match t.render() {
        Ok(body) => Html(body).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Template)]
#[template(path = "bookmarks/grid.html")]
struct GridPage {
    user: Option<UserView>,
    header_shows_bookmark_actions: bool,
    bookmarks: Vec<BookmarkView>,
    filter_tags: Vec<TagView>,
    sort: String,
    suggest_title: String,
    suggest_description: String,
    suggest_preview_image_url: Option<String>,
    suggest_tags: String,
}

#[derive(Template)]
#[template(path = "bookmarks/list_with_filters.html")]
struct BookmarkListWithFilters {
    bookmarks: Vec<BookmarkView>,
    filter_tags: Vec<TagView>,
    sort: String,
}

#[derive(Template)]
#[template(path = "bookmarks/card.html")]
struct BookmarkCard {
    bookmark: BookmarkView,
}

#[derive(Template)]
#[template(path = "bookmarks/add_modal_suggest_fields.html")]
struct SuggestFields {
    suggest_title: String,
    suggest_description: String,
    suggest_preview_image_url: Option<String>,
    suggest_tags: String,
}

#[derive(Deserialize)]
pub struct ListQuery {
    search: Option<String>,
    tags: Option<String>,
    sort: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> axum::response::Response {
    let active_tags: Vec<String> = query
        .tags
        .as_deref()
        .filter(|t| !t.is_empty())
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();

    let sort_str = query.sort.clone().unwrap_or_else(|| "newest".into());
    let sort = match sort_str.as_str() {
        "oldest" => BookmarkSort::Oldest,
        "title" => BookmarkSort::Title,
        "domain" => BookmarkSort::Domain,
        _ => BookmarkSort::Newest,
    };

    let filter = BookmarkFilter {
        search: query.search.and_then(|s| {
            let trimmed = s.trim().to_string();
            if trimmed.is_empty() { None } else { Some(trimmed) }
        }),
        tags: if active_tags.is_empty() {
            None
        } else {
            Some(active_tags.clone())
        },
        sort: Some(sort),
        ..Default::default()
    };

    let bookmarks = with_bookmarks!(&state.bookmarks, svc => svc.list(user.id, filter).await)
        .unwrap_or_default();

    let bookmark_views: Vec<BookmarkView> = bookmarks.into_iter().map(Into::into).collect();

    // Query all distinct tags for the filter bar (used by both HTMX and full-page paths).
    let all_tag_names = with_bookmarks!(&state.bookmarks, svc =>
        svc.all_tags(user.id).await
    )
    .unwrap_or_default();
    let filter_tags: Vec<TagView> = all_tag_names
        .into_iter()
        .map(|name| {
            let active = active_tags.contains(&name);
            TagView { name, active }
        })
        .collect();

    if is_htmx(&headers) {
        render(&BookmarkListWithFilters {
            bookmarks: bookmark_views,
            filter_tags,
            sort: sort_str,
        })
    } else {
        render(&GridPage {
            user: Some(user.into()),
            header_shows_bookmark_actions: true,
            bookmarks: bookmark_views,
            filter_tags,
            sort: sort_str,
            suggest_title: String::new(),
            suggest_description: String::new(),
            suggest_preview_image_url: None,
            suggest_tags: String::new(),
        })
    }
}

#[derive(Deserialize)]
pub struct CreateForm {
    url: String,
    title: Option<String>,
    description: Option<String>,
    tags_input: Option<String>,
}

#[derive(Deserialize)]
pub struct SuggestForm {
    url: String,
    title: Option<String>,
    description: Option<String>,
    tags_input: Option<String>,
}

pub async fn create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Form(form): Form<CreateForm>,
) -> axum::response::Response {
    let tags = form
        .tags_input
        .filter(|t| !t.is_empty())
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect());

    let input = CreateBookmark {
        url: form.url,
        title: form.title.filter(|t| !t.is_empty()),
        description: form.description.filter(|d| !d.is_empty()),
        image_url: None,
        domain: None,
        tags,
    };

    match with_bookmarks!(&state.bookmarks, svc => svc.create(user.id, input).await) {
        Ok(bookmark) => render(&BookmarkCard {
            bookmark: bookmark.into(),
        }),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn suggest(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Form(form): Form<SuggestForm>,
) -> axum::response::Response {
    let metadata = if form.url.trim().is_empty() {
        None
    } else {
        with_bookmarks!(&state.bookmarks, svc => svc.extract_metadata(&form.url).await).ok()
    };

    // Attempt LLM enrichment if user has it configured
    let enrichment = try_llm_enrich(&state, user.id, &form.url, &metadata).await;

    // Preserve user-typed tags; only use LLM tags if user hasn't typed any
    let user_tags = form.tags_input.and_then(non_empty);
    let suggest_tags = user_tags.unwrap_or_else(|| {
        enrichment
            .as_ref()
            .map(|e| e.tags.join(", "))
            .unwrap_or_default()
    });

    render(&SuggestFields {
        suggest_title: fill_if_blank(
            form.title,
            enrichment
                .as_ref()
                .and_then(|e| e.title.clone())
                .or_else(|| metadata.as_ref().and_then(|m| m.title.clone())),
        ),
        suggest_description: fill_if_blank(
            form.description,
            enrichment
                .as_ref()
                .and_then(|e| e.description.clone())
                .or_else(|| metadata.as_ref().and_then(|m| m.description.clone())),
        ),
        suggest_preview_image_url: metadata.and_then(|meta| meta.image_url),
        suggest_tags,
    })
}

async fn try_llm_enrich(
    state: &AppState,
    user_id: Uuid,
    url: &str,
    metadata: &Option<UrlMetadata>,
) -> Option<EnrichmentOutput> {
    let (api_key, model) = match state.settings.get_decrypted_api_key(user_id).await {
        Ok(Some(pair)) => pair,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!(user_id = %user_id, error = %e, "failed to load LLM settings for enrichment");
            return None;
        }
    };

    let input = EnrichmentInput {
        url: url.to_string(),
        scraped_title: metadata.as_ref().and_then(|m| m.title.clone()),
        scraped_description: metadata.as_ref().and_then(|m| m.description.clone()),
    };

    match state.enricher.enrich(&api_key, &model, input).await {
        Ok(output) => Some(output),
        Err(e) => {
            tracing::warn!(user_id = %user_id, url = %url, error = %e, "LLM enrichment failed, falling back to scrape-only");
            None
        }
    }
}

pub async fn delete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> axum::response::Response {
    match with_bookmarks!(&state.bookmarks, svc => svc.delete(id, user.id).await) {
        Ok(()) => Html("").into_response(),
        Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    }
}

fn fill_if_blank(current: Option<String>, suggested: Option<String>) -> String {
    current
        .and_then(non_empty)
        .or_else(|| suggested.and_then(non_empty))
        .unwrap_or_default()
}
