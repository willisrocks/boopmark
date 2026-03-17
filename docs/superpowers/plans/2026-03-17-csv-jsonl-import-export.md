# CSV / JSONL Import & Export Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add CSV and JSONL import/export of bookmarks to all three surfaces (server API, web UI, CLI), sharing one service layer.

**Architecture:** New domain types in `transfer.rs` define import/export modes and result shapes. `BookmarkService` gets two new methods (`export_all`, `import_batch`) that all three surfaces call. Serialization/deserialization (CSV, JSONL) lives in the adapter layer (handlers, CLI), never in the service. The repository gets four new methods to support export-without-limit and restore-mode writes.

**Tech Stack:** Rust, `csv = "1"` for CSV parsing/writing, `serde_json` (already present) for JSONL, `axum::extract::Multipart` for file upload (requires `multipart` feature on `axum`), `reqwest` with `multipart` feature for CLI file upload.

---

## File Map

**New files:**
- `server/src/domain/transfer.rs` — `ExportMode`, `ImportMode`, `ImportStrategy`, `ImportRecord`, `ImportResult`
- `server/src/web/api/transfer.rs` — `export` and `import` handlers + route registration

**Modified files:**
- `Cargo.toml` — add `csv = "1"` to workspace deps; add `multipart` feature to `axum` and `reqwest`
- `server/Cargo.toml` — add `csv.workspace = true`
- `cli/Cargo.toml` — add `csv.workspace = true`
- `server/src/domain/mod.rs` — add `pub mod transfer;`
- `server/src/domain/ports/bookmark_repo.rs` — add `find_by_url`, `export_all`, `insert_with_id`, `upsert_full`
- `server/src/adapters/postgres/bookmark_repo.rs` — implement the four new repo methods
- `server/src/app/bookmarks.rs` — add `export_all` and `import_batch`
- `server/src/web/api/mod.rs` — nest transfer routes under `/bookmarks`
- `templates/settings/index.html` — add import/export section
- `cli/src/main.rs` — add `Export` and `Import` subcommands

---

## Chunk 1: Dependencies and Domain Types

### Task 1: Add dependencies

**Files:**
- Modify: `Cargo.toml`
- Modify: `server/Cargo.toml`
- Modify: `cli/Cargo.toml`

- [ ] **Step 1: Update workspace Cargo.toml**

In `Cargo.toml`, change the `axum` and `reqwest` entries and add `csv`:

```toml
axum = { version = "0.8", features = ["multipart"] }
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls", "multipart"] }
csv = "1"
```

- [ ] **Step 2: Add csv to server and cli Cargo.toml**

In `server/Cargo.toml`, add to `[dependencies]`:
```toml
csv.workspace = true
```

In `cli/Cargo.toml`, add to `[dependencies]`:
```toml
csv.workspace = true
```

- [ ] **Step 3: Verify it compiles**

```bash
cargo build
```
Expected: compiles without errors (no new functionality yet).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml server/Cargo.toml cli/Cargo.toml Cargo.lock
git commit -m "chore: add csv and multipart dependencies"
```

---

### Task 2: Define domain transfer types

**Files:**
- Create: `server/src/domain/transfer.rs`
- Modify: `server/src/domain/mod.rs`

- [ ] **Step 1: Create `server/src/domain/transfer.rs`**

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportMode {
    #[default]
    Export,
    Backup,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportMode {
    #[default]
    Import,
    Restore,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportStrategy {
    Skip,
    #[default]
    Upsert,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImportRecord {
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    // Backup-mode fields — ignored in Import mode
    pub id: Option<Uuid>,
    pub image_url: Option<String>,
    pub domain: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportResult {
    pub created: usize,
    pub updated: usize,
    pub skipped: usize,
    pub errors: Vec<ImportError>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportError {
    pub row: usize,
    pub message: String,
}
```

- [ ] **Step 2: Register the module**

In `server/src/domain/mod.rs`, add:
```rust
pub mod transfer;
```

- [ ] **Step 3: Verify it compiles**

```bash
cargo build -p boopmark-server
```
Expected: compiles.

- [ ] **Step 4: Commit**

```bash
git add server/src/domain/transfer.rs server/src/domain/mod.rs
git commit -m "feat: add import/export domain types"
```

---

## Chunk 2: Repository Port and Postgres Implementation

### Task 3: Extend BookmarkRepository trait

**Files:**
- Modify: `server/src/domain/ports/bookmark_repo.rs`

- [ ] **Step 1: Write failing compile test (open the file and verify trait compiles before changes)**

```bash
cargo build -p boopmark-server 2>&1 | head -5
```
Expected: builds cleanly.

- [ ] **Step 2: Add four new methods to the trait**

