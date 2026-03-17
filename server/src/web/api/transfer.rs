use axum::body::Body;
use axum::extract::{Multipart, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

use crate::domain::bookmark::Bookmark;
use crate::domain::transfer::{ExportMode, ImportMode, ImportRecord, ImportStrategy};
use crate::web::extractors::AuthUser;
use crate::web::state::{AppState, Bookmarks};

use axum::Json;
use crate::domain::error::DomainError;
use serde::Serialize;

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

fn error_response(err: DomainError) -> impl IntoResponse {
    let (status, message) = match &err {
        DomainError::NotFound => (StatusCode::NOT_FOUND, "not found"),
        DomainError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
        DomainError::AlreadyExists => (StatusCode::CONFLICT, "already exists"),
        DomainError::InvalidInput(_) => (StatusCode::BAD_REQUEST, "invalid input"),
        DomainError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
    };
    (status, Json(ErrorBody { error: message.to_string() }))
}

macro_rules! with_bookmarks {
    ($bookmarks:expr, $svc:ident => $body:expr) => {
        match $bookmarks {
            Bookmarks::Local($svc) => $body,
            Bookmarks::S3($svc) => $body,
        }
    };
}

// --- Query params ---

#[derive(Debug, Default, Deserialize)]
pub struct ExportParams {
    #[serde(default)]
    pub format: ExportFormat,
    #[serde(default)]
    pub mode: ExportMode,
}

#[derive(Debug, Default, Deserialize)]
pub struct ImportParams {
    #[serde(default)]
    pub format: ImportFormat,
    #[serde(default)]
    pub mode: ImportMode,
    #[serde(default)]
    pub strategy: ImportStrategy,
}

#[derive(Debug, Default, Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    #[default]
    Jsonl,
    Csv,
}

#[derive(Debug, Default, Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum ImportFormat {
    #[default]
    Jsonl,
    Csv,
}

// --- JSONL helpers ---

