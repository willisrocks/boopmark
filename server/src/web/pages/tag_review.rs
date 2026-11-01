//! `/tags/review`: the review screen for merge proposals (tag spec §3).
//!
//! Proposals come from the spelling-variant job and the background LLM
//! run. Nothing is applied until the user submits the rows they checked;
//! the result is one batch with Undo.

use askama::Template;
use axum::Form;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use chrono::Utc;
use serde::Deserialize;
use std::collections::HashMap;
use uuid::Uuid;

use crate::app::tag_consolidation::StartOutcome;
use crate::domain::error::DomainError;
use crate::domain::tag_alias::{ChangeKind, Confidence, ProposalSource, TagChangeBatch};
use crate::domain::tag_consolidation::{
    ConsolidationJob, JobStatus, ReviewChip, ReviewRow, ReviewedRow,
};
use crate::web::extractors::AuthUser;
use crate::web::pages::shared::UserView;
use crate::web::state::AppState;

const PAGE: &str = "/tags/review";
/// Form fields carrying an edited merge target are named `canonical:<id>`.
const CANONICAL_FIELD: &str = "canonical:";
/// Hidden fields named `shown:<id>` carry the JSON list of spellings a row
/// showed, so the apply merges exactly those.
const SHOWN_FIELD: &str = "shown:";

struct RowView {
    id: String,
    canonical: String,
    chips: Vec<ReviewChip>,
    /// The chips as a JSON list, posted back with the row.
    shown: String,
    bookmarks: i64,
    sample_title: Option<String>,
    confidence: &'static str,
    source: &'static str,
    prechecked: bool,
}

impl From<ReviewRow> for RowView {
    fn from(row: ReviewRow) -> Self {
        Self {
            id: row.id.to_string(),
            canonical: row.canonical,
            shown: serde_json::to_string(
                &row.chips.iter().map(|c| c.tag.as_str()).collect::<Vec<_>>(),
            )
            .unwrap_or_default(),
            chips: row.chips,
            bookmarks: row.bookmarks,
            sample_title: row.sample_title,
            confidence: row.confidence.as_str(),
            source: match row.source {
                ProposalSource::Llm => "AI",
                ProposalSource::Deterministic => "Spelling",
            },
            // Only `high` rows start checked; the rest wait under "Check these".
            prechecked: row.confidence == Confidence::High,
        }
    }
}

struct BatchView {
    id: String,
    label: String,
    merges: usize,
    bookmarks: u64,
    when: String,
    undone: bool,
}

impl From<TagChangeBatch> for BatchView {
    fn from(batch: TagChangeBatch) -> Self {
        Self {
            id: batch.id.to_string(),
            label: match batch.kind {
                ChangeKind::LlmMerge => "AI and spelling merges",
                _ => "Spelling merges",
            }
            .to_string(),
            merges: batch.summary.merges.len(),
            bookmarks: batch.summary.bookmarks_changed,
            when: batch.created_at.format("%b %d, %H:%M UTC").to_string(),
            undone: batch.undone_at.is_some(),
        }
    }
}

/// The job line under the actions. `polling` keeps the fragment asking
/// for updates while a run is going.
struct StatusView {
    state: &'static str,
    message: String,
    polling: bool,
}

fn status_view(job: Option<&ConsolidationJob>) -> Option<StatusView> {
    let job = job?;
    Some(match job.status {
        JobStatus::Running => StatusView {
            state: "running",
            message: "Looking for duplicate tags with AI. This usually takes about a minute; \
                      you can leave this page."
                .into(),
            polling: true,
        },
        JobStatus::Failed => StatusView {
            state: "failed",
            message: job
                .error
                .clone()
                .unwrap_or_else(|| "The last run failed. Try again.".into()),
            polling: false,
        },
        JobStatus::Succeeded => StatusView {
            state: "succeeded",
            message: format!(
                "The last AI run found {} suggestion{} after checking {} pair{} ({}).",
                job.proposals,
                if job.proposals == 1 { "" } else { "s" },
                job.pairs_checked,
                if job.pairs_checked == 1 { "" } else { "s" },
                ago(job.finished_at.unwrap_or(job.started_at)),
            ),
            polling: false,
        },
    })
}

fn ago(at: chrono::DateTime<Utc>) -> String {
    let minutes = (Utc::now() - at).num_minutes();
    match minutes {
        ..1 => "just now".into(),
        1 => "1 minute ago".into(),
        2..60 => format!("{minutes} minutes ago"),
        _ => at.format("%b %d, %H:%M UTC").to_string(),
    }
}