In `server/src/domain/ports/bookmark_repo.rs`, add to the trait after `tags_with_counts`:

```rust
async fn export_all(&self, user_id: Uuid) -> Result<Vec<Bookmark>, DomainError>;
async fn find_by_url(
    &self,
    user_id: Uuid,
    url: &str,
) -> Result<Option<Bookmark>, DomainError>;
async fn insert_with_id(&self, bookmark: Bookmark) -> Result<Bookmark, DomainError>;
async fn upsert_full(&self, bookmark: Bookmark) -> Result<Bookmark, DomainError>;
```

- [ ] **Step 3: Verify the trait change breaks the impl (expected)**

```bash
cargo build -p boopmark-server 2>&1 | grep "not implemented"
```
Expected: error about missing methods in `PostgresPool`.

---

### Task 4: Implement new repo methods in Postgres adapter

**Files:**
- Modify: `server/src/adapters/postgres/bookmark_repo.rs`

- [ ] **Step 1: Implement `export_all`**

Add to the `impl BookmarkRepository for PostgresPool` block:

```rust
async fn export_all(&self, user_id: Uuid) -> Result<Vec<Bookmark>, DomainError> {
    sqlx::query_as::<_, Bookmark>(
        "SELECT id, user_id, url, title, description, image_url, domain, tags, created_at, updated_at
         FROM bookmarks WHERE user_id = $1 ORDER BY created_at DESC",
    )
    .bind(user_id)
    .fetch_all(&self.pool)
    .await
    .map_err(|e| DomainError::Internal(e.to_string()))
}
```

- [ ] **Step 2: Implement `find_by_url`**

```rust
async fn find_by_url(
    &self,
    user_id: Uuid,
    url: &str,
) -> Result<Option<Bookmark>, DomainError> {
    sqlx::query_as::<_, Bookmark>(
        "SELECT id, user_id, url, title, description, image_url, domain, tags, created_at, updated_at
         FROM bookmarks WHERE user_id = $1 AND url = $2",
    )
    .bind(user_id)
    .bind(url)
    .fetch_optional(&self.pool)
    .await
    .map_err(|e| DomainError::Internal(e.to_string()))
}
```

- [ ] **Step 3: Implement `insert_with_id`**

```rust
async fn insert_with_id(&self, bookmark: Bookmark) -> Result<Bookmark, DomainError> {
    sqlx::query_as::<_, Bookmark>(
        "INSERT INTO bookmarks (id, user_id, url, title, description, image_url, domain, tags, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         RETURNING id, user_id, url, title, description, image_url, domain, tags, created_at, updated_at",
    )
    .bind(bookmark.id)
    .bind(bookmark.user_id)
    .bind(&bookmark.url)
    .bind(&bookmark.title)
    .bind(&bookmark.description)
    .bind(&bookmark.image_url)
    .bind(&bookmark.domain)
    .bind(&bookmark.tags)
    .bind(bookmark.created_at)
    .bind(bookmark.updated_at)
    .fetch_one(&self.pool)
    .await
    .map_err(|e| DomainError::Internal(e.to_string()))
}
```

- [ ] **Step 4: Implement `upsert_full`**

```rust
async fn upsert_full(&self, bookmark: Bookmark) -> Result<Bookmark, DomainError> {
    sqlx::query_as::<_, Bookmark>(
        "UPDATE bookmarks
         SET url = $3, title = $4, description = $5, image_url = $6, domain = $7,
             tags = $8, created_at = $9, updated_at = $10
         WHERE id = $1 AND user_id = $2
         RETURNING id, user_id, url, title, description, image_url, domain, tags, created_at, updated_at",
    )
    .bind(bookmark.id)
    .bind(bookmark.user_id)
    .bind(&bookmark.url)
    .bind(&bookmark.title)
    .bind(&bookmark.description)
    .bind(&bookmark.image_url)
    .bind(&bookmark.domain)
    .bind(&bookmark.tags)
    .bind(bookmark.created_at)
    .bind(bookmark.updated_at)
    .fetch_optional(&self.pool)
    .await
    .map_err(|e| DomainError::Internal(e.to_string()))?
    .ok_or(DomainError::NotFound)
}
```

- [ ] **Step 5: Verify it compiles**

```bash
cargo build -p boopmark-server
```
Expected: compiles without errors.

- [ ] **Step 6: Commit**

```bash
git add server/src/domain/ports/bookmark_repo.rs server/src/adapters/postgres/bookmark_repo.rs
git commit -m "feat: add export_all, find_by_url, insert_with_id, upsert_full to bookmark repo"
```

---

## Chunk 3: Service Layer

### Task 5: Add `export_all` and `import_batch` to `BookmarkService`

**Files:**
- Modify: `server/src/app/bookmarks.rs`

- [ ] **Step 1: Write unit tests first**

