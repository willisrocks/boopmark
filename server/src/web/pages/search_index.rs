use askama::Template;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use uuid::Uuid;

use crate::domain::error::DomainError;
use crate::domain::search::SearchIndexStatus;
use crate::web::extractors::AuthUser;
use crate::web::state::AppState;

/// Settings → Search index: what the background ingest has done with the
/// user's bookmarks, loaded into the settings page as a fragment.
#[derive(Template)]
#[template(path = "settings/search_index.html")]
struct SearchIndexPanel {
    total: String,
    /// Articles with stored text, whatever the source.
    indexed: String,
    /// The non-zero part of "· 198 fetched · 48 captured · 16 scraped".
    sources: Option<String>,
    pending: String,
    unfetched: String,
    has_unfetched: bool,
    /// Bookmarks embedded by the configured model, when embeddings are on.
    embedded: Option<String>,
    failing_domains: Vec<(String, String)>,
    /// The owner's deploy evidence: embedder model and extension versions.
    deployment: Option<String>,
    message: Option<String>,
}

impl SearchIndexPanel {
    fn new(status: SearchIndexStatus, deployment: Option<String>, message: Option<String>) -> Self {
        let indexed = status.indexed;
        let sources: Vec<String> = [
            (indexed.fetched, "fetched"),
            (indexed.captured, "captured"),
            (indexed.scraped, "scraped"),
        ]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, label)| format!("{} {label}", thousands(n)))
        .collect();
        Self {
            total: thousands(status.total),
            indexed: thousands(indexed.total()),
            sources: (!sources.is_empty()).then(|| sources.join(" · ")),
            pending: thousands(status.pending),
            unfetched: thousands(status.unfetched),
            has_unfetched: status.unfetched > 0,
            embedded: status.embedded.map(thousands),
            failing_domains: status
                .failing_domains
                .into_iter()
                .map(|(host, n)| (host, thousands(n)))
                .collect(),
            deployment,
            message,
        }
    }
}

/// `1290` → `1,290`.
fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    if n < 0 { format!("-{out}") } else { out }
}

fn render(t: &impl Template) -> Response {
    match t.render() {
        Ok(body) => Html(body).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn render_panel(
    state: &AppState,
    user: &crate::domain::user::User,
    message: Option<String>,
) -> Response {
    let status = match state.search.index_status(user.id).await {
        Ok(status) => status,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let deployment = if user.is_owner() {
        state.search.deployment_line().await.ok()
    } else {
        None
    };
    render(&SearchIndexPanel::new(status, deployment, message))
}

async fn panel(State(state): State<AppState>, AuthUser(user): AuthUser) -> Response {
    render_panel(&state, &user, None).await
}

async fn retry_failed(State(state): State<AppState>, AuthUser(user): AuthUser) -> Response {
    let message = match state.search.retry_failed_fetches(user.id).await {
        Ok(0) => "Nothing to retry.".to_string(),
        Ok(1) => "1 bookmark will be fetched again.".to_string(),
        Ok(n) => format!("{} bookmarks will be fetched again.", thousands(n as i64)),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    render_panel(&state, &user, Some(message)).await
}

/// The card's "Refetch" button; the response replaces the button.
async fn refetch(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> Response {
    match state.search.refetch(user.id, id).await {
        Ok(()) => Html(
            "<span class=\"text-xs text-gray-500\" data-testid=\"bookmark-refetch-queued\">Refetching…</span>",
        )
        .into_response(),
        Err(DomainError::NotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/settings/search-index", get(panel))
        .route("/settings/search-index/retry", post(retry_failed))
        .route("/bookmarks/{id}/refetch", post(refetch))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::search::ContentSourceCounts;

    fn status() -> SearchIndexStatus {
        SearchIndexStatus {
            total: 1290,
            indexed: ContentSourceCounts {
                fetched: 1140,
                captured: 48,
                scraped: 16,
            },
            pending: 0,
            unfetched: 86,
            embedded: Some(1290),
            failing_domains: vec![("medium.com".into(), 40), ("x.com".into(), 3)],
        }
    }

    #[test]
    fn counts_get_thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1290), "1,290");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn panel_shows_counts_failing_domains_and_retry() {
        let body = SearchIndexPanel::new(status(), None, None)
            .render()
            .unwrap();
        assert!(body.contains(
            "Indexed 1,204 of 1,290 articles · 1,140 fetched · 48 captured · 16 scraped · 86 couldn"
        ));
        assert!(body.contains("data-testid=\"search-index-retry\""));
        assert!(body.contains("medium.com"));
        assert!(body.contains("Searchable by meaning: 1,290"));
        assert!(!body.contains("search-index-deployment"));
    }

    #[test]
    fn owner_line_and_disabled_embeddings() {
        let mut status = status();
        status.embedded = None;
        status.unfetched = 0;
        status.failing_domains.clear();
        let body = SearchIndexPanel::new(
            status,
            Some("disabled · vector 0.8.2 · pg_trgm 1.6".into()),
            Some("Nothing to retry.".into()),
        )
        .render()
        .unwrap();
        assert!(body.contains("data-testid=\"search-index-deployment\""));
        assert!(body.contains("disabled · vector 0.8.2 · pg_trgm 1.6"));
        assert!(body.contains("Nothing to retry."));
        assert!(!body.contains("Searchable by meaning"));
        assert!(!body.contains("search-index-retry"));
    }
}
