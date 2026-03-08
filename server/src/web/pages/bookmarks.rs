use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::Form;
use serde::Deserialize;
use uuid::Uuid;

use crate::domain::bookmark::{Bookmark, BookmarkFilter, BookmarkSort, CreateBookmark};
use crate::web::extractors::AuthUser;
use crate::web::middleware::auth::is_htmx;
use crate::web::state::{AppState, Bookmarks};

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

/// Pre-computed view of a user for templates.
struct UserView {
    email: String,
    display_name: String,
    email_initial: String,
    image: Option<String>,
}

impl From<crate::domain::user::User> for UserView {
    fn from(u: crate::domain::user::User) -> Self {
        let email_initial = u.email.chars().next().unwrap_or('?').to_string();
        let display_name = u.name.clone().unwrap_or_default();
        Self {
            email: u.email,
            display_name,
            email_initial,
            image: u.image,
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
    bookmarks: Vec<BookmarkView>,
    filter_tags: Vec<TagView>,
    sort: String,
}

#[derive(Template)]
#[template(path = "bookmarks/list.html")]
struct BookmarkList {
    bookmarks: Vec<BookmarkView>,
}

#[derive(Template)]
#[template(path = "bookmarks/card.html")]
struct BookmarkCard {
    bookmark: BookmarkView,
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
        search: query.search,
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

    let all_tags = collect_all_tags(&bookmarks);
    let filter_tags: Vec<TagView> = all_tags
        .into_iter()
        .map(|name| {
            let active = active_tags.contains(&name);
            TagView { name, active }
        })
        .collect();

    let bookmark_views: Vec<BookmarkView> = bookmarks.into_iter().map(Into::into).collect();

    if is_htmx(&headers) {
        render(&BookmarkList {
            bookmarks: bookmark_views,
        })
    } else {
        render(&GridPage {
            user: Some(user.into()),
            bookmarks: bookmark_views,
            filter_tags,
            sort: sort_str,
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

fn collect_all_tags(bookmarks: &[Bookmark]) -> Vec<String> {
    let mut tags: Vec<String> = bookmarks
        .iter()
        .flat_map(|b| b.tags.iter().cloned())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    tags.sort();
    tags
}