Add to the `#[cfg(test)]` block in `server/src/app/bookmarks.rs`:

```rust
#[cfg(test)]
mod import_tests {
    use super::*;
    use crate::domain::ports::bookmark_repo::BookmarkRepository;
    use crate::domain::ports::metadata::MetadataExtractor;
    use crate::domain::ports::storage::ObjectStorage;
    use crate::domain::transfer::*;
    use chrono::Utc;
    use std::sync::Mutex;
    use uuid::Uuid;

    struct MockRepo {
        bookmarks: Mutex<Vec<Bookmark>>,
    }

    impl MockRepo {
        fn new(bookmarks: Vec<Bookmark>) -> Self {
            Self { bookmarks: Mutex::new(bookmarks) }
        }
    }

    impl BookmarkRepository for MockRepo {
        async fn create(&self, user_id: Uuid, input: CreateBookmark) -> Result<Bookmark, DomainError> {
            let b = Bookmark {
                id: Uuid::new_v4(),
                user_id,
                url: input.url,
                title: input.title,
                description: input.description,
                image_url: input.image_url,
                domain: input.domain,
                tags: input.tags.unwrap_or_default(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
            };
            self.bookmarks.lock().unwrap().push(b.clone());
            Ok(b)
        }
        async fn get(&self, id: Uuid, user_id: Uuid) -> Result<Bookmark, DomainError> {
            self.bookmarks.lock().unwrap()
                .iter().find(|b| b.id == id && b.user_id == user_id)
                .cloned().ok_or(DomainError::NotFound)
        }
        async fn list(&self, user_id: Uuid, _filter: BookmarkFilter) -> Result<Vec<Bookmark>, DomainError> {
            Ok(self.bookmarks.lock().unwrap().iter().filter(|b| b.user_id == user_id).cloned().collect())
        }
        async fn update(&self, id: Uuid, user_id: Uuid, input: UpdateBookmark) -> Result<Bookmark, DomainError> {
            let mut bookmarks = self.bookmarks.lock().unwrap();
            let b = bookmarks.iter_mut().find(|b| b.id == id && b.user_id == user_id).ok_or(DomainError::NotFound)?;
            if let Some(t) = input.title { b.title = Some(t); }
            if let Some(d) = input.description { b.description = Some(d); }
            if let Some(tags) = input.tags { b.tags = tags; }
            Ok(b.clone())
        }
        async fn delete(&self, id: Uuid, user_id: Uuid) -> Result<(), DomainError> {
            let mut bookmarks = self.bookmarks.lock().unwrap();
            let len_before = bookmarks.len();
            bookmarks.retain(|b| !(b.id == id && b.user_id == user_id));
            if bookmarks.len() == len_before { Err(DomainError::NotFound) } else { Ok(()) }
        }
        async fn all_tags(&self, _user_id: Uuid) -> Result<Vec<String>, DomainError> { Ok(vec![]) }
        async fn tags_with_counts(&self, _user_id: Uuid) -> Result<Vec<(String, i64)>, DomainError> { Ok(vec![]) }
        async fn export_all(&self, user_id: Uuid) -> Result<Vec<Bookmark>, DomainError> {
            Ok(self.bookmarks.lock().unwrap().iter().filter(|b| b.user_id == user_id).cloned().collect())
        }
        async fn find_by_url(&self, user_id: Uuid, url: &str) -> Result<Option<Bookmark>, DomainError> {
            Ok(self.bookmarks.lock().unwrap().iter().find(|b| b.user_id == user_id && b.url == url).cloned())
        }
        async fn insert_with_id(&self, bookmark: Bookmark) -> Result<Bookmark, DomainError> {
            self.bookmarks.lock().unwrap().push(bookmark.clone());
            Ok(bookmark)
        }
        async fn upsert_full(&self, bookmark: Bookmark) -> Result<Bookmark, DomainError> {
            let mut bookmarks = self.bookmarks.lock().unwrap();
            let b = bookmarks.iter_mut().find(|b| b.id == bookmark.id).ok_or(DomainError::NotFound)?;
            *b = bookmark.clone();
            Ok(bookmark)
        }
    }

    struct NoopMetadata;
    impl MetadataExtractor for NoopMetadata {
        async fn extract(&self, _url: &str) -> Result<UrlMetadata, DomainError> {
            Ok(UrlMetadata { title: None, description: None, image_url: None, domain: None })
        }
    }

    struct NoopStorage;
    impl ObjectStorage for NoopStorage {
        async fn put(&self, _key: &str, _data: Vec<u8>, _content_type: &str) -> Result<String, DomainError> {
            Ok(String::new())
        }
        async fn get(&self, _key: &str) -> Result<Vec<u8>, DomainError> { Ok(vec![]) }
        async fn delete(&self, _key: &str) -> Result<(), DomainError> { Ok(()) }
        fn public_url(&self, key: &str) -> String { key.to_string() }
    }

    fn make_service(bookmarks: Vec<Bookmark>) -> BookmarkService<MockRepo, NoopMetadata, NoopStorage> {
        BookmarkService::new(
            std::sync::Arc::new(MockRepo::new(bookmarks)),
            std::sync::Arc::new(NoopMetadata),
            std::sync::Arc::new(NoopStorage),
        )
    }

    fn make_bookmark(user_id: Uuid, url: &str) -> Bookmark {
        Bookmark {
            id: Uuid::new_v4(),
            user_id,
            url: url.to_string(),
            title: Some("Test".to_string()),
            description: None,
            image_url: None,
            domain: None,
            tags: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn make_record(url: &str) -> ImportRecord {
        ImportRecord {
            url: url.to_string(),
            title: Some("Imported".to_string()),
            description: None,
            tags: vec![],
            id: None,
            image_url: None,
            domain: None,
            created_at: None,
            updated_at: None,
        }
    }

    #[tokio::test]
    async fn import_creates_new_bookmark() {
        let user_id = Uuid::new_v4();
        let svc = make_service(vec![]);
        let result = svc.import_batch(
            user_id,
            vec![make_record("https://example.com")],
            ImportStrategy::Upsert,
            ImportMode::Import,
        ).await.unwrap();
        assert_eq!(result.created, 1);
        assert_eq!(result.updated, 0);
        assert_eq!(result.skipped, 0);
    }

    #[tokio::test]
    async fn import_skips_existing_url_when_strategy_is_skip() {
        let user_id = Uuid::new_v4();
        let existing = make_bookmark(user_id, "https://example.com");
        let svc = make_service(vec![existing]);
        let result = svc.import_batch(
            user_id,
            vec![make_record("https://example.com")],
            ImportStrategy::Skip,
            ImportMode::Import,
        ).await.unwrap();
        assert_eq!(result.skipped, 1);
        assert_eq!(result.created, 0);
        assert_eq!(result.updated, 0);
    }

    #[tokio::test]
    async fn import_upserts_existing_url_when_strategy_is_upsert() {
        let user_id = Uuid::new_v4();
        let existing = make_bookmark(user_id, "https://example.com");
        let svc = make_service(vec![existing]);
        let result = svc.import_batch(
            user_id,
            vec![make_record("https://example.com")],
            ImportStrategy::Upsert,
            ImportMode::Import,
        ).await.unwrap();
        assert_eq!(result.updated, 1);
        assert_eq!(result.created, 0);
        assert_eq!(result.skipped, 0);
    }

    #[tokio::test]
    async fn import_records_error_for_invalid_url() {
        let user_id = Uuid::new_v4();
        let svc = make_service(vec![]);
        let result = svc.import_batch(
            user_id,
            vec![make_record("not-a-url")],
            ImportStrategy::Upsert,
            ImportMode::Import,
        ).await.unwrap();
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.created, 0);
    }

    #[tokio::test]
    async fn restore_creates_new_bookmark_with_original_id() {
        let user_id = Uuid::new_v4();
        let original_id = Uuid::new_v4();
        let svc = make_service(vec![]);
        let mut record = make_record("https://example.com");
        record.id = Some(original_id);
        let result = svc.import_batch(
            user_id,
            vec![record],
            ImportStrategy::Upsert,
            ImportMode::Restore,
        ).await.unwrap();
        assert_eq!(result.created, 1);
    }

    #[tokio::test]
    async fn restore_records_error_when_id_is_missing() {
        let user_id = Uuid::new_v4();
        let svc = make_service(vec![]);
        let result = svc.import_batch(
            user_id,
            vec![make_record("https://example.com")],
            ImportStrategy::Upsert,
            ImportMode::Restore,
        ).await.unwrap();
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.created, 0);
    }
}
```

