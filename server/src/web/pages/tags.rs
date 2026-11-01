//! The tag filter bar's data and popover search, and the `/tags` library
//! page: canonical tags with counts and aliases, rename, manual merge and
//! batch undo.

use askama::Template;
use axum::Form;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse};
use chrono::{Duration, Utc};
use serde::Deserialize;
use uuid::Uuid;

use crate::domain::error::DomainError;
use crate::domain::tag_alias::{
    ChangeKind, ProposalSource, ProposalStatus, TagChangeBatch, TagMerge,
};
use crate::domain::tag_library::{
    TagRow, TagSort, alias_preview, filter_rows, library_rows, search, sort_rows, top_tags,
};
use crate::web::extractors::AuthUser;
use crate::web::middleware::auth::is_htmx;
use crate::web::pages::shared::UserView;
use crate::web::state::AppState;

/// Tags shown as chips in the filter bar (spec: 10 on desktop). The
/// template hides all but the first four on narrow screens.
const TOP_TAGS: usize = 10;
const SEARCH_LIMIT: usize = 20;
const ALIAS_PREVIEW: usize = 3;
const HISTORY_LIMIT: i64 = 20;
/// Spec: the batch history keeps undo for 30 days.
const UNDO_WINDOW_DAYS: i64 = 30;