#[derive(Template)]
#[template(path = "tags/review.html")]
struct ReviewPage {
    user: Option<UserView>,
    header_shows_bookmark_actions: bool,
    check_rows: Vec<RowView>,
    high_rows: Vec<RowView>,
    batches: Vec<BatchView>,
    status: Option<StatusView>,
    has_provider: bool,
    notice: Option<String>,
    error: Option<String>,
    applied_batch: Option<String>,
}

#[derive(Template)]
#[template(path = "tags/review_status.html")]
struct StatusFragment {
    status: Option<StatusView>,
}

#[derive(Deserialize, Default)]
struct ReviewQuery {
    notice: Option<String>,
    error: Option<String>,
    batch: Option<Uuid>,
}

fn render(t: &impl Template) -> Response {
    match t.render() {
        Ok(body) => Html(body).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Back to the review page with a message. User-facing text only.
fn back(key: &str, message: &str, batch: Option<Uuid>) -> Response {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair(key, message);
    if let Some(batch) = batch {
        query.append_pair("batch", &batch.to_string());
    }
    let query = query.finish();
    Redirect::to(&format!("{PAGE}?{query}")).into_response()
}

fn back_with_error(error: DomainError) -> Response {
    let message = match error {
        DomainError::InvalidInput(message) => message,
        DomainError::NotFound => "That change no longer exists. Reload and try again.".into(),
        other => {
            tracing::warn!(error = %other, "tag review action failed");
            "Something went wrong. Try again.".into()
        }
    };
    back("error", &message, None)
}

async fn review_page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(query): Query<ReviewQuery>,
) -> Response {
    let review = match state.tag_consolidation.review(user.id).await {
        Ok(review) => review,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load the tag review page");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let has_provider = matches!(
        state.settings.get_text_provider_credentials(user.id).await,
        Ok(Some(_))
    );
    let (high_rows, check_rows): (Vec<ReviewRow>, Vec<ReviewRow>) = review
        .rows
        .into_iter()
        .partition(|row| row.confidence == Confidence::High);
    render(&ReviewPage {
        user: Some(user.into()),
        header_shows_bookmark_actions: false,
        check_rows: check_rows.into_iter().map(RowView::from).collect(),
        high_rows: high_rows.into_iter().map(RowView::from).collect(),
        batches: review.batches.into_iter().map(BatchView::from).collect(),
        status: status_view(review.job.as_ref()),
        has_provider,
        notice: query.notice,
        error: query.error,
        applied_batch: query.batch.map(|b| b.to_string()),
    })
}

async fn status(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
) -> Response {
    let job = match state.tag_consolidation.job(user.id).await {
        Ok(job) => job,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load the tag consolidation status");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let status = status_view(job.as_ref());
    // A poll that sees the run end reloads the page to show its rows.
    if headers.contains_key("hx-request") && !status.as_ref().is_some_and(|s| s.polling) {
        return ([("HX-Refresh", "true")], StatusCode::OK).into_response();
    }
    render(&StatusFragment { status })
}

async fn run(State(state): State<AppState>, AuthUser(user): AuthUser) -> Response {
    match state.tag_consolidation.start(user.id).await {
        Ok(StartOutcome::Started) => Redirect::to(PAGE).into_response(),
        Ok(StartOutcome::AlreadyRunning) => back("notice", "A run is already going.", None),
        Err(e) => back_with_error(e),
    }
}

async fn normalize(State(state): State<AppState>, AuthUser(user): AuthUser) -> Response {
    match state.tag_consolidation.propose_normalization(user.id).await {
        Ok(found) if found.is_empty() => back("notice", "No spelling variants found.", None),
        Ok(found) => back(
            "notice",
            &format!(
                "Found {} group{} of spelling variants.",
                found.len(),
                if found.len() == 1 { "" } else { "s" }
            ),
            None,
        ),
        Err(e) => back_with_error(e),
    }
}

/// The checked rows of a review form, each with the spellings it showed
/// and its submitted target. A row whose spellings are missing or garbled
/// shows nothing, which the apply refuses as a changed list.
fn parse_selection(fields: Vec<(String, String)>) -> Vec<ReviewedRow> {
    let mut ids: Vec<Uuid> = Vec::new();
    let mut shown: HashMap<Uuid, Vec<String>> = HashMap::new();
    let mut canonicals: HashMap<Uuid, String> = HashMap::new();
    let field_id = |name: &str, prefix: &str| {
        name.strip_prefix(prefix)
            .and_then(|id| Uuid::parse_str(id).ok())
    };
    for (name, value) in fields {
        if name == "ids" {
            if let Ok(id) = Uuid::parse_str(&value)
                && !ids.contains(&id)
            {
                ids.push(id);
            }
        } else if let Some(id) = field_id(&name, SHOWN_FIELD) {
            shown.insert(id, serde_json::from_str(&value).unwrap_or_default());
        } else if let Some(id) = field_id(&name, CANONICAL_FIELD) {
            canonicals.insert(id, value);
        }
    }
    ids.into_iter()
        .map(|id| ReviewedRow {
            id,
            shown: shown.remove(&id).unwrap_or_default(),
            canonical: canonicals.remove(&id),
        })
        .collect()
}

async fn apply(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Form(fields): Form<Vec<(String, String)>>,
) -> Response {
    let rows = parse_selection(fields);
    match state.tag_consolidation.apply(user.id, &rows).await {
        Ok(batch) => {
            let merges = batch.summary.merges.len();
            back(
                "notice",
                &format!(
                    "Applied {merges} merge{} to {} bookmark{}.",
                    if merges == 1 { "" } else { "s" },
                    batch.summary.bookmarks_changed,
                    if batch.summary.bookmarks_changed == 1 {
                        ""
                    } else {
                        "s"
                    },
                ),
                Some(batch.id),
            )
        }
        Err(e) => back_with_error(e),
    }
}

async fn dismiss(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Form(fields): Form<Vec<(String, String)>>,
) -> Response {
    let ids: Vec<Uuid> = parse_selection(fields).iter().map(|r| r.id).collect();
    match state.tag_consolidation.dismiss(user.id, &ids).await {
        Ok(0) => back("error", "Check the suggestions to dismiss.", None),
        Ok(n) => back(
            "notice",
            &format!("Dismissed {n} suggestion{}.", if n == 1 { "" } else { "s" }),
            None,
        ),
        Err(e) => back_with_error(e),
    }
}

async fn undo(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(batch_id): Path<Uuid>,
) -> Response {
    match state.tag_consolidation.undo(user.id, batch_id).await {
        Ok(_) => back("notice", "Undone. The merged tags are back.", None),
        Err(e) => back_with_error(e),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(PAGE, get(review_page))
        .route("/tags/review/status", get(status))
        .route("/tags/review/run", post(run))
        .route("/tags/review/normalize", post(normalize))
        .route("/tags/review/apply", post(apply))
        .route("/tags/review/dismiss", post(dismiss))
        .route("/tags/review/undo/{batch_id}", post(undo))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_reads_checked_rows_with_their_shown_spellings_and_targets() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let rows = parse_selection(vec![
            ("ids".into(), a.to_string()),
            ("ids".into(), a.to_string()),
            ("ids".into(), "not-a-uuid".into()),
            ("ids".into(), c.to_string()),
            (format!("shown:{a}"), r#"["js","javascript"]"#.into()),
            (format!("canonical:{a}"), "JavaScript".into()),
            // Unchecked rows are ignored.
            (format!("shown:{b}"), r#"["rust"]"#.into()),
            (format!("canonical:{b}"), "rust".into()),
            // Garbled spellings show nothing, which the apply refuses.
            (format!("shown:{c}"), "not json".into()),
        ]);
        assert_eq!(
            rows,
            vec![
                ReviewedRow {
                    id: a,
                    shown: vec!["js".into(), "javascript".into()],
                    canonical: Some("JavaScript".into()),
                },
                ReviewedRow {
                    id: c,
                    shown: vec![],
                    canonical: None,
                },
            ]
        );
    }

    #[test]
    fn a_running_job_polls_and_a_finished_one_does_not() {
        let job = ConsolidationJob {
            run_id: Uuid::nil(),
            status: JobStatus::Running,
            started_at: Utc::now(),
            finished_at: None,
            error: None,
            proposals: 0,
            pairs_checked: 0,
            input_tokens: 0,
            output_tokens: 0,
        };
        assert!(status_view(Some(&job)).unwrap().polling);
        let done = ConsolidationJob {
            status: JobStatus::Succeeded,
            proposals: 1,
            pairs_checked: 12,
            finished_at: Some(Utc::now()),
            ..job
        };
        let view = status_view(Some(&done)).unwrap();
        assert!(!view.polling);
        assert_eq!(
            view.message,
            "The last AI run found 1 suggestion after checking 12 pairs (just now)."
        );
        assert!(status_view(None).is_none());
    }
}