- [ ] **Step 2: Run tests to confirm they fail**

```bash
cargo test -p boopmark-server import_tests 2>&1 | tail -20
```
Expected: compile errors — `export_all` and `import_batch` don't exist yet.

- [ ] **Step 3: Implement `export_all` and `import_batch`**

Add to the `impl<R, M, S> BookmarkService<R, M, S>` block in `server/src/app/bookmarks.rs`:

```rust
pub async fn export_all(&self, user_id: Uuid) -> Result<Vec<Bookmark>, DomainError> {
    self.repo.export_all(user_id).await
}

pub async fn import_batch(
    &self,
    user_id: Uuid,
    records: Vec<crate::domain::transfer::ImportRecord>,
    strategy: crate::domain::transfer::ImportStrategy,
    mode: crate::domain::transfer::ImportMode,
) -> Result<crate::domain::transfer::ImportResult, DomainError> {
    use crate::domain::transfer::{ImportError, ImportMode, ImportResult, ImportStrategy};

    let mut result = ImportResult { created: 0, updated: 0, skipped: 0, errors: vec![] };

    for (row, record) in records.into_iter().enumerate() {
        if url::Url::parse(&record.url).is_err() {
            result.errors.push(ImportError {
                row,
                message: format!("invalid URL: {}", record.url),
            });
            continue;
        }

        match mode {
            ImportMode::Import => {
                match self.repo.find_by_url(user_id, &record.url).await? {
                    Some(existing) => match strategy {
                        ImportStrategy::Skip => result.skipped += 1,
                        ImportStrategy::Upsert => {
                            self.repo.update(existing.id, user_id, UpdateBookmark {
                                title: record.title,
                                description: record.description,
                                tags: Some(record.tags),
                            }).await?;
                            result.updated += 1;
                        }
                    },
                    None => {
                        self.repo.create(user_id, CreateBookmark {
                            url: record.url,
                            title: record.title,
                            description: record.description,
                            image_url: None,
                            domain: None,
                            tags: Some(record.tags),
                        }).await?;
                        result.created += 1;
                    }
                }
            }
            ImportMode::Restore => {
                let Some(id) = record.id else {
                    result.errors.push(ImportError {
                        row,
                        message: "restore mode requires id field".to_string(),
                    });
                    continue;
                };

                let now = chrono::Utc::now();
                let bookmark = Bookmark {
                    id,
                    user_id,
                    url: record.url,
                    title: record.title,
                    description: record.description,
                    image_url: record.image_url,
                    domain: record.domain,
                    tags: record.tags,
                    created_at: record.created_at.unwrap_or(now),
                    updated_at: record.updated_at.unwrap_or(now),
                };

                match self.repo.get(id, user_id).await {
                    Ok(_) => match strategy {
                        ImportStrategy::Skip => result.skipped += 1,
                        ImportStrategy::Upsert => {
                            self.repo.upsert_full(bookmark).await?;
                            result.updated += 1;
                        }
                    },
                    Err(DomainError::NotFound) => {
                        self.repo.insert_with_id(bookmark).await?;
                        result.created += 1;
                    }
                    Err(e) => return Err(e),
                }
            }
        }
    }

    Ok(result)
}
```

