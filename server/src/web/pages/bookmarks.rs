use askama::Template;
use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse};
use serde::Deserialize;
use uuid::Uuid;

use crate::domain::bookmark::{
    Bookmark, BookmarkFilter, BookmarkSort, CreateBookmark, UpdateBookmark,
};
use crate::domain::error::DomainError;
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

fn error_response(e: DomainError) -> axum::response::Response {
    let status = match &e {
        DomainError::NotFound => StatusCode::NOT_FOUND,
        DomainError::Unauthorized => StatusCode::UNAUTHORIZED,
        DomainError::InvalidInput(_) => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, e.to_string()).into_response()
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    suggest_preview_image_url: Option<String>,
    suggest_tags: String,
}

#[derive(Template)]
#[template(path = "bookmarks/edit_modal.html")]
struct EditModal {
    bookmark_id: Uuid,
    suggest_title: String,
    suggest_description: String,
    suggest_tags: String,
    has_llm: bool,
}

#[derive(Template)]
#[template(path = "bookmarks/edit_suggest_fields.html")]
struct EditSuggestFields {
    suggest_title: String,
    suggest_description: String,
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
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
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
    let result = state.enrichment.suggest(user.id, &form.url, None).await;

    // Preserve user-typed tags; only use enrichment tags if user hasn't typed any
    let user_tags = form.tags_input.and_then(non_empty);
    let suggest_tags = user_tags.unwrap_or_else(|| {
        if result.tags.is_empty() {
            String::new()
        } else {
            result.tags.join(", ")
        }
    });

    render(&SuggestFields {
        suggest_title: fill_if_blank(form.title, result.title),
        suggest_description: fill_if_blank(form.description, result.description),
        suggest_preview_image_url: result.image_url,
        suggest_tags,
    })
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

pub async fn edit(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> axum::response::Response {
    let bookmark = match with_bookmarks!(&state.bookmarks, svc => svc.get(id, user.id).await) {
        Ok(b) => b,
        Err(e) => return error_response(e),
    };

    let has_llm = state
        .settings
        .get_decrypted_api_key(user.id)
        .await
        .ok()
        .flatten()
        .is_some();

    render(&EditModal {
        bookmark_id: bookmark.id,
        suggest_title: bookmark.title.unwrap_or_default(),
        suggest_description: bookmark.description.unwrap_or_default(),
        suggest_tags: bookmark.tags.join(", "),
        has_llm,
    })
}

#[derive(Deserialize)]
pub struct EditForm {
    title: Option<String>,
    description: Option<String>,
    tags_input: Option<String>,
}

/// Separate form struct for the edit-suggest endpoint.
/// Unlike `SuggestForm` (used by the add flow), this does NOT include a `url`
/// field because the edit modal form has no URL input -- the URL is fetched
/// from the database by bookmark ID.
#[derive(Deserialize)]
pub struct EditSuggestForm {
    title: Option<String>,
    description: Option<String>,
    tags_input: Option<String>,
}

pub async fn update(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
    Form(form): Form<EditForm>,
) -> axum::response::Response {
    let tags = form.tags_input.filter(|t| !t.is_empty()).map(|t| {
        t.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    });

    // Pass all three fields as Some(...) so the user can clear them.
    //
    // Title & description: the SQL uses
    //   CASE WHEN $n = '' THEN NULL ELSE COALESCE($n, col) END
    // so an empty string clears the field to NULL (matching never-set
    // semantics), a non-empty string updates it, and None (NULL) keeps
    // the old value.
    //
    // Tags: the SQL uses COALESCE($5, tags), so Some(vec![]) clears
    // the array to '{}' and None keeps the old tags.
    let input = UpdateBookmark {
        title: form.title,
        description: form.description,
        tags: Some(tags.unwrap_or_default()),
    };

    match with_bookmarks!(&state.bookmarks, svc => svc.update(id, user.id, input).await) {
        Ok(bookmark) => render(&BookmarkCard {
            bookmark: bookmark.into(),
        }),
        Err(e) => error_response(e),
    }
}

pub async fn edit_suggest(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
    Form(form): Form<EditSuggestForm>,
) -> axum::response::Response {
    // Get the bookmark to find its URL
    let bookmark = match with_bookmarks!(&state.bookmarks, svc => svc.get(id, user.id).await) {
        Ok(b) => b,
        Err(e) => return error_response(e),
    };

    // Get existing tags with counts for LLM context
    let existing_tags = with_bookmarks!(&state.bookmarks, svc =>
        svc.tags_with_counts(user.id).await
    )
    .ok();

    let result = state
        .enrichment
        .suggest(user.id, &bookmark.url, existing_tags)
        .await;

    // For edit suggest, always prefer enrichment suggestions over current
    // form values. The user explicitly asked for suggestions, so we replace
    // all fields. Fall back to current form values only if no suggestion exists.
    let suggest_tags = if !result.tags.is_empty() {
        result.tags.join(", ")
    } else {
        form.tags_input.and_then(non_empty).unwrap_or_default()
    };

    let suggest_title = result
        .title
        .and_then(non_empty)
        .unwrap_or_else(|| form.title.and_then(non_empty).unwrap_or_default());

    let suggest_description = result
        .description
        .and_then(non_empty)
        .unwrap_or_else(|| form.description.and_then(non_empty).unwrap_or_default());

    render(&EditSuggestFields {
        suggest_title,
        suggest_description,
        suggest_tags,
    })
}

fn fill_if_blank(current: Option<String>, suggested: Option<String>) -> String {
    current
        .and_then(non_empty)
        .or_else(|| suggested.and_then(non_empty))
        .unwrap_or_default()
}