fn bookmarks_to_jsonl_export(bookmarks: &[Bookmark]) -> String {
    bookmarks
        .iter()
        .map(|b| {
            serde_json::json!({
                "url": b.url,
                "title": b.title,
                "description": b.description,
                "tags": b.tags,
            })
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn bookmarks_to_jsonl_backup(bookmarks: &[Bookmark]) -> String {
    bookmarks
        .iter()
        .map(|b| {
            serde_json::json!({
                "id": b.id,
                "url": b.url,
                "title": b.title,
                "description": b.description,
                "image_url": b.image_url,
                "domain": b.domain,
                "tags": b.tags,
                "created_at": b.created_at,
                "updated_at": b.updated_at,
            })
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_jsonl(text: &str) -> Result<Vec<ImportRecord>, String> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .enumerate()
        .map(|(i, line)| {
            serde_json::from_str::<serde_json::Value>(line)
                .map_err(|e| format!("line {}: {e}", i + 1))
                .and_then(|v| {
                    Ok(ImportRecord {
                        url: v["url"]
                            .as_str()
                            .ok_or_else(|| format!("line {}: missing url", i + 1))?
                            .to_string(),
                        title: v["title"].as_str().map(str::to_string),
                        description: v["description"].as_str().map(str::to_string),
                        tags: v["tags"]
                            .as_array()
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|t| t.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        id: v["id"].as_str().and_then(|s| s.parse::<Uuid>().ok()),
                        image_url: v["image_url"].as_str().map(str::to_string),
                        domain: v["domain"].as_str().map(str::to_string),
                        created_at: v["created_at"]
                            .as_str()
                            .and_then(|s| s.parse::<DateTime<Utc>>().ok()),
                        updated_at: v["updated_at"]
                            .as_str()
                            .and_then(|s| s.parse::<DateTime<Utc>>().ok()),
                    })
                })
        })
        .collect()
}

// --- CSV helpers ---

/// The escape sentinel prepended to formula-trigger cells on export.
///
/// U+0001 (SOH, Start of Heading) is a control character that cannot appear in
/// user-entered text, so it is unambiguous as an escape prefix. Using it lets
/// `csv_unescape` strip it unconditionally — no apostrophe-counting or
/// look-ahead heuristics needed — which means third-party CSV values are never
/// accidentally mutated and the round-trip is fully lossless for all inputs.
///
/// Spreadsheet apps (Excel, Google Sheets, LibreOffice) treat a cell starting
/// with \x01 as plain text and do not execute the rest as a formula, so the
/// injection-prevention goal is achieved without relying on the `'` prefix.
const CSV_ESCAPE_SENTINEL: char = '\x01';

/// Prefix-escape a cell value so spreadsheet apps don't execute it as a formula.
/// Values starting with `=`, `+`, `-`, or `@` are prefixed with `\x01` (SOH).
/// All other values are returned unchanged.
fn csv_safe(value: &str) -> std::borrow::Cow<'_, str> {
    if value.starts_with(['=', '+', '-', '@']) {
        std::borrow::Cow::Owned(format!("{CSV_ESCAPE_SENTINEL}{value}"))
    } else {
        std::borrow::Cow::Borrowed(value)
    }
}

/// Reverse `csv_safe`: strip the leading `\x01` sentinel when present.
/// Any other value, including those starting with apostrophes, is returned
/// unchanged. This is an unconditional strip, so round-trips are always
/// lossless and third-party CSV values are never affected.
fn csv_unescape(s: &str) -> String {
    s.strip_prefix(CSV_ESCAPE_SENTINEL)
        .unwrap_or(s)
        .to_string()
}

/// Encode tags as a JSON array so that tags containing `|` survive a roundtrip.
fn tags_to_csv(tags: &[String]) -> String {
    serde_json::to_string(tags).unwrap_or_else(|_| "[]".to_string())
}

/// Decode tags from a CSV cell. Accepts both the current JSON-array format
/// (written by `tags_to_csv`) and the legacy pipe-delimited format, so that
/// CSV files exported before this change can still be imported.
///
/// Known limitation: a single legacy tag whose text is a valid JSON string
/// array (e.g. `["a"]`) will be misread as a JSON-encoded tag list and
/// imported as tag `a`. This case is considered theoretical and not worth
/// the complexity of a versioned format marker.
fn tags_from_csv(cell: &str) -> Vec<String> {
    // JSON array format (current): ["a","b"]
    if let Ok(tags) = serde_json::from_str::<Vec<String>>(cell) {
        return tags;
    }
    // Legacy pipe-delimited format: a|b|c
    cell.split('|')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn bookmarks_to_csv_export(bookmarks: &[Bookmark]) -> Result<String, String> {
    let mut wtr = csv::Writer::from_writer(vec![]);
    wtr.write_record(["url", "title", "description", "tags"])
        .map_err(|e| e.to_string())?;
    for b in bookmarks {
        let tags = tags_to_csv(&b.tags);
        wtr.write_record([
            csv_safe(b.url.as_str()).as_ref(),
            csv_safe(b.title.as_deref().unwrap_or("")).as_ref(),
            csv_safe(b.description.as_deref().unwrap_or("")).as_ref(),
            tags.as_str(),
        ])
        .map_err(|e| e.to_string())?;
    }
    String::from_utf8(wtr.into_inner().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

fn bookmarks_to_csv_backup(bookmarks: &[Bookmark]) -> Result<String, String> {
    let mut wtr = csv::Writer::from_writer(vec![]);
    wtr.write_record([
        "id",
        "url",
        "title",
        "description",
        "image_url",
        "domain",
        "tags",
        "created_at",
        "updated_at",
    ])
    .map_err(|e| e.to_string())?;
    for b in bookmarks {
        let id_str = b.id.to_string();
        let created_str = b.created_at.to_rfc3339();
        let updated_str = b.updated_at.to_rfc3339();
        let tags = tags_to_csv(&b.tags);
        wtr.write_record([
            id_str.as_str(),
            csv_safe(b.url.as_str()).as_ref(),
            csv_safe(b.title.as_deref().unwrap_or("")).as_ref(),
            csv_safe(b.description.as_deref().unwrap_or("")).as_ref(),
            csv_safe(b.image_url.as_deref().unwrap_or("")).as_ref(),
            csv_safe(b.domain.as_deref().unwrap_or("")).as_ref(),
            tags.as_str(),
            created_str.as_str(),
            updated_str.as_str(),
        ])
        .map_err(|e| e.to_string())?;
    }
    String::from_utf8(wtr.into_inner().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

fn parse_csv(text: &str) -> Result<Vec<ImportRecord>, String> {
    let mut rdr = csv::Reader::from_reader(text.as_bytes());
    let headers = rdr.headers().map_err(|e| e.to_string())?.clone();

    let has_id = headers.iter().any(|h| h == "id");

    rdr.records()
        .enumerate()
        .map(|(i, row)| {
            let row = row.map_err(|e| format!("row {}: {e}", i + 2))?;
            let get = |name: &str| -> &str {
                headers
                    .iter()
                    .position(|h| h == name)
                    .and_then(|idx| row.get(idx))
                    .unwrap_or("")
            };
            Ok(ImportRecord {
                url: csv_unescape(get("url")),
                title: Some(csv_unescape(get("title")))
                    .filter(|s| !s.is_empty()),
                description: Some(csv_unescape(get("description")))
                    .filter(|s| !s.is_empty()),
                tags: tags_from_csv(get("tags")),
                id: if has_id {
                    get("id").parse::<Uuid>().ok()
                } else {
                    None
                },
                image_url: Some(csv_unescape(get("image_url")))
                    .filter(|s| !s.is_empty()),
                domain: Some(csv_unescape(get("domain")))
                    .filter(|s| !s.is_empty()),
                created_at: get("created_at").parse::<DateTime<Utc>>().ok(),
                updated_at: get("updated_at").parse::<DateTime<Utc>>().ok(),
            })
        })
        .collect()
}

// --- Handlers ---

pub async fn export_handler(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Query(params): Query<ExportParams>,
) -> impl IntoResponse {
    let bookmarks = match with_bookmarks!(&state.bookmarks, svc => svc.export_all(user.id).await) {
        Ok(b) => b,
        Err(e) => return Err(error_response(e).into_response()),
    };

    let date = chrono::Utc::now().format("%Y-%m-%d");
    let (body, content_type, filename) = match (params.format, params.mode) {
        (ExportFormat::Jsonl, ExportMode::Export) => (
            bookmarks_to_jsonl_export(&bookmarks),
            "application/x-ndjson",
            format!("bookmarks-{date}.jsonl"),
        ),
        (ExportFormat::Jsonl, ExportMode::Backup) => (
            bookmarks_to_jsonl_backup(&bookmarks),
            "application/x-ndjson",
            format!("bookmarks-backup-{date}.jsonl"),
        ),
        (ExportFormat::Csv, ExportMode::Export) => {
            match bookmarks_to_csv_export(&bookmarks) {
                Ok(s) => (s, "text/csv", format!("bookmarks-{date}.csv")),
                Err(e) => {
                    return Err(error_response(DomainError::Internal(e)).into_response())
                }
            }
        }
        (ExportFormat::Csv, ExportMode::Backup) => {
            match bookmarks_to_csv_backup(&bookmarks) {
                Ok(s) => (s, "text/csv", format!("bookmarks-backup-{date}.csv")),
                Err(e) => {
                    return Err(error_response(DomainError::Internal(e)).into_response())
                }
            }
        }
    };

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .body(Body::from(body))
        .unwrap())
}

pub async fn import_handler(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Query(params): Query<ImportParams>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let mut file_text: Option<String> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            match field.text().await {
                Ok(text) => {
                    file_text = Some(text);
                    break;
                }
                Err(e) => {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        Json(ErrorBody {
                            error: format!("failed to read file: {e}"),
                        }),
                    )
                        .into_response())
                }
            }
        }
    }

    let text = match file_text {
        Some(t) => t,
        None => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorBody {
                    error: "missing 'file' field in multipart body".to_string(),
                }),
            )
                .into_response())
        }
    };

    let records = match params.format {
        ImportFormat::Jsonl => parse_jsonl(&text),
        ImportFormat::Csv => parse_csv(&text),
    };

    let records = match records {
        Ok(r) => r,
        Err(e) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorBody {
                    error: format!("parse error: {e}"),
                }),
            )
                .into_response())
        }
    };

    let result = with_bookmarks!(
        &state.bookmarks,
        svc => svc.import_batch(user.id, records, params.strategy, params.mode).await
    );

    match result {
        Ok(r) => Ok(Json(r).into_response()),
        Err(e) => Err(error_response(e).into_response()),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/export", get(export_handler))
        .route("/import", post(import_handler))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use uuid::Uuid;

    fn make_bookmark(url: &str, tags: Vec<&str>) -> Bookmark {
        Bookmark {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            user_id: Uuid::new_v4(),
            url: url.to_string(),
            title: Some("Test".to_string()),
            description: Some("Desc".to_string()),
            image_url: Some("https://example.com/img.png".to_string()),
            domain: Some("example.com".to_string()),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap(),
        }
    }

    #[test]
    fn jsonl_export_roundtrip() {
        let bm = make_bookmark("https://example.com", vec!["rust", "web"]);
        let jsonl = bookmarks_to_jsonl_export(&[bm.clone()]);
        let records = parse_jsonl(&jsonl).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].url, bm.url);
        assert_eq!(records[0].tags, bm.tags);
        assert!(records[0].id.is_none());
    }

    #[test]
    fn jsonl_backup_roundtrip() {
        let bm = make_bookmark("https://example.com", vec!["rust"]);
        let jsonl = bookmarks_to_jsonl_backup(&[bm.clone()]);
        let records = parse_jsonl(&jsonl).unwrap();
        assert_eq!(records[0].id, Some(bm.id));
        assert_eq!(records[0].url, bm.url);
        assert_eq!(records[0].domain, bm.domain);
        assert_eq!(records[0].image_url, bm.image_url);
    }

    #[test]
    fn csv_export_roundtrip() {
        let bm = make_bookmark("https://example.com", vec!["rust", "web"]);
        let csv_text = bookmarks_to_csv_export(&[bm.clone()]).unwrap();
        let records = parse_csv(&csv_text).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].url, bm.url);
        assert_eq!(records[0].tags, bm.tags);
    }

    #[test]
    fn csv_backup_roundtrip() {
        let bm = make_bookmark("https://example.com", vec!["a", "b"]);
        let csv_text = bookmarks_to_csv_backup(&[bm.clone()]).unwrap();
        let records = parse_csv(&csv_text).unwrap();
        assert_eq!(records[0].id, Some(bm.id));
        assert_eq!(records[0].tags, bm.tags);
        assert_eq!(records[0].domain, bm.domain);
    }

    #[test]
    fn csv_handles_empty_optional_fields() {
        let mut bm = make_bookmark("https://example.com", vec![]);
        bm.title = None;
        bm.description = None;
        bm.image_url = None;
        bm.domain = None;
        let csv_text = bookmarks_to_csv_export(&[bm]).unwrap();
        let records = parse_csv(&csv_text).unwrap();
        assert!(records[0].title.is_none());
        assert!(records[0].tags.is_empty());
    }

    #[test]
    fn parse_jsonl_skips_empty_lines() {
        let text =
            "{\"url\":\"https://a.com\",\"tags\":[]}\n\n{\"url\":\"https://b.com\",\"tags\":[]}";
        let records = parse_jsonl(text).unwrap();
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn parse_jsonl_returns_error_for_missing_url() {
        let text = "{\"title\":\"No URL\",\"tags\":[]}";
        let result = parse_jsonl(text);
        assert!(result.is_err());
    }

    #[test]
    fn parse_jsonl_returns_error_for_malformed_json() {
        let text = "not json at all";
        let result = parse_jsonl(text);
        assert!(result.is_err());
    }

    #[test]
    fn csv_export_handles_special_characters() {
        let mut bm = make_bookmark("https://example.com", vec![]);
        bm.title = Some("Title with, commas and \"quotes\"".to_string());
        let csv_text = bookmarks_to_csv_export(&[bm.clone()]).unwrap();
        let records = parse_csv(&csv_text).unwrap();
        assert_eq!(records[0].title, bm.title);
    }

    #[test]
    fn csv_formula_injection_cells_are_escaped() {
        let mut bm = make_bookmark("https://example.com", vec![]);
        bm.title = Some("=SUM(1+1)".to_string());
        bm.description = Some("+malicious".to_string());
        let csv_text = bookmarks_to_csv_export(&[bm]).unwrap();
        // Raw CSV must not contain an unescaped leading = or +. The sentinel
        // \x01 is prepended so spreadsheet apps treat the cell as plain text.
        assert!(csv_text.contains("\x01=SUM(1+1)"));
        assert!(csv_text.contains("\x01+malicious"));
    }

    #[test]
    fn csv_formula_injection_survives_roundtrip() {
        let mut bm = make_bookmark("https://example.com", vec![]);
        bm.title = Some("=HYPERLINK(\"evil\")".to_string());
        let csv_text = bookmarks_to_csv_export(&[bm.clone()]).unwrap();
        let records = parse_csv(&csv_text).unwrap();
        // After roundtrip the original title is restored (sentinel stripped).
        assert_eq!(records[0].title, bm.title);
    }

    #[test]
    fn csv_tags_with_pipe_characters_roundtrip() {
        // Tags containing `|` must survive CSV export/import unchanged because
        // we now use JSON encoding instead of a pipe separator.
        let bm = make_bookmark("https://example.com", vec!["a|b", "c|d"]);
        let csv_text = bookmarks_to_csv_export(&[bm.clone()]).unwrap();
        let records = parse_csv(&csv_text).unwrap();
        assert_eq!(records[0].tags, bm.tags);
    }

    #[test]
    fn csv_apostrophe_formula_char_roundtrip() {
        // User data starting with '= is NOT a formula trigger (the ' is the
        // first char, not =), so csv_safe leaves it unchanged and csv_unescape
        // also leaves it unchanged (no \x01 sentinel present).
        let mut bm = make_bookmark("https://example.com", vec![]);
        bm.title = Some("'=not-a-formula".to_string());
        bm.description = Some("'+positive".to_string());
        let csv_text = bookmarks_to_csv_export(&[bm.clone()]).unwrap();
        let records = parse_csv(&csv_text).unwrap();
        assert_eq!(records[0].title, bm.title);
        assert_eq!(records[0].description, bm.description);
    }

    #[test]
    fn csv_genuine_apostrophe_not_stripped() {
        // User data starting with ' followed by any char is left intact.
        let mut bm = make_bookmark("https://example.com", vec![]);
        bm.title = Some("'90s nostalgia".to_string());
        let csv_text = bookmarks_to_csv_export(&[bm.clone()]).unwrap();
        let records = parse_csv(&csv_text).unwrap();
        assert_eq!(records[0].title, bm.title);
    }

    #[test]
    fn csv_import_third_party_apostrophe_formula_preserved() {
        // Third-party CSVs with apostrophe-escaped formula cells (e.g. from
        // Excel export) are imported as-is; the \x01 sentinel is never present
        // so csv_unescape never strips anything.
        let csv_text = "url,title\nhttps://example.com,'''=literal\n";
        let records = parse_csv(csv_text).unwrap();
        // All three apostrophes and the formula char are preserved unchanged.
        assert_eq!(records[0].title.as_deref(), Some("'''=literal"));
    }
}