- [ ] **Step 4: Run tests to confirm they pass**

```bash
cargo test -p boopmark-server import_tests
```
Expected: all 6 tests pass.

- [ ] **Step 5: Commit**

```bash
git add server/src/app/bookmarks.rs
git commit -m "feat: add export_all and import_batch to BookmarkService"
```

---

## Chunk 4: API Handlers

### Task 6: Export and import handlers

**Files:**
- Create: `server/src/web/api/transfer.rs`
- Modify: `server/src/web/api/mod.rs`

- [ ] **Step 1: Write serialization unit tests first**

Create `server/src/web/api/transfer.rs` with tests at the bottom:

```rust
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

// Re-use the error_response helper from the bookmarks handler pattern
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
    bookmarks.iter().map(|b| {
        serde_json::json!({
            "url": b.url,
            "title": b.title,
            "description": b.description,
            "tags": b.tags,
        }).to_string()
    }).collect::<Vec<_>>().join("\n")
}

fn bookmarks_to_jsonl_backup(bookmarks: &[Bookmark]) -> String {
    bookmarks.iter().map(|b| {
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
        }).to_string()
    }).collect::<Vec<_>>().join("\n")
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
                        url: v["url"].as_str().ok_or("missing url")?.to_string(),
                        title: v["title"].as_str().map(str::to_string),
                        description: v["description"].as_str().map(str::to_string),
                        tags: v["tags"].as_array()
                            .map(|arr| arr.iter().filter_map(|t| t.as_str().map(str::to_string)).collect())
                            .unwrap_or_default(),
                        id: v["id"].as_str().and_then(|s| s.parse::<Uuid>().ok()),
                        image_url: v["image_url"].as_str().map(str::to_string),
                        domain: v["domain"].as_str().map(str::to_string),
                        created_at: v["created_at"].as_str().and_then(|s| s.parse::<DateTime<Utc>>().ok()),
                        updated_at: v["updated_at"].as_str().and_then(|s| s.parse::<DateTime<Utc>>().ok()),
                    })
                })
        })
        .collect()
}

// --- CSV helpers ---

fn bookmarks_to_csv_export(bookmarks: &[Bookmark]) -> Result<String, String> {
    let mut wtr = csv::Writer::from_writer(vec![]);
    wtr.write_record(["url", "title", "description", "tags"]).map_err(|e| e.to_string())?;
    for b in bookmarks {
        wtr.write_record([
            b.url.as_str(),
            b.title.as_deref().unwrap_or(""),
            b.description.as_deref().unwrap_or(""),
            &b.tags.join("|"),
        ]).map_err(|e| e.to_string())?;
    }
    String::from_utf8(wtr.into_inner().map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

fn bookmarks_to_csv_backup(bookmarks: &[Bookmark]) -> Result<String, String> {
    let mut wtr = csv::Writer::from_writer(vec![]);
    wtr.write_record(["id", "url", "title", "description", "image_url", "domain", "tags", "created_at", "updated_at"])
        .map_err(|e| e.to_string())?;
    for b in bookmarks {
        wtr.write_record([
            b.id.to_string().as_str(),
            b.url.as_str(),
            b.title.as_deref().unwrap_or(""),
            b.description.as_deref().unwrap_or(""),
            b.image_url.as_deref().unwrap_or(""),
            b.domain.as_deref().unwrap_or(""),
            &b.tags.join("|"),
            &b.created_at.to_rfc3339(),
            &b.updated_at.to_rfc3339(),
        ]).map_err(|e| e.to_string())?;
    }
    String::from_utf8(wtr.into_inner().map_err(|e| e.to_string())?).map_err(|e| e.to_string())
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
                headers.iter().position(|h| h == name)
                    .and_then(|idx| row.get(idx))
                    .unwrap_or("")
            };
            Ok(ImportRecord {
                url: get("url").to_string(),
                title: Some(get("title")).filter(|s| !s.is_empty()).map(str::to_string),
                description: Some(get("description")).filter(|s| !s.is_empty()).map(str::to_string),
                tags: get("tags").split('|').filter(|s| !s.is_empty()).map(str::to_string).collect(),
                id: if has_id { get("id").parse::<Uuid>().ok() } else { None },
                image_url: Some(get("image_url")).filter(|s| !s.is_empty()).map(str::to_string),
                domain: Some(get("domain")).filter(|s| !s.is_empty()).map(str::to_string),
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
        (ExportFormat::Csv, ExportMode::Export) => match bookmarks_to_csv_export(&bookmarks) {
            Ok(s) => (s, "text/csv", format!("bookmarks-{date}.csv")),
            Err(e) => return Err(error_response(DomainError::Internal(e)).into_response()),
        },
        (ExportFormat::Csv, ExportMode::Backup) => match bookmarks_to_csv_backup(&bookmarks) {
            Ok(s) => (s, "text/csv", format!("bookmarks-backup-{date}.csv")),
            Err(e) => return Err(error_response(DomainError::Internal(e)).into_response()),
        },
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
                Ok(text) => { file_text = Some(text); break; }
                Err(e) => return Err((
                    StatusCode::BAD_REQUEST,
                    Json(ErrorBody { error: format!("failed to read file: {e}") }),
                ).into_response()),
            }
        }
    }

    let text = match file_text {
        Some(t) => t,
        None => return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorBody { error: "missing 'file' field in multipart body".to_string() }),
        ).into_response()),
    };

    let records = match params.format {
        ImportFormat::Jsonl => parse_jsonl(&text),
        ImportFormat::Csv => parse_csv(&text),
    };

    let records = match records {
        Ok(r) => r,
        Err(e) => return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorBody { error: format!("parse error: {e}") }),
        ).into_response()),
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
        // Export mode: no id in output
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
        let csv = bookmarks_to_csv_export(&[bm.clone()]).unwrap();
        let records = parse_csv(&csv).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].url, bm.url);
        assert_eq!(records[0].tags, bm.tags);
    }

    #[test]
    fn csv_backup_roundtrip() {
        let bm = make_bookmark("https://example.com", vec!["a", "b"]);
        let csv = bookmarks_to_csv_backup(&[bm.clone()]).unwrap();
        let records = parse_csv(&csv).unwrap();
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
        let csv = bookmarks_to_csv_export(&[bm]).unwrap();
        let records = parse_csv(&csv).unwrap();
        assert!(records[0].title.is_none());
        assert!(records[0].tags.is_empty());
    }

    #[test]
    fn parse_jsonl_skips_empty_lines() {
        let text = "{\"url\":\"https://a.com\",\"tags\":[]}\n\n{\"url\":\"https://b.com\",\"tags\":[]}";
        let records = parse_jsonl(text).unwrap();
        assert_eq!(records.len(), 2);
    }
}
```