fn render(t: &impl Template) -> axum::response::Response {
    match t.render() {
        Ok(body) => Html(body).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn error_response(e: DomainError) -> axum::response::Response {
    tracing::error!("tag page failed: {e}");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

/// Parse the comma-separated `tags` filter of `/bookmarks`.
pub(crate) fn parse_tag_list(tags: Option<&str>) -> Vec<String> {
    tags.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

fn bookmarks_url(tags: &[String]) -> String {
    format!("/bookmarks?tags={}", urlencoding::encode(&tags.join(",")))
}

/// `active` with `tag` removed if present, added otherwise.
fn toggled(active: &[String], tag: &str) -> Vec<String> {
    if active.iter().any(|t| t == tag) {
        active.iter().filter(|t| *t != tag).cloned().collect()
    } else {
        let mut tags = active.to_vec();
        tags.push(tag.to_string());
        tags
    }
}

/// A chip in the filter bar; `url` toggles the tag in the current filter.
pub(crate) struct FilterChip {
    pub(crate) name: String,
    pub(crate) count: Option<i64>,
    pub(crate) url: String,
}

/// The filter bar: active tags first, then the most used tags, then a
/// "More tags" popover for the rest.
pub(crate) struct FilterBar {
    pub(crate) active: Vec<FilterChip>,
    pub(crate) top: Vec<FilterChip>,
    pub(crate) more_count: usize,
    pub(crate) tags_value: String,
}

impl FilterBar {
    pub(crate) fn new(tag_counts: &[(String, i64)], active: &[String]) -> Self {
        let rows = library_rows(tag_counts, &[]);
        let count_of = |name: &str| rows.iter().find(|r| r.name == name).map(|r| r.count);
        let chip = |name: &str, count: Option<i64>| FilterChip {
            name: name.to_string(),
            count,
            url: bookmarks_url(&toggled(active, name)),
        };
        let top: Vec<FilterChip> = top_tags(&rows, TOP_TAGS, active)
            .into_iter()
            .map(|r| chip(&r.name, Some(r.count)))
            .collect();
        let active_used = active.iter().filter(|t| count_of(t).is_some()).count();
        Self {
            active: active.iter().map(|t| chip(t, count_of(t))).collect(),
            more_count: rows.len() - top.len() - active_used,
            top,
            tags_value: active.join(","),
        }
    }
}

struct TagOption {
    name: String,
    count: i64,
    aliases: String,
    more_aliases: usize,
    active: bool,
    url: String,
}

#[derive(Template)]
#[template(path = "tags/search_results.html")]
struct SearchResults {
    options: Vec<TagOption>,
    query: String,
}

#[derive(Deserialize)]
pub struct SearchQuery {
    q: Option<String>,
    tags: Option<String>,
}

/// `GET /tags/search`: the filter popover's results, matching canonical
/// names and aliases. Each row toggles its tag in the current filter.
pub async fn search_tags(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(query): Query<SearchQuery>,
) -> axum::response::Response {
    let active = parse_tag_list(query.tags.as_deref());
    let q = query.q.unwrap_or_default();
    let rows = match load_rows(&state, user.id).await {
        Ok(rows) => rows,
        Err(e) => return error_response(e),
    };
    let options = search(&rows, &q, SEARCH_LIMIT)
        .into_iter()
        .map(|row| {
            let (aliases, more_aliases) = alias_preview(row, &q, ALIAS_PREVIEW);
            TagOption {
                name: row.name.clone(),
                count: row.count,
                aliases: aliases.join(", "),
                more_aliases,
                active: active.contains(&row.name),
                url: bookmarks_url(&toggled(&active, &row.name)),
            }
        })
        .collect();
    render(&SearchResults {
        options,
        query: q.trim().to_string(),
    })
}

async fn load_rows(state: &AppState, user_id: Uuid) -> Result<Vec<TagRow>, DomainError> {
    let (counts, aliases) = tokio::try_join!(
        state.tag_aliases.tag_counts(user_id),
        state.tag_aliases.list_aliases(user_id),
    )?;
    Ok(library_rows(&counts, &aliases))
}

// ---------------------------------------------------------------------------
// Library page
// ---------------------------------------------------------------------------

struct LibraryRow {
    name: String,
    count: i64,
    aliases: String,
    more_aliases: usize,
    filter_url: String,
}

struct PendingView {
    spellings: usize,
    groups: usize,
}

struct BatchView {
    id: Uuid,
    kind: &'static str,
    summary: String,
    when: String,
    bookmarks: String,
    undone: bool,
    can_undo: bool,
}

struct Flash {
    error: bool,
    message: String,
    undo: Option<Uuid>,
}

impl Flash {
    fn ok(message: String, undo: Option<Uuid>) -> Self {
        Self {
            error: false,
            message,
            undo,
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            error: true,
            message: message.into(),
            undo: None,
        }
    }

    fn from_error(e: DomainError) -> Self {
        match e {
            DomainError::InvalidInput(message) => Self::error(message),
            DomainError::NotFound => Self::error("That change no longer exists."),
            e => {
                tracing::error!("tag change failed: {e}");
                Self::error("Could not apply the change. Try again.")
            }
        }
    }
}

struct LibraryBody {
    q: String,
    sort: &'static str,
    tag_count: usize,
    alias_count: usize,
    rows: Vec<LibraryRow>,
    all_names: Vec<String>,
    pending: Option<PendingView>,
    batches: Vec<BatchView>,
    flash: Option<Flash>,
}

#[derive(Template)]
#[template(path = "tags/library.html")]
struct LibraryPage {
    user: Option<UserView>,
    header_shows_bookmark_actions: bool,
    body: LibraryBody,
}

#[derive(Template)]
#[template(path = "tags/library_body.html")]
struct LibraryBodyFragment {
    body: LibraryBody,
}

/// The library's filter and sort, sent with every request so a change
/// re-renders the same view.
#[derive(Deserialize, Default)]
pub struct LibraryView {
    q: Option<String>,
    sort: Option<String>,
}

fn kind_label(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Normalize => "Cleanup",
        ChangeKind::LlmMerge => "AI merge",
        ChangeKind::Rename => "Rename",
        ChangeKind::Merge => "Merge",
        ChangeKind::Delete => "Delete",
    }
}

fn plural(n: u64, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// "a, b → c; d → e and 2 more" for a batch's merges.
fn merges_summary(merges: &[TagMerge]) -> String {
    const SHOWN: usize = 3;
    let mut parts: Vec<String> = merges
        .iter()
        .take(SHOWN)
        .map(|m| {
            let sources: Vec<&str> = m
                .members
                .iter()
                .map(String::as_str)
                .filter(|s| *s != m.canonical)
                .collect();
            format!("{} → {}", sources.join(", "), m.canonical)
        })
        .collect();
    if merges.len() > SHOWN {
        parts.push(format!("and {} more", merges.len() - SHOWN));
    }
    parts.join("; ")
}

fn batch_view(batch: &TagChangeBatch) -> BatchView {
    let undo_cutoff = Utc::now() - Duration::days(UNDO_WINDOW_DAYS);
    BatchView {
        id: batch.id,
        kind: kind_label(batch.kind),
        summary: merges_summary(&batch.summary.merges),
        when: batch.created_at.format("%b %d, %Y %H:%M").to_string(),
        bookmarks: plural(batch.summary.bookmarks_changed, "bookmark"),
        undone: batch.undone_at.is_some(),
        can_undo: batch.undone_at.is_none() && batch.created_at > undo_cutoff,
    }
}

async fn library_body(
    state: &AppState,
    user_id: Uuid,
    view: &LibraryView,
    flash: Option<Flash>,
) -> Result<LibraryBody, DomainError> {
    let svc = &state.tag_aliases;
    let (mut rows, batches, pending) = tokio::try_join!(
        load_rows(state, user_id),
        svc.list_batches(user_id, HISTORY_LIMIT),
        svc.list_proposals(user_id, Some(ProposalStatus::Pending)),
    )?;
    let q = view.q.as_deref().unwrap_or_default().trim().to_string();
    let sort = TagSort::parse(view.sort.as_deref().unwrap_or_default());

    let tag_count = rows.len();
    let alias_count = rows.iter().map(|r| r.aliases.len()).sum();
    let mut all_names: Vec<String> = rows.iter().map(|r| r.name.clone()).collect();
    all_names.sort_by_key(|n| n.to_lowercase());
    sort_rows(&mut rows, sort);
    let rows = filter_rows(rows, &q)
        .into_iter()
        .map(|row| {
            let (aliases, more_aliases) = alias_preview(&row, &q, ALIAS_PREVIEW);
            LibraryRow {
                aliases: aliases.join(", "),
                more_aliases,
                filter_url: bookmarks_url(std::slice::from_ref(&row.name)),
                name: row.name,
                count: row.count,
            }
        })
        .collect();
    let pending = (!pending.is_empty()).then(|| PendingView {
        spellings: pending.iter().map(|p| p.members.len()).sum(),
        groups: pending.len(),
    });

    Ok(LibraryBody {
        q,
        sort: sort.as_str(),
        tag_count,
        alias_count,
        rows,
        all_names,
        pending,
        batches: batches.iter().map(batch_view).collect(),
        flash,
    })
}

async fn render_body(
    state: &AppState,
    user_id: Uuid,
    view: &LibraryView,
    flash: Option<Flash>,
) -> axum::response::Response {
    match library_body(state, user_id, view, flash).await {
        Ok(body) => render(&LibraryBodyFragment { body }),
        Err(e) => error_response(e),
    }
}

/// The normalization job runs lazily, once: on the first visit when the
/// user has no deterministic proposal of any status yet. It only stores
/// proposals; the review screen applies them.
async fn ensure_normalization_proposals(
    state: &AppState,
    user_id: Uuid,
) -> Result<(), DomainError> {
    let svc = &state.tag_aliases;
    let proposals = svc.list_proposals(user_id, None).await?;
    if !proposals
        .iter()
        .any(|p| p.source == ProposalSource::Deterministic)
    {
        svc.propose_normalization(user_id).await?;
    }
    Ok(())
}

/// `GET /tags`: the full page, or only the table and history for an HTMX
/// filter or sort change.
pub async fn library(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    Query(view): Query<LibraryView>,
) -> axum::response::Response {
    if is_htmx(&headers) {
        return render_body(&state, user.id, &view, None).await;
    }
    if let Err(e) = ensure_normalization_proposals(&state, user.id).await {
        // The page still works without proposals; the banner just stays hidden.
        tracing::error!("tag normalization proposals failed: {e}");
    }
    match library_body(&state, user.id, &view, None).await {
        Ok(body) => render(&LibraryPage {
            user: Some(user.into()),
            header_shows_bookmark_actions: false,
            body,
        }),
        Err(e) => error_response(e),
    }
}

/// The rename/merge dialog's fields. `source` repeats (bulk merge), which
/// `axum::Form` cannot collect, so the body is parsed here.
struct MergeForm {
    kind: String,
    sources: Vec<String>,
    target: String,
    view: LibraryView,
}

impl MergeForm {
    fn parse(body: &[u8]) -> Self {
        let mut form = Self {
            kind: String::new(),
            sources: Vec::new(),
            target: String::new(),
            view: LibraryView::default(),
        };
        for (key, value) in url::form_urlencoded::parse(body) {
            let value = value.into_owned();
            match key.as_ref() {
                "kind" => form.kind = value,
                "source" => form.sources.push(value),
                "target" => form.target = value,
                "q" => form.view.q = Some(value),
                "sort" => form.view.sort = Some(value),
                _ => {}
            }
        }
        form
    }

    /// The merge to apply, or the message to show instead.
    fn merge(&self) -> Result<(ChangeKind, TagMerge), String> {
        let kind = match self.kind.as_str() {
            "rename" => ChangeKind::Rename,
            "merge" => ChangeKind::Merge,
            _ => return Err("Unknown tag change.".into()),
        };
        let target = self.target.trim();
        if target.is_empty() {
            return Err("Enter a tag name.".into());
        }
        if kind == ChangeKind::Rename && self.sources.len() != 1 {
            return Err("Rename one tag at a time.".into());
        }
        let members: Vec<String> = self
            .sources
            .iter()
            .filter(|s| *s != target)
            .cloned()
            .collect();
        if members.is_empty() {
            return Err(match kind {
                ChangeKind::Rename => "That is already the tag's name.".into(),
                _ => "Choose tags to merge into a different tag.".into(),
            });
        }
        Ok((
            kind,
            TagMerge {
                canonical: target.to_string(),
                members,
            },
        ))
    }
}

fn quoted_list(names: &[String]) -> String {
    names
        .iter()
        .map(|n| format!("“{n}”"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `POST /tags/merge`: rename one tag or merge tags into another, as one
/// undoable batch. Returns the re-rendered body with the outcome.
pub async fn merge(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    body: Bytes,
) -> axum::response::Response {
    let form = MergeForm::parse(&body);
    let flash = match form.merge() {
        Err(message) => Flash::error(message),
        Ok((kind, merge)) => {
            let what = match kind {
                ChangeKind::Rename => format!(
                    "Renamed {} to “{}”",
                    quoted_list(&merge.members),
                    merge.canonical
                ),
                _ => format!(
                    "Merged {} into “{}”",
                    quoted_list(&merge.members),
                    merge.canonical
                ),
            };
            match state
                .tag_aliases
                .merge_tags(user.id, kind, std::slice::from_ref(&merge))
                .await
            {
                Ok(Some(batch)) => Flash::ok(
                    format!(
                        "{what} on {}.",
                        plural(batch.summary.bookmarks_changed, "bookmark")
                    ),
                    Some(batch.id),
                ),
                Ok(None) => Flash::ok("Nothing to change.".into(), None),
                Err(e) => Flash::from_error(e),
            }
        }
    };
    render_body(&state, user.id, &form.view, Some(flash)).await
}

/// `POST /tags/batches/{id}/undo`: undo one batch as a unit.
pub async fn undo(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(batch_id): Path<Uuid>,
    Form(view): Form<LibraryView>,
) -> axum::response::Response {
    let flash = match state.tag_aliases.undo_batch(user.id, batch_id).await {
        Ok(batch) => Flash::ok(
            format!(
                "Undid {}: {}.",
                kind_label(batch.kind).to_lowercase(),
                merges_summary(&batch.summary.merges)
            ),
            None,
        ),
        Err(e) => Flash::from_error(e),
    };
    render_body(&state, user.id, &view, Some(flash)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_string()).collect()
    }

    #[test]
    fn tag_lists_parse_and_toggle() {
        assert_eq!(parse_tag_list(Some(" rust, ,go ")), s(&["rust", "go"]));
        assert!(parse_tag_list(None).is_empty());
        assert_eq!(toggled(&s(&["rust", "go"]), "rust"), s(&["go"]));
        assert_eq!(toggled(&s(&["rust"]), "c++"), s(&["rust", "c++"]));
        assert_eq!(
            bookmarks_url(&s(&["rust", "c++", "UI/UX"])),
            "/bookmarks?tags=rust%2Cc%2B%2B%2CUI%2FUX"
        );
    }

    #[test]
    fn filter_bar_shows_active_tags_first_and_counts_the_rest() {
        let counts: Vec<(String, i64)> = (0..15)
            .map(|i| (format!("tag{i:02}"), 100 - i as i64))
            .collect();
        let bar = FilterBar::new(&counts, &s(&["tag03", "gone"]));
        let active: Vec<(&str, Option<i64>)> = bar
            .active
            .iter()
            .map(|c| (c.name.as_str(), c.count))
            .collect();
        assert_eq!(active, vec![("tag03", Some(97)), ("gone", None)]);
        assert_eq!(bar.active[0].url, "/bookmarks?tags=gone");
        assert_eq!(bar.top.len(), TOP_TAGS);
        assert!(bar.top.iter().all(|c| c.name != "tag03"));
        assert_eq!(bar.top[0].url, "/bookmarks?tags=tag03%2Cgone%2Ctag00");
        assert_eq!(bar.more_count, 15 - TOP_TAGS - 1);
        assert_eq!(bar.tags_value, "tag03,gone");
    }

    #[test]
    fn merge_form_collects_repeated_sources_and_validates() {
        let form =
            MergeForm::parse(b"kind=merge&source=k8s&source=K8s&target=+kubernetes+&q=k&sort=az");
        assert_eq!(form.view.q.as_deref(), Some("k"));
        assert_eq!(form.view.sort.as_deref(), Some("az"));
        let (kind, merge) = form.merge().unwrap();
        assert_eq!(kind, ChangeKind::Merge);
        assert_eq!(merge.canonical, "kubernetes");
        assert_eq!(merge.members, s(&["k8s", "K8s"]));

        let into_itself = MergeForm::parse(b"kind=merge&source=rust&target=rust");
        assert!(into_itself.merge().is_err());
        let rename_two = MergeForm::parse(b"kind=rename&source=a&source=b&target=c");
        assert!(rename_two.merge().is_err());
        let blank = MergeForm::parse(b"kind=rename&source=a&target=+");
        assert!(blank.merge().is_err());
        let rename = MergeForm::parse(b"kind=rename&source=github&target=GitHub");
        assert_eq!(rename.merge().unwrap().0, ChangeKind::Rename);
    }

    #[test]
    fn merges_summary_names_sources_and_targets() {
        let merges = vec![
            TagMerge {
                canonical: "kubernetes".into(),
                members: s(&["kubernetes", "k8s"]),
            },
            TagMerge {
                canonical: "GitHub".into(),
                members: s(&["github", "git hub"]),
            },
        ];
        assert_eq!(
            merges_summary(&merges),
            "k8s → kubernetes; github, git hub → GitHub"
        );
        let many: Vec<TagMerge> = (0..5)
            .map(|i| TagMerge {
                canonical: format!("t{i}"),
                members: vec![format!("s{i}")],
            })
            .collect();
        assert_eq!(
            merges_summary(&many),
            "s0 → t0; s1 → t1; s2 → t2; and 2 more"
        );
    }
}
