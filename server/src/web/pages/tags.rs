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
use crate::domain::tag_alias::{ChangeKind, ProposalStatus, TagAlias, TagChangeBatch, TagMerge};
use crate::domain::tag_library::{
    ManualMerge, ManualMergePlan, TagRow, TagSort, alias_preview, filter_rows, library_rows,
    plan_manual_merge, search, sort_rows, top_tags,
};
use crate::domain::tag_normalize::clean_tag;
use crate::web::extractors::AuthUser;
use crate::web::middleware::auth::is_htmx;
use crate::web::pages::shared::UserView;
use crate::web::state::AppState;

/// Tags shown as chips in the filter bar (spec: 10 on desktop). The
/// template hides all but the first four on narrow screens.
const TOP_TAGS: usize = 10;
const SEARCH_LIMIT: usize = 20;
const ALIAS_PREVIEW: usize = 3;
/// The first page of batches; [`recent_batches`] fetches more as needed.
const HISTORY_PAGE: i64 = 50;
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

/// Parse the comma-separated `tags` filter of `/bookmarks`, without blanks
/// or repeats.
pub(crate) fn parse_tag_list(tags: Option<&str>) -> Vec<String> {
    let mut list: Vec<String> = Vec::new();
    for tag in tags.unwrap_or_default().split(',').map(str::trim) {
        if !tag.is_empty() && !list.iter().any(|t| t == tag) {
            list.push(tag.to_string());
        }
    }
    list
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
            more_count: rows.len().saturating_sub(top.len() + active_used),
            top,
            tags_value: active.join(","),
        }
    }
}

struct TagOption {
    name: String,
    count: i64,
    aliases: String,
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
        .map(|row| TagOption {
            name: row.name.clone(),
            count: row.count,
            aliases: alias_preview(row, &q, ALIAS_PREVIEW),
            active: active.contains(&row.name),
            url: bookmarks_url(&toggled(&active, &row.name)),
        })
        .collect();
    render(&SearchResults {
        options,
        query: q.trim().to_string(),
    })
}

async fn load_tags(
    state: &AppState,
    user_id: Uuid,
) -> Result<(Vec<(String, i64)>, Vec<TagAlias>), DomainError> {
    tokio::try_join!(
        state.tag_aliases.tag_counts(user_id),
        state.tag_aliases.list_aliases(user_id),
    )
}

async fn load_rows(state: &AppState, user_id: Uuid) -> Result<Vec<TagRow>, DomainError> {
    let (counts, aliases) = load_tags(state, user_id).await?;
    Ok(library_rows(&counts, &aliases))
}

// ---------------------------------------------------------------------------
// Library page
// ---------------------------------------------------------------------------

struct LibraryRow {
    name: String,
    count: i64,
    aliases: String,
    filter_url: String,
}

struct BatchView {
    id: Uuid,
    kind: &'static str,
    summary: String,
    when: String,
    bookmarks: String,
    undone: bool,
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

/// Which responses carry the `<datalist>` of tag names for the merge
/// dialog. It only changes after a merge or undo, so filter and sort
/// requests leave it out.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TagNames {
    /// Filter or sort: the page's list is still current.
    Skip,
    /// Full page: `library.html` renders it.
    Page,
    /// After a change: the fragment replaces it out of band.
    Refresh,
}