- [ ] **Step 2: Run serialization tests**

```bash
cargo test -p boopmark-server -- web::api::transfer::tests
```
Expected: all 6 tests pass.

- [ ] **Step 3: Register transfer routes**

In `server/src/web/api/mod.rs`, add:

```rust
pub mod transfer;
```

And in `pub fn routes()`, change the existing bookmarks nest to merge the transfer routes in:
```rust
.nest("/bookmarks", bookmarks::routes().merge(transfer::routes()))
```

- [ ] **Step 4: Build and verify**

```bash
cargo build -p boopmark-server
```
Expected: compiles.

- [ ] **Step 5: Commit**

```bash
git add server/src/web/api/transfer.rs server/src/web/api/mod.rs
git commit -m "feat: add export and import API handlers"
```

---

## Chunk 5: Web UI and CLI

### Task 7: Add import/export section to settings page

**Files:**
- Modify: `templates/settings/index.html`

- [ ] **Step 1: Add a new section to settings page**

In `templates/settings/index.html`, add a new `<section>` after the API Keys section (before `</div></main>`):

```html
<section class="space-y-5">
    <div>
        <h2 class="text-lg font-semibold">Import & Export</h2>
        <p class="text-sm text-gray-400">Backup or migrate your bookmarks.</p>
    </div>

    <div class="space-y-3">
        <p class="text-sm font-medium text-gray-200">Export</p>
        <div class="flex flex-wrap gap-2">
            <a href="/api/v1/bookmarks/export?format=jsonl&mode=export"
               class="px-4 py-2 bg-gray-700 hover:bg-gray-600 rounded-lg text-sm font-medium">
                Export JSONL
            </a>
            <a href="/api/v1/bookmarks/export?format=csv&mode=export"
               class="px-4 py-2 bg-gray-700 hover:bg-gray-600 rounded-lg text-sm font-medium">
                Export CSV
            </a>
            <a href="/api/v1/bookmarks/export?format=jsonl&mode=backup"
               class="px-4 py-2 bg-gray-700 hover:bg-gray-600 rounded-lg text-sm font-medium">
                Backup JSONL
            </a>
            <a href="/api/v1/bookmarks/export?format=csv&mode=backup"
               class="px-4 py-2 bg-gray-700 hover:bg-gray-600 rounded-lg text-sm font-medium">
                Backup CSV
            </a>
        </div>
    </div>

    <div class="space-y-3">
        <p class="text-sm font-medium text-gray-200">Import</p>
        <form id="import-form" class="space-y-3">
            <div class="flex flex-wrap gap-3">
                <select name="format" class="px-3 py-2 rounded-lg bg-[#1a1d2e] border border-gray-700 text-gray-200 text-sm">
                    <option value="jsonl">JSONL</option>
                    <option value="csv">CSV</option>
                </select>
                <select name="mode" class="px-3 py-2 rounded-lg bg-[#1a1d2e] border border-gray-700 text-gray-200 text-sm">
                    <option value="import">Import</option>
                    <option value="restore">Restore</option>
                </select>
                <select name="strategy" class="px-3 py-2 rounded-lg bg-[#1a1d2e] border border-gray-700 text-gray-200 text-sm">
                    <option value="upsert">Upsert</option>
                    <option value="skip">Skip duplicates</option>
                </select>
            </div>
            <div class="flex gap-3 items-center">
                <input type="file" name="file" accept=".jsonl,.csv,.json"
                    class="text-sm text-gray-300 file:mr-3 file:py-2 file:px-4 file:rounded-lg file:border-0 file:text-sm file:font-medium file:bg-gray-700 file:text-gray-200 hover:file:bg-gray-600">
                <button type="submit"
                    class="px-4 py-2 bg-blue-600 hover:bg-blue-700 rounded-lg text-sm font-medium">
                    Import
                </button>
            </div>
        </form>
        <div id="import-result" class="text-sm"></div>
    </div>

    <script>
        document.getElementById('import-form').addEventListener('submit', async (e) => {
            e.preventDefault();
            const form = e.target;
            const format = form.format.value;
            const mode = form.mode.value;
            const strategy = form.strategy.value;
            const file = form.file.files[0];
            if (!file) return;
            const body = new FormData();
            body.append('file', file);
            const res = await fetch(`/api/v1/bookmarks/import?format=${format}&mode=${mode}&strategy=${strategy}`, {
                method: 'POST', body,
            });
            const data = await res.json();
            const el = document.getElementById('import-result');
            if (res.ok) {
                el.className = 'text-sm text-emerald-300';
                el.textContent = `Created: ${data.created}, Updated: ${data.updated}, Skipped: ${data.skipped}, Errors: ${data.errors.length}`;
            } else {
                el.className = 'text-sm text-red-400';
                el.textContent = data.error || 'Import failed';
            }
        });
    </script>
</section>
```

