# CSV / JSONL Import & Export

**Date:** 2026-03-17

## Overview

Add import and export of bookmarks in CSV and JSONL formats across all three surfaces: server API, web UI, and CLI. Follows hexagonal architecture — the service layer owns domain logic, adapters handle serialization.

## API Endpoints

```
GET  /api/v1/bookmarks/export?format=csv|jsonl&mode=export|backup
POST /api/v1/bookmarks/import?format=csv|jsonl&strategy=skip|upsert&mode=import|restore
```

**Export:**
- `format` defaults to `jsonl`
- `mode` defaults to `export`
- Returns a file download with `Content-Disposition: attachment; filename="bookmarks-{date}.{ext}"`
- Requires bearer auth

**Import:**
- `format` defaults to `jsonl`
- `strategy` defaults to `upsert`
- `mode` defaults to `import`
- Accepts a multipart form upload with a single `file` field
- Returns `200 OK` with an `ImportResult` JSON body regardless of row-level errors

## Data Formats

### JSONL

**Export mode** (core fields only):
```json
{"url":"https://github.com","title":"GitHub","description":"Where the world builds software","tags":["development","coding"]}
```

**Backup mode** (matches seed file structure, omits `user_id`):
```json
{"id":"550e8400-e29b-41d4-a716-446655440000","url":"https://github.com","title":"GitHub","description":"Where the world builds software","image_url":"https://github.githubassets.com/images/modules/logos_page/GitHub-Mark.png","domain":"github.com","tags":["development","coding"],"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"}
```

### CSV

**Export mode** columns: `url,title,description,tags`

**Backup mode** columns: `id,url,title,description,image_url,domain,tags,created_at,updated_at`

- Header row always included
- Tags are pipe-separated within the cell: `development|coding`
- Empty optional fields are empty strings
- `user_id` is never included in any export format

## Import Modes

**`mode=import`** (default):
- Consumes core fields only (`url`, `title`, `description`, `tags`)
- Extra columns in the file (e.g. from a backup) are silently ignored
- Matches existing bookmarks by `url`
- Generates fresh UUIDs and timestamps for new records

**`mode=restore`**:
- Consumes all backup fields
- Matches existing bookmarks by `id`
- Preserves original `id`, `created_at`, `updated_at`, `domain`, `image_url`
- INSERTs with explicit UUID (no DB-generated id)

## Import Strategy

Applies in both import modes, determining what happens on a match:

- **`upsert`** (default): overwrite the matched record with incoming data
- **`skip`**: leave the matched record unchanged, count as skipped

## Service Layer

New domain types in `server/src/domain/`:

```rust
pub enum ExportMode { Export, Backup }
pub enum ImportMode { Import, Restore }
pub enum ImportStrategy { Skip, Upsert }

pub struct ImportRecord {
    // Backup fields — present only in backup files, ignored in Import mode
    pub id: Option<Uuid>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub domain: Option<String>,
    pub image_url: Option<String>,
    // Core fields
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
}

pub struct ImportResult {
    pub created: usize,
    pub updated: usize,
    pub skipped: usize,
    pub errors: Vec<(usize, String)>, // (row_index, error_message)
}
```

Two new methods on `BookmarkService`:

```rust
async fn export_all(user_id: Uuid, mode: ExportMode) -> Result<Vec<Bookmark>>;
async fn import_batch(user_id: Uuid, records: Vec<ImportRecord>, strategy: ImportStrategy, mode: ImportMode) -> Result<ImportResult>;
```

One new method on the `BookmarkRepository` port:

```rust
async fn upsert(bookmark: Bookmark) -> Result<Bookmark>;
```

Serialization and deserialization stay in the adapter layer (handlers, CLI). The service only speaks domain types.

## CLI

```
boop export [--format csv|jsonl] [--mode export|backup] [--output <file>]
boop import <file> [--format csv|jsonl] [--mode import|restore] [--strategy skip|upsert]
```

- `--format` defaults to `jsonl`; auto-detected from file extension when `--output` / `<file>` is provided, with `--format` as explicit override
- Export writes to stdout by default; `--output <file>` writes to a file
- After import, prints a summary: `Created: 42, Updated: 3, Skipped: 1, Errors: 0`
- CLI calls the same server endpoints as the web UI (multipart upload for import, file download for export)

## Error Handling

- **Row-level errors** (invalid URL, malformed field): recorded in `ImportResult.errors`, processing continues. Response is always `200 OK`.
- **Format/parse errors** (malformed JSONL, wrong CSV column count): abort the entire import with `400 Bad Request`.

## Testing

- Unit tests on `export_all` and `import_batch` with mock repo covering all strategy/mode combinations
- Unit tests on CSV and JSONL serialization/deserialization roundtrips
- E2E Playwright test for the web UI export/import flow
- CLI tests following existing pattern (mock server, assert request shape)