struct LibraryBody {
    q: String,
    sort: &'static str,
    summary: String,
    rows: Vec<LibraryRow>,
    all_names: Vec<String>,
    refresh_names: bool,
    pending: Option<String>,
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

fn plural(n: usize, word: &str) -> String {
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

fn bookmarks_changed(batch: &TagChangeBatch) -> String {
    plural(batch.summary.bookmarks_changed as usize, "bookmark")
}

fn batch_view(batch: &TagChangeBatch) -> BatchView {
    BatchView {
        id: batch.id,
        kind: kind_label(batch.kind),
        summary: merges_summary(&batch.summary.merges),
        when: batch.created_at.format("%b %d, %Y %H:%M").to_string(),
        bookmarks: bookmarks_changed(batch),
        undone: batch.undone_at.is_some(),
    }
}

/// Every batch inside the undo window, newest first. The repository lists
/// the newest N, so ask for more until the oldest returned is outside the
/// window or there are no more.
async fn recent_batches(
    state: &AppState,
    user_id: Uuid,
) -> Result<Vec<TagChangeBatch>, DomainError> {
    let cutoff = Utc::now() - Duration::days(UNDO_WINDOW_DAYS);
    let mut limit = HISTORY_PAGE;
    loop {
        let batches = state.tag_aliases.list_batches(user_id, limit).await?;
        let complete =
            (batches.len() as i64) < limit || batches.last().is_some_and(|b| b.created_at < cutoff);
        if complete {
            return Ok(batches
                .into_iter()
                .filter(|b| b.created_at >= cutoff)
                .collect());
        }
        limit *= 4;
    }
}

async fn library_body(
    state: &AppState,
    user_id: Uuid,
    view: &LibraryView,
    names: TagNames,
    flash: Option<Flash>,
) -> Result<LibraryBody, DomainError> {
    let (rows, batches, pending) = tokio::try_join!(
        load_rows(state, user_id),
        recent_batches(state, user_id),
        state
            .tag_aliases
            .list_proposals(user_id, Some(ProposalStatus::Pending)),
    )?;
    let q = view.q.as_deref().unwrap_or_default().trim().to_string();
    let sort = TagSort::parse(view.sort.as_deref().unwrap_or_default());

    let alias_count = rows.iter().map(|r| r.aliases.len()).sum();
    let summary = format!(
        "{} · {} folded in as aliases",
        plural(rows.len(), "tag"),
        plural(alias_count, "spelling")
    );
    let mut all_names: Vec<String> = Vec::new();
    if names != TagNames::Skip {
        all_names = rows.iter().map(|r| r.name.clone()).collect();
        all_names.sort_by_cached_key(|n| n.to_lowercase());
    }
    let mut rows = filter_rows(rows, &q);
    sort_rows(&mut rows, sort);
    let rows = rows
        .into_iter()
        .map(|row| LibraryRow {
            aliases: alias_preview(&row, &q, ALIAS_PREVIEW),
            filter_url: bookmarks_url(std::slice::from_ref(&row.name)),
            name: row.name,
            count: row.count,
        })
        .collect();
    let pending = (!pending.is_empty()).then(|| {
        format!(
            "{} can merge into {}",
            plural(pending.iter().map(|p| p.members.len()).sum(), "spelling"),
            plural(pending.len(), "tag")
        )
    });

    Ok(LibraryBody {
        q,
        sort: sort.as_str(),
        summary,
        rows,
        all_names,
        refresh_names: names == TagNames::Refresh,
        pending,
        batches: batches.iter().map(batch_view).collect(),
        flash,
    })
}

async fn render_body(
    state: &AppState,
    user_id: Uuid,
    view: &LibraryView,
    names: TagNames,
    flash: Option<Flash>,
) -> axum::response::Response {
    match library_body(state, user_id, view, names, flash).await {
        Ok(body) => render(&LibraryBodyFragment { body }),
        Err(e) => error_response(e),
    }
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
        return render_body(&state, user.id, &view, TagNames::Skip, None).await;
    }
    if let Err(e) = state
        .tag_aliases
        .ensure_normalization_proposals(user.id)
        .await
    {
        // The page still works without proposals; the banner just stays hidden.
        tracing::error!("tag normalization proposals failed: {e}");
    }
    match library_body(&state, user.id, &view, TagNames::Page, None).await {
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

    /// The change as typed, checked for shape only; the repository checks it
    /// against the library under the lock ([`plan_manual_merge`]).
    fn request(&self) -> Result<ManualMerge, String> {
        let kind = match self.kind.as_str() {
            "rename" => ChangeKind::Rename,
            "merge" => ChangeKind::Merge,
            _ => return Err("Unknown tag change.".into()),
        };
        if self.target.trim().is_empty() {
            return Err("Enter a tag name.".into());
        }
        // The write path's rule (trimmed, no control characters), plus no
        // commas: tags are entered and filtered as comma-separated lists.
        let target = clean_tag(&self.target)
            .filter(|t| !t.contains(','))
            .ok_or("A tag name cannot contain commas or control characters.")?;
        if self.sources.is_empty() {
            return Err("Choose tags to merge.".into());
        }
        if kind == ChangeKind::Rename && self.sources.len() != 1 {
            return Err("Rename one tag at a time.".into());
        }
        Ok(ManualMerge {
            kind,
            target: target.to_string(),
            sources: self.sources.clone(),
        })
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
    let flash = apply_merge_form(&state, user.id, &form)
        .await
        .unwrap_or_else(|flash| flash);
    render_body(&state, user.id, &form.view, TagNames::Refresh, Some(flash)).await
}

async fn apply_merge_form(
    state: &AppState,
    user_id: Uuid,
    form: &MergeForm,
) -> Result<Flash, Flash> {
    let request = form.request().map_err(Flash::error)?;
    let (plan, batch) = state
        .tag_aliases
        .merge_tags(user_id, &request)
        .await
        .map_err(Flash::from_error)?;
    let Some(batch) = batch else {
        return Ok(Flash::ok("Nothing to change.".into(), None));
    };
    let ManualMergePlan {
        kind,
        merge,
        redirected_from,
    } = plan;
    let what = match (kind, redirected_from) {
        (ChangeKind::Rename, _) => format!(
            "Renamed {} to “{}”",
            quoted_list(&merge.members),
            merge.canonical
        ),
        (_, Some(typed)) => format!(
            "“{typed}” is another spelling of “{}”, so merged {} into it",
            merge.canonical,
            quoted_list(&merge.members)
        ),
        (_, None) => format!(
            "Merged {} into “{}”",
            quoted_list(&merge.members),
            merge.canonical
        ),
    };
    Ok(Flash::ok(
        format!("{what} on {}.", bookmarks_changed(&batch)),
        Some(batch.id),
    ))
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
    render_body(&state, user.id, &view, TagNames::Refresh, Some(flash)).await
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
        assert_eq!(parse_tag_list(Some("rust,rust,go")), s(&["rust", "go"]));
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

        let one = FilterBar::new(
            &[("rust".to_string(), 1)],
            &parse_tag_list(Some("rust,rust")),
        );
        assert_eq!(one.more_count, 0);
    }

    fn alias(spelling: &str, canonical: &str) -> TagAlias {
        TagAlias {
            alias_key: crate::domain::tag_normalize::compact_key(spelling),
            alias: spelling.into(),
            canonical: canonical.into(),
            source: crate::domain::tag_alias::AliasSource::Manual,
            batch_id: None,
        }
    }

    /// `form` planned against a library of javascript (alias js),
    /// kubernetes (alias k8s), python, rust and a legacy `K8S` tag.
    fn plan(form: &[u8]) -> Result<(ChangeKind, TagMerge, Option<String>), String> {
        let aliases = vec![
            alias("javascript", "javascript"),
            alias("js", "javascript"),
            alias("kubernetes", "kubernetes"),
            alias("k8s", "kubernetes"),
        ];
        let counts: Vec<(String, i64)> = [("javascript", 5), ("kubernetes", 3), ("python", 4)]
            .iter()
            .map(|(t, c)| ((*t).to_string(), *c))
            .chain([("rust".to_string(), 2), ("K8S".to_string(), 1)])
            .collect();
        let rows = library_rows(&counts, &aliases);
        let plan = plan_manual_merge(&MergeForm::parse(form).request()?, &rows, &aliases)
            .map_err(|e| e.to_string())?;
        Ok((plan.kind, plan.merge, plan.redirected_from))
    }

    #[test]
    fn merge_form_collects_repeated_sources_and_validates() {
        let form = MergeForm::parse(
            b"kind=merge&source=python&source=rust&target=+scripting+&q=k&sort=az",
        );
        assert_eq!(form.view.q.as_deref(), Some("k"));
        assert_eq!(form.view.sort.as_deref(), Some("az"));
        let (kind, merge, redirected) =
            plan(b"kind=merge&source=python&source=rust&target=+scripting+").unwrap();
        assert_eq!(kind, ChangeKind::Merge);
        assert_eq!(merge.canonical, "scripting");
        assert_eq!(merge.members, s(&["python", "rust"]));
        assert_eq!(redirected, None);

        assert!(plan(b"kind=merge&source=rust&target=rust").is_err());
        assert!(plan(b"kind=rename&source=python&source=rust&target=c").is_err());
        assert!(plan(b"kind=rename&source=rust&target=+").is_err());
        assert!(plan(b"kind=merge&target=rust").is_err());
        let rename = plan(b"kind=rename&source=rust&target=Rust").unwrap();
        assert_eq!(rename.0, ChangeKind::Rename);
        assert_eq!(rename.1.canonical, "Rust");
    }

    #[test]
    fn targets_with_commas_or_control_characters_are_refused() {
        let error = plan(b"kind=rename&source=rust&target=research%2Cnotes").unwrap_err();
        assert!(error.contains("commas"), "{error}");
        assert!(plan(b"kind=merge&source=rust&target=a%0Ab").is_err());
    }

    #[test]
    fn a_target_that_is_another_tags_alias_merges_into_that_tag() {
        // js -> javascript: renaming python to JS must not take js away.
        let (kind, merge, redirected) = plan(b"kind=rename&source=python&target=JS").unwrap();
        assert_eq!(kind, ChangeKind::Merge);
        assert_eq!(merge.canonical, "javascript");
        assert_eq!(merge.members, s(&["python"]));
        assert_eq!(redirected.as_deref(), Some("JS"));

        let (_, merge, _) = plan(b"kind=merge&source=rust&target=k8s").unwrap();
        assert_eq!(merge.canonical, "kubernetes");

        // A legacy spelling of an aliased key cannot be moved elsewhere.
        let error = plan(b"kind=merge&source=K8S&target=rust").unwrap_err();
        assert!(error.contains("spelling of “kubernetes”"), "{error}");
    }

    #[test]
    fn a_stale_page_is_refused() {
        // `k8s` is an alias now, no longer a tag: the page is out of date.
        let error = plan(b"kind=rename&source=k8s&target=kube").unwrap_err();
        assert!(error.contains("Reload"), "{error}");
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