- [ ] **Step 2: Build to verify templates compile**

```bash
cargo build -p boopmark-server
```
Expected: compiles (Askama validates templates at build time).

- [ ] **Step 3: Commit**

```bash
git add templates/settings/index.html
git commit -m "feat: add import/export section to settings page"
```

---

### Task 8: Add `export` and `import` commands to CLI

**Files:**
- Modify: `cli/src/main.rs`

- [ ] **Step 1: Add Import and Export to the Commands enum**

In `cli/src/main.rs`, add to the `Commands` enum after `Delete`:

```rust
/// Export bookmarks to a file
Export {
    /// Output format: jsonl (default) or csv
    #[arg(long, default_value = "jsonl")]
    format: String,
    /// Export mode: export (default, core fields) or backup (all fields)
    #[arg(long, default_value = "export")]
    mode: String,
    /// Write output to file (default: stdout). Format auto-detected from extension.
    #[arg(long, short)]
    output: Option<String>,
},
/// Import bookmarks from a file
Import {
    /// Path to the file to import
    file: String,
    /// File format: jsonl (default) or csv. Auto-detected from extension if omitted.
    #[arg(long)]
    format: Option<String>,
    /// Import mode: import (default) or restore
    #[arg(long, default_value = "import")]
    mode: String,
    /// Conflict strategy: upsert (default) or skip
    #[arg(long, default_value = "upsert")]
    strategy: String,
},
```

- [ ] **Step 2: Add `post_multipart` to `ApiClient`**

In the `ApiClient` impl block, add:

```rust
async fn post_multipart(
    &self,
    path: &str,
    file_bytes: Vec<u8>,
    filename: &str,
    mime: &str,
) -> Result<reqwest::Response, String> {
    let part = reqwest::multipart::Part::bytes(file_bytes)
        .file_name(filename.to_string())
        .mime_str(mime)
        .map_err(|e| e.to_string())?;
    let form = reqwest::multipart::Form::new().part("file", part);
    self.client
        .post(self.url(path))
        .bearer_auth(&self.api_key)
        .multipart(form)
        .send()
        .await
        .map_err(|e| e.to_string())
}
```

- [ ] **Step 3: Add handlers in the `match` on `cli.command`**

In the `main` function's command match, add cases for `Export` and `Import`:

```rust
Commands::Export { format, mode, output } => {
    let cfg = AppConfig::load();
    let client = cfg.client()?;

    // Auto-detect format from output extension
    let format = if let Some(ref path) = output {
        if path.ends_with(".csv") { "csv".to_string() } else { format }
    } else {
        format
    };

    let url = format!("/bookmarks/export?format={format}&mode={mode}");
    let resp = client.get(&url).await?;
    if !resp.status().is_success() {
        return Err(format!("export failed: HTTP {}", resp.status()));
    }
    let body = resp.text().await.map_err(|e| e.to_string())?;

    match output {
        Some(path) => std::fs::write(&path, &body).map_err(|e| e.to_string())?,
        None => print!("{body}"),
    }
    Ok(())
}

Commands::Import { file, format, mode, strategy } => {
    let cfg = AppConfig::load();
    let client = cfg.client()?;

    // Auto-detect format from file extension
    let format = format.unwrap_or_else(|| {
        if file.ends_with(".csv") { "csv".to_string() } else { "jsonl".to_string() }
    });
    let mime = if format == "csv" { "text/csv" } else { "application/x-ndjson" };

    let bytes = std::fs::read(&file).map_err(|e| format!("failed to read {file}: {e}"))?;
    let filename = std::path::Path::new(&file)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let url = format!("/bookmarks/import?format={format}&mode={mode}&strategy={strategy}");
    let resp = client.post_multipart(&url, bytes, &filename, mime).await?;

    #[derive(serde::Deserialize)]
    struct ImportResult {
        created: usize,
        updated: usize,
        skipped: usize,
        errors: Vec<serde_json::Value>,
    }

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("import failed: {body}"));
    }

    let result: ImportResult = resp.json().await.map_err(|e| e.to_string())?;
    println!("Created: {}, Updated: {}, Skipped: {}, Errors: {}",
        result.created, result.updated, result.skipped, result.errors.len());
    if !result.errors.is_empty() {
        for err in &result.errors {
            eprintln!("  error: {err}");
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run existing CLI tests**

```bash
cargo test -p boop
```
Expected: all existing tests still pass.

- [ ] **Step 5: Build the CLI**

```bash
cargo build -p boop
```
Expected: compiles.

- [ ] **Step 6: Commit**

```bash
git add cli/src/main.rs
git commit -m "feat: add export and import commands to boop CLI"
```

---

## Final Verification

- [ ] **Run all tests**

```bash
cargo test
```
Expected: all tests pass.

- [ ] **Run the E2E test suite**

```bash
npx playwright test tests/e2e/suggest.spec.js
```
Expected: existing E2E tests still pass.

- [ ] **Final commit if clean**

```bash
git status
```
Expected: clean working tree.
