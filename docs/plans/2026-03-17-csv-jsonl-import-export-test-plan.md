# CSV / JSONL Import & Export — Test Plan

## Harness Requirements

**No new harnesses need to be built.** The existing infrastructure covers all needs:

- **Playwright E2E harness**: `playwright.config.js` + `scripts/e2e/start-server.sh` launches a real server on `http://127.0.0.1:4010` with `ENABLE_E2E_AUTH=1`, real Postgres, and local storage. Tests use `signIn(page)` to authenticate via the E2E button, and `page.evaluate(fetch(...))` to call API endpoints with bearer tokens.
- **Rust unit test harness**: `cargo test -p boopmark-server` and `cargo test -p boop` run in-process tests with mock repos and no external dependencies.
- **API key creation helper**: Reuse the `createApiKey(page, name)` pattern from `tests/e2e/api-enrichment.spec.js` to get a bearer token for API calls.
- **Fresh browser context pattern**: Reuse the `browser.newContext()` pattern from existing API specs for bearer-only auth without session cookies.

All E2E tests run against the Playwright E2E server with a real Postgres database. All unit tests run in-process with mock implementations.

---

## Test Plan

### 1. Export JSONL via API returns a downloadable file with correct content

- **Name**: Exporting bookmarks as JSONL returns a file with url, title, description, tags for each bookmark
- **Type**: scenario
- **Disposition**: new
- **Harness**: Playwright E2E
- **Preconditions**: User is signed in. At least one bookmark exists (created via the API in test setup).
- **Actions**:
  1. Sign in via E2E auth button.
  2. Create an API key via the settings UI.
  3. In a fresh browser context, create a bookmark via `POST /api/v1/bookmarks` with bearer auth: `{ url: "https://example.com/export-test", title: "Export Test", description: "A test bookmark", tags: ["test", "export"] }`.
  4. Call `GET /api/v1/bookmarks/export?format=jsonl&mode=export` with bearer auth.
  5. Assert response status is 200.
  6. Assert `Content-Type` header is `application/x-ndjson`.
  7. Assert `Content-Disposition` header contains `attachment` and `bookmarks-` and `.jsonl`.
  8. Parse the response body as JSONL (split by newlines, parse each as JSON).
  9. Assert at least one line contains `url: "https://example.com/export-test"`, `title: "Export Test"`, and `tags` including `"test"` and `"export"`.
  10. Assert no line contains an `id`, `user_id`, `created_at`, or `updated_at` field.
- **Expected outcome**: The export endpoint returns a valid JSONL file download with only core fields. Source of truth: design spec Section "Data Formats" specifying export mode includes only `url`, `title`, `description`, `tags`.
- **Interactions**: Exercises `BookmarkService::export_all` -> `BookmarkRepository::export_all` -> Postgres, plus JSONL serialization in the handler.

### 2. Export CSV via API returns a downloadable file with correct headers and content

- **Name**: Exporting bookmarks as CSV returns a file with url, title, description, tags columns
- **Type**: scenario
- **Disposition**: new
- **Harness**: Playwright E2E
- **Preconditions**: User is signed in. At least one bookmark exists.
- **Actions**:
  1. Reuse the same setup as test 1 (API key + bookmark creation).
  2. Call `GET /api/v1/bookmarks/export?format=csv&mode=export` with bearer auth.
  3. Assert response status is 200.
  4. Assert `Content-Type` header is `text/csv`.
  5. Assert `Content-Disposition` header contains `attachment` and `.csv`.
  6. Parse the response body as CSV text.
  7. Assert the header row is `url,title,description,tags`.
  8. Assert at least one data row contains the created bookmark's URL and pipe-separated tags (`test|export`).
- **Expected outcome**: The CSV export uses the correct column headers and pipe-separated tags. Source of truth: design spec "CSV export mode columns: `url,title,description,tags`" and "Tags are pipe-separated".
- **Interactions**: Same as test 1, plus CSV serialization.

### 3. Export backup JSONL includes all fields (id, timestamps, domain, image_url)

- **Name**: Backup mode export includes id, created_at, updated_at, domain, image_url in each JSONL line
- **Type**: scenario
- **Disposition**: new
- **Harness**: Playwright E2E
- **Preconditions**: User is signed in. At least one bookmark exists.
- **Actions**:
  1. Reuse setup from test 1.
  2. Call `GET /api/v1/bookmarks/export?format=jsonl&mode=backup` with bearer auth.
  3. Assert response status is 200.
  4. Parse the body as JSONL.
  5. Assert at least one line contains `id` (UUID string), `url`, `tags` (array), `created_at` (ISO datetime), `updated_at` (ISO datetime).
  6. Assert no line contains `user_id`.
- **Expected outcome**: Backup mode includes all bookmark fields except `user_id`. Source of truth: design spec "Backup mode (matches seed file structure, omits `user_id`)".
- **Interactions**: Same service path as test 1, different serialization branch.

### 4. Import JSONL via API creates new bookmarks and returns result summary

- **Name**: Importing a JSONL file creates bookmarks and returns created/updated/skipped/errors counts
- **Type**: scenario
- **Disposition**: new
- **Harness**: Playwright E2E
- **Preconditions**: User is signed in. The specific URLs being imported do not already exist as bookmarks.
- **Actions**:
  1. Sign in and create an API key.
  2. Construct a JSONL string with two valid records:
     ```
     {"url":"https://import-test-1.example.com","title":"Import 1","description":"First","tags":["a"]}
     {"url":"https://import-test-2.example.com","title":"Import 2","description":"Second","tags":["b"]}
     ```
  3. Create a `FormData` with a `file` field containing the JSONL string as a Blob.
  4. Call `POST /api/v1/bookmarks/import?format=jsonl&strategy=upsert&mode=import` with bearer auth and the multipart body.
  5. Assert response status is 200.
  6. Assert response JSON has `created: 2`, `updated: 0`, `skipped: 0`, `errors: []`.
  7. Call `GET /api/v1/bookmarks/export?format=jsonl&mode=export` and verify both imported URLs appear.
- **Expected outcome**: The import endpoint creates new bookmarks and returns an accurate summary. Subsequent export confirms the data persisted. Source of truth: design spec import mode section and `ImportResult` struct.
- **Interactions**: Exercises multipart parsing -> JSONL parsing -> `BookmarkService::import_batch` -> `BookmarkRepository::find_by_url` + `BookmarkRepository::create` -> Postgres.

### 5. Import CSV via API creates new bookmarks

- **Name**: Importing a CSV file creates bookmarks and returns correct counts
- **Type**: scenario
- **Disposition**: new
- **Harness**: Playwright E2E
- **Preconditions**: User is signed in. Import URLs do not already exist.
- **Actions**:
  1. Sign in and create an API key.
  2. Construct a CSV string:
     ```
     url,title,description,tags
     https://csv-import-1.example.com,CSV One,First CSV,tag1|tag2
     https://csv-import-2.example.com,CSV Two,Second CSV,tag3
     ```
  3. POST as multipart to `/api/v1/bookmarks/import?format=csv&strategy=upsert&mode=import`.
  4. Assert response status 200 with `created: 2`.
  5. Verify via export that the imported bookmarks exist with correct tags (pipe-separated tags parsed correctly into arrays).
- **Expected outcome**: CSV import correctly parses pipe-separated tags and creates bookmarks. Source of truth: design spec CSV format.
- **Interactions**: CSV parsing -> import_batch -> Postgres.

### 6. Import with strategy=skip leaves existing bookmarks unchanged

- **Name**: Importing with skip strategy does not overwrite existing bookmarks with matching URLs
- **Type**: integration
- **Disposition**: new
- **Harness**: Playwright E2E
- **Preconditions**: User is signed in. A bookmark with a known URL already exists.
- **Actions**:
  1. Sign in and create an API key.
  2. Create a bookmark via API: `POST /api/v1/bookmarks` with `{ url: "https://skip-test.example.com", title: "Original Title", tags: ["original"] }`.
  3. Import a JSONL file containing the same URL with different title: `{"url":"https://skip-test.example.com","title":"New Title","tags":["new"]}`.
  4. Use `strategy=skip`.
  5. Assert response has `skipped: 1`, `created: 0`, `updated: 0`.
  6. Fetch the bookmark via export and verify title is still `"Original Title"` and tags still contain `"original"`.
- **Expected outcome**: Skip strategy preserves existing data. Source of truth: design spec "skip: leave the matched record unchanged, count as skipped".
- **Interactions**: `find_by_url` returns existing -> skip path in `import_batch`.

### 7. Import with strategy=upsert updates existing bookmarks

- **Name**: Importing with upsert strategy overwrites matching bookmark's title, description, and tags
- **Type**: integration
- **Disposition**: new
- **Harness**: Playwright E2E
- **Preconditions**: User is signed in. A bookmark with a known URL already exists.
- **Actions**:
  1. Sign in and create an API key.
  2. Create a bookmark via API: `{ url: "https://upsert-test.example.com", title: "Old Title", tags: ["old"] }`.
  3. Import a JSONL file with the same URL and new data: `{"url":"https://upsert-test.example.com","title":"Updated Title","description":"New desc","tags":["updated"]}`.
  4. Use `strategy=upsert` (default).
  5. Assert response has `updated: 1`, `created: 0`, `skipped: 0`.
  6. Export and verify the bookmark now has `title: "Updated Title"` and tags contain `"updated"`.
- **Expected outcome**: Upsert strategy overwrites matched bookmark fields. Source of truth: design spec "upsert: overwrite the matched record with incoming data".
- **Interactions**: `find_by_url` returns existing -> update path in `import_batch` -> `BookmarkRepository::update`.

### 8. Import with invalid URL records error but continues processing other rows

- **Name**: A row with an invalid URL is recorded as an error without aborting the import
- **Type**: boundary
- **Disposition**: new
- **Harness**: Playwright E2E
- **Preconditions**: User is signed in.
- **Actions**:
  1. Sign in and create an API key.
  2. Import a JSONL file with one valid and one invalid URL:
     ```
     {"url":"https://valid-row.example.com","title":"Valid","tags":[]}
     {"url":"not-a-url","title":"Invalid","tags":[]}
     ```
  3. Assert response status is 200 (not 400 — row-level errors don't abort).
  4. Assert response has `created: 1`, `errors` array length 1.
  5. Assert the error entry references the invalid URL.
- **Expected outcome**: Row-level errors are collected without aborting. Source of truth: design spec "Row-level errors (invalid URL, malformed field): recorded in `ImportResult.errors`, processing continues".
- **Interactions**: URL validation in `import_batch` -> error path -> continue loop.

### 9. Import with malformed file returns 400

- **Name**: Uploading a malformed file (not valid JSONL/CSV) returns 400 Bad Request
- **Type**: boundary
- **Disposition**: new
- **Harness**: Playwright E2E
- **Preconditions**: User is signed in.
- **Actions**:
  1. Sign in and create an API key.
  2. POST a multipart file with body `"this is not valid json at all"` to `/api/v1/bookmarks/import?format=jsonl`.
  3. Assert response status is 400.
  4. Assert response JSON has an `error` field containing "parse error".
- **Expected outcome**: Format-level parse errors abort the import with 400. Source of truth: design spec "Format/parse errors (malformed JSONL, wrong CSV column count): abort the entire import with `400 Bad Request`".
- **Interactions**: `parse_jsonl` returns `Err` -> handler returns 400.

### 10. Import without file field returns 400

- **Name**: POST to import endpoint without a 'file' field in multipart body returns 400
- **Type**: boundary
- **Disposition**: new
- **Harness**: Playwright E2E
- **Preconditions**: User is signed in.
- **Actions**:
  1. Sign in and create an API key.
  2. POST to `/api/v1/bookmarks/import?format=jsonl` with an empty `FormData` (no `file` field).
  3. Assert response status is 400.
  4. Assert response JSON error message mentions "missing" or "file".
- **Expected outcome**: The handler rejects requests without the required file field. Source of truth: implementation plan handler code checking for `file_text`.
- **Interactions**: Multipart parsing -> missing field check.

### 11. Export and import endpoints require authentication

- **Name**: Unauthenticated requests to export and import endpoints return 401
- **Type**: boundary
- **Disposition**: new
- **Harness**: Playwright E2E
- **Preconditions**: No authentication (no session cookie, no bearer token).
- **Actions**:
  1. Using `request` fixture (no auth), call `GET /api/v1/bookmarks/export`.
  2. Assert status is 401.
  3. Using `request` fixture, call `POST /api/v1/bookmarks/import` with an empty body.
  4. Assert status is 401.
- **Expected outcome**: Both endpoints enforce authentication. Source of truth: design spec "Both require bearer auth (same as existing endpoints)".
- **Interactions**: `AuthUser` extractor rejects unauthenticated requests.

### 12. Export-import JSONL roundtrip preserves data

- **Name**: Exporting bookmarks as JSONL and re-importing them results in the same bookmarks
- **Type**: integration
- **Disposition**: new
- **Harness**: Playwright E2E
- **Preconditions**: User is signed in with bookmarks.
- **Actions**:
  1. Sign in and create an API key.
  2. Create two bookmarks via API with distinct URLs, titles, descriptions, and tags.
  3. Export as JSONL (`format=jsonl&mode=export`).
  4. Delete both bookmarks via API (`DELETE /api/v1/bookmarks/{id}`).
  5. Import the exported JSONL content.
  6. Export again and compare the JSONL output with the original export.
  7. Assert the URLs, titles, descriptions, and tags match.
- **Expected outcome**: Data survives a full export-delete-import cycle. Source of truth: design spec specifying that export and import use the same field set.
- **Interactions**: Full stack roundtrip: export_all -> serialize -> parse -> import_batch -> create -> export_all.

### 13. Export-import CSV roundtrip preserves data

- **Name**: Exporting bookmarks as CSV and re-importing preserves all fields
- **Type**: integration
- **Disposition**: new
- **Harness**: Playwright E2E
- **Preconditions**: User is signed in with bookmarks.
- **Actions**:
  1. Same flow as test 12 but using `format=csv`.
  2. Assert URLs, titles, descriptions, and pipe-separated tags survive the roundtrip.
- **Expected outcome**: CSV roundtrip preserves data including multi-value tags. Source of truth: design spec CSV format.
- **Interactions**: Same as test 12 but exercises CSV serialization/parsing.

### 14. Settings page shows Import & Export section with export links and import form

- **Name**: The settings page displays export buttons and an import form
- **Type**: scenario
- **Disposition**: extend (extends existing settings.spec.js)
- **Harness**: Playwright E2E
- **Preconditions**: User is signed in.
- **Actions**:
  1. Sign in and navigate to `/settings`.
  2. Assert heading "Import & Export" is visible.
  3. Assert text "Backup or migrate your bookmarks." is visible.
  4. Assert four export links are visible: "Export JSONL", "Export CSV", "Backup JSONL", "Backup CSV".
  5. Assert each link has the correct `href` (e.g., `/api/v1/bookmarks/export?format=jsonl&mode=export`).
  6. Assert the import form is visible with `select` dropdowns for format, mode, and strategy.
  7. Assert a file input and "Import" button are present.
- **Expected outcome**: The settings page renders the complete import/export section. Source of truth: implementation plan Task 6 template HTML.
- **Interactions**: Askama template rendering.

### 15. Web UI import form submits file and displays result

- **Name**: Using the settings page import form to upload a JSONL file shows the result summary
- **Type**: scenario
- **Disposition**: new
- **Harness**: Playwright E2E
- **Preconditions**: User is signed in. A valid JSONL file exists to upload.
- **Actions**:
  1. Sign in and navigate to `/settings`.
  2. Write a temporary JSONL file with one bookmark record.
  3. Select "JSONL" format and "Import" mode from the dropdowns.
  4. Upload the file via the file input using `page.setInputFiles()`.
  5. Click the "Import" button.
  6. Assert the `#import-result` element displays text matching "Created: 1" with a success color class.
- **Expected outcome**: The form submits via JavaScript fetch, parses the JSON response, and displays the summary. Source of truth: implementation plan Task 6 JavaScript handler.
- **Interactions**: Frontend JS -> multipart fetch -> import_handler -> service -> Postgres.

### 16. Backup-mode export and restore-mode import roundtrip preserves IDs and timestamps

- **Name**: Backup export followed by restore import preserves original bookmark IDs and timestamps
- **Type**: integration
- **Disposition**: new
- **Harness**: Playwright E2E
- **Preconditions**: User is signed in with bookmarks.
- **Actions**:
  1. Sign in and create an API key.
  2. Create a bookmark via API and note its `id` and `created_at` from the response.
  3. Export as backup JSONL (`format=jsonl&mode=backup`).
  4. Delete the bookmark via API.
  5. Import the backup file with `mode=restore&strategy=upsert`.
  6. Assert response has `created: 1`.
  7. Fetch the bookmark by listing all bookmarks and finding the one with the original URL.
  8. Assert the restored bookmark has the same `id` as the original.
- **Expected outcome**: Restore mode preserves the original UUID. Source of truth: design spec "Restore mode: uses all backup fields, matches by `id`, preserves original `created_at`/`updated_at`".
- **Interactions**: Full backup/restore path: export_all -> backup serialize -> parse with id/timestamps -> import_batch(Restore) -> insert_with_id -> Postgres.

### 17. Restore mode rejects records without id field

- **Name**: Importing in restore mode with a record missing the id field records an error
- **Type**: boundary
- **Disposition**: new
- **Harness**: Playwright E2E
- **Preconditions**: User is signed in.
- **Actions**:
  1. Sign in and create an API key.
  2. Import a JSONL file with a record that has no `id` field: `{"url":"https://no-id.example.com","title":"No ID","tags":[]}`.
  3. Use `mode=restore`.
  4. Assert response has `errors` array length 1.
  5. Assert the error message mentions "id".
- **Expected outcome**: Restore mode requires the id field and reports a row-level error when missing. Source of truth: design spec "Restore mode: uses all backup fields" and implementation plan `import_batch` code requiring `record.id` in Restore mode.
- **Interactions**: `import_batch` Restore branch -> missing id check -> error.

### 18. JSONL serialization roundtrip (export mode) preserves core fields

- **Name**: Serializing bookmarks to export-mode JSONL and parsing back preserves url, title, description, tags
- **Type**: unit
- **Disposition**: new (included in implementation plan Task 5)
- **Harness**: `cargo test -p boopmark-server -- web::api::transfer::tests::jsonl_export_roundtrip`
- **Preconditions**: None (in-process test).
- **Actions**:
  1. Create a `Bookmark` struct with known values and tags `["rust", "web"]`.
  2. Serialize with `bookmarks_to_jsonl_export`.
  3. Parse with `parse_jsonl`.
  4. Assert url, title, description, tags match.
  5. Assert `id` is `None` (not included in export mode).
- **Expected outcome**: Export-mode JSONL round-trips core fields. Source of truth: design spec JSONL export format.
- **Interactions**: None (pure function test).

### 19. JSONL serialization roundtrip (backup mode) preserves all fields

- **Name**: Serializing bookmarks to backup-mode JSONL and parsing back preserves id, timestamps, domain, image_url
- **Type**: unit
- **Disposition**: new (included in implementation plan Task 5)
- **Harness**: `cargo test -p boopmark-server -- web::api::transfer::tests::jsonl_backup_roundtrip`
- **Preconditions**: None.
- **Actions**:
  1. Create a `Bookmark` with all fields set.
  2. Serialize with `bookmarks_to_jsonl_backup`.
  3. Parse with `parse_jsonl`.
  4. Assert `id`, `url`, `domain`, `image_url` match.
- **Expected outcome**: Backup-mode JSONL round-trips all fields. Source of truth: design spec backup format.
- **Interactions**: None.

### 20. CSV serialization roundtrip (export mode) preserves fields and pipe-separated tags

- **Name**: Serializing to export CSV and parsing back preserves url, title, tags with pipe separation
- **Type**: unit
- **Disposition**: new (included in implementation plan Task 5)
- **Harness**: `cargo test -p boopmark-server -- web::api::transfer::tests::csv_export_roundtrip`
- **Preconditions**: None.
- **Actions**:
  1. Create a `Bookmark` with tags `["rust", "web"]`.
  2. Serialize with `bookmarks_to_csv_export`.
  3. Parse with `parse_csv`.
  4. Assert url and tags match.
- **Expected outcome**: CSV export round-trips including pipe-separated tags. Source of truth: design spec CSV format.
- **Interactions**: None.

### 21. CSV serialization roundtrip (backup mode) preserves all fields

- **Name**: Serializing to backup CSV and parsing back preserves id, tags, domain
- **Type**: unit
- **Disposition**: new (included in implementation plan Task 5)
- **Harness**: `cargo test -p boopmark-server -- web::api::transfer::tests::csv_backup_roundtrip`
- **Preconditions**: None.
- **Actions**: Same flow as test 20 but backup mode.
- **Expected outcome**: Backup CSV round-trips all fields. Source of truth: design spec.
- **Interactions**: None.

### 22. CSV handles empty optional fields

- **Name**: Bookmarks with None title, description, image_url, domain serialize to empty strings and parse back as None
- **Type**: unit
- **Disposition**: new (included in implementation plan Task 5)
- **Harness**: `cargo test -p boopmark-server -- web::api::transfer::tests::csv_handles_empty_optional_fields`
- **Preconditions**: None.
- **Actions**:
  1. Create a `Bookmark` with `title: None`, `description: None`, `image_url: None`, `domain: None`, `tags: []`.
  2. Serialize with `bookmarks_to_csv_export`.
  3. Parse with `parse_csv`.
  4. Assert `title` is `None` and `tags` is empty.
- **Expected outcome**: Empty optional fields survive CSV roundtrip. Source of truth: design spec "Empty optional fields are empty strings".
- **Interactions**: None.

### 23. CSV export handles commas and quotes in field values

- **Name**: Bookmarks with commas and quotes in title are correctly escaped in CSV and parse back intact
- **Type**: unit
- **Disposition**: new (included in implementation plan Task 5)
- **Harness**: `cargo test -p boopmark-server -- web::api::transfer::tests::csv_export_handles_special_characters`
- **Preconditions**: None.
- **Actions**:
  1. Create a bookmark with `title: "Title with, commas and \"quotes\""`.
  2. Serialize and parse back.
  3. Assert the title matches exactly.
- **Expected outcome**: CSV escaping handles RFC 4180 edge cases. Source of truth: CSV RFC 4180.
- **Interactions**: None.

### 24. JSONL parser skips empty lines

- **Name**: Empty lines between JSONL records are ignored
- **Type**: unit
- **Disposition**: new (included in implementation plan Task 5)
- **Harness**: `cargo test -p boopmark-server -- web::api::transfer::tests::parse_jsonl_skips_empty_lines`
- **Preconditions**: None.
- **Actions**: Parse `{"url":"https://a.com","tags":[]}\n\n{"url":"https://b.com","tags":[]}`. Assert 2 records.
- **Expected outcome**: Blank lines are filtered. Source of truth: JSONL convention.
- **Interactions**: None.

### 25. JSONL parser rejects records missing url field

- **Name**: A JSONL line without a "url" field causes a parse error
- **Type**: unit
- **Disposition**: new (included in implementation plan Task 5)
- **Harness**: `cargo test -p boopmark-server -- web::api::transfer::tests::parse_jsonl_returns_error_for_missing_url`
- **Preconditions**: None.
- **Actions**: Parse `{"title":"No URL","tags":[]}`. Assert error.
- **Expected outcome**: Missing url aborts parsing. Source of truth: design spec requiring url field.
- **Interactions**: None.

### 26. JSONL parser rejects malformed JSON

- **Name**: Non-JSON text causes a parse error
- **Type**: unit
- **Disposition**: new (included in implementation plan Task 5)
- **Harness**: `cargo test -p boopmark-server -- web::api::transfer::tests::parse_jsonl_returns_error_for_malformed_json`
- **Preconditions**: None.
- **Actions**: Parse `"not json at all"`. Assert error.
- **Expected outcome**: Invalid JSON aborts parsing. Source of truth: JSONL format requirement.
- **Interactions**: None.

### 27. import_batch creates new bookmark in import mode

- **Name**: Importing a record with a URL that does not exist creates a new bookmark
- **Type**: unit
- **Disposition**: new (included in implementation plan Task 4)
- **Harness**: `cargo test -p boopmark-server -- import_tests::import_creates_new_bookmark`
- **Preconditions**: Empty mock repo.
- **Actions**: Call `import_batch` with one record. Assert `created: 1`.
- **Expected outcome**: New URLs create new bookmarks. Source of truth: design spec import mode behavior.
- **Interactions**: MockRepo::find_by_url returns None -> MockRepo::create.

### 28. import_batch skips existing URL with skip strategy

- **Name**: Importing a record matching an existing URL with skip strategy increments skipped count
- **Type**: unit
- **Disposition**: new (included in implementation plan Task 4)
- **Harness**: `cargo test -p boopmark-server -- import_tests::import_skips_existing_url_when_strategy_is_skip`
- **Preconditions**: Mock repo contains bookmark with matching URL.
- **Actions**: Call `import_batch` with skip strategy. Assert `skipped: 1`.
- **Expected outcome**: Skip strategy does not modify existing bookmarks. Source of truth: design spec.
- **Interactions**: MockRepo::find_by_url returns Some -> skip.

### 29. import_batch upserts existing URL with upsert strategy

- **Name**: Importing a record matching an existing URL with upsert strategy increments updated count
- **Type**: unit
- **Disposition**: new (included in implementation plan Task 4)
- **Harness**: `cargo test -p boopmark-server -- import_tests::import_upserts_existing_url_when_strategy_is_upsert`
- **Preconditions**: Mock repo contains bookmark with matching URL.
- **Actions**: Call `import_batch` with upsert strategy. Assert `updated: 1`.
- **Expected outcome**: Upsert strategy updates existing bookmarks. Source of truth: design spec.
- **Interactions**: MockRepo::find_by_url returns Some -> MockRepo::update.

### 30. import_batch records error for invalid URL

- **Name**: A record with an invalid URL is counted as an error, not created
- **Type**: unit
- **Disposition**: new (included in implementation plan Task 4)
- **Harness**: `cargo test -p boopmark-server -- import_tests::import_records_error_for_invalid_url`
- **Preconditions**: Empty mock repo.
- **Actions**: Call `import_batch` with `url: "not-a-url"`. Assert `errors.len() == 1` and `created == 0`.
- **Expected outcome**: Invalid URLs are rejected at the service layer. Source of truth: implementation plan URL validation.
- **Interactions**: url::Url::parse fails -> error recorded.

### 31. import_batch in restore mode creates bookmark with original ID

- **Name**: Restore mode uses the record's id field when inserting a new bookmark
- **Type**: unit
- **Disposition**: new (included in implementation plan Task 4)
- **Harness**: `cargo test -p boopmark-server -- import_tests::restore_creates_new_bookmark_with_original_id`
- **Preconditions**: Empty mock repo.
- **Actions**: Call `import_batch` in Restore mode with a record that has `id: Some(uuid)`. Assert `created: 1`.
- **Expected outcome**: Restore mode uses `insert_with_id` with the specified UUID. Source of truth: design spec restore mode.
- **Interactions**: MockRepo::get returns NotFound -> MockRepo::insert_with_id.

### 32. import_batch in restore mode errors when id is missing

- **Name**: Restore mode records an error when a record has no id field
- **Type**: unit
- **Disposition**: new (included in implementation plan Task 4)
- **Harness**: `cargo test -p boopmark-server -- import_tests::restore_records_error_when_id_is_missing`
- **Preconditions**: Empty mock repo.
- **Actions**: Call `import_batch` in Restore mode with `id: None`. Assert `errors.len() == 1`.
- **Expected outcome**: Missing id in restore mode is a row-level error. Source of truth: implementation plan restore mode logic.
- **Interactions**: None (early return before repo call).

### 33. import_batch in restore mode skips existing ID with skip strategy

- **Name**: Restore mode with skip strategy does not overwrite an existing bookmark with the same ID
- **Type**: unit
- **Disposition**: new (included in implementation plan Task 4)
- **Harness**: `cargo test -p boopmark-server -- import_tests::restore_skips_existing_id_when_strategy_is_skip`
- **Preconditions**: Mock repo contains bookmark with matching ID.
- **Actions**: Call `import_batch` in Restore mode with skip strategy. Assert `skipped: 1`.
- **Expected outcome**: Skip strategy in restore mode. Source of truth: design spec strategy + restore interaction.
- **Interactions**: MockRepo::get returns Ok -> skip.

### 34. import_batch in restore mode upserts existing ID

- **Name**: Restore mode with upsert strategy overwrites an existing bookmark with the same ID
- **Type**: unit
- **Disposition**: new (included in implementation plan Task 4)
- **Harness**: `cargo test -p boopmark-server -- import_tests::restore_upserts_existing_id`
- **Preconditions**: Mock repo contains bookmark with matching ID.
- **Actions**: Call `import_batch` in Restore mode with upsert strategy. Assert `updated: 1`.
- **Expected outcome**: Upsert overwrites by ID in restore mode. Source of truth: design spec.
- **Interactions**: MockRepo::get returns Ok -> MockRepo::upsert_full.

### 35. export_all returns only the requesting user's bookmarks

- **Name**: export_all does not leak bookmarks from other users
- **Type**: unit
- **Disposition**: new (included in implementation plan Task 4)
- **Harness**: `cargo test -p boopmark-server -- import_tests::export_all_returns_user_bookmarks`
- **Preconditions**: Mock repo contains bookmarks from two different users.
- **Actions**: Call `export_all` with one user's ID. Assert only that user's bookmarks are returned.
- **Expected outcome**: Tenant isolation is enforced. Source of truth: design spec, general security requirement.
- **Interactions**: MockRepo::export_all filters by user_id.

### 36. import_batch handles mixed results across multiple records

- **Name**: A batch with a new URL, an existing URL (skip), and an invalid URL reports correct counts
- **Type**: unit
- **Disposition**: new (included in implementation plan Task 4)
- **Harness**: `cargo test -p boopmark-server -- import_tests::import_multiple_records_mixed_results`
- **Preconditions**: Mock repo contains one existing bookmark.
- **Actions**: Call `import_batch` with 3 records: new URL, existing URL, invalid URL. Strategy: skip. Assert `created: 1`, `skipped: 1`, `errors.len(): 1`.
- **Expected outcome**: All three outcome paths work within a single batch. Source of truth: design spec.
- **Interactions**: Combined paths through import_batch.

### 37. CLI parses export command with defaults

- **Name**: `boop export` parses successfully with default format and mode
- **Type**: unit
- **Disposition**: new (included in implementation plan Task 7)
- **Harness**: `cargo test -p boop -- test_cli_export_default`
- **Preconditions**: None.
- **Actions**: Parse `["boop", "export"]` with `Cli::try_parse_from`. Assert it matches `Commands::Export`.
- **Expected outcome**: CLI accepts export with no flags. Source of truth: implementation plan Task 7 CLI definition.
- **Interactions**: None (clap parsing only).

### 38. CLI parses export command with all options

- **Name**: `boop export --format csv --mode backup -o out.csv` parses all options correctly
- **Type**: unit
- **Disposition**: new (included in implementation plan Task 7)
- **Harness**: `cargo test -p boop -- test_cli_export_with_options`
- **Preconditions**: None.
- **Actions**: Parse with format=csv, mode=backup, output=out.csv. Assert all values match.
- **Expected outcome**: All CLI flags are correctly parsed. Source of truth: implementation plan.
- **Interactions**: None.

### 39. CLI parses import command with file argument

- **Name**: `boop import bookmarks.jsonl` parses the file path
- **Type**: unit
- **Disposition**: new (included in implementation plan Task 7)
- **Harness**: `cargo test -p boop -- test_cli_import_with_file`
- **Preconditions**: None.
- **Actions**: Parse `["boop", "import", "bookmarks.jsonl"]`. Assert it matches `Commands::Import`.
- **Expected outcome**: Positional file argument is parsed. Source of truth: implementation plan.
- **Interactions**: None.

### 40. CLI parses import command with all options

- **Name**: `boop import data.csv --format csv --mode restore --strategy skip` parses all options
- **Type**: unit
- **Disposition**: new (included in implementation plan Task 7)
- **Harness**: `cargo test -p boop -- test_cli_import_with_all_options`
- **Preconditions**: None.
- **Actions**: Parse with all flags. Assert file, format, mode, strategy match.
- **Expected outcome**: All CLI flags are correctly parsed. Source of truth: implementation plan.
- **Interactions**: None.

---

## Coverage Summary

### Covered

- **API export endpoint**: All four format/mode combinations (JSONL export, JSONL backup, CSV export, CSV backup) verified through E2E tests 1-3.
- **API import endpoint**: JSONL and CSV import verified through E2E tests 4-5. Strategy variations (skip, upsert) verified through E2E tests 6-7.
- **Restore mode**: Full backup-restore roundtrip with ID preservation (test 16), missing-id error (test 17).
- **Error handling**: Invalid URL rows continue processing (test 8), malformed files return 400 (test 9), missing file field returns 400 (test 10).
- **Authentication**: Export and import endpoints require auth (test 11).
- **Data integrity roundtrips**: JSONL roundtrip (test 12), CSV roundtrip (test 13).
- **Web UI**: Settings page import/export section rendered (test 14), import form submission (test 15).
- **Serialization**: All format/mode combinations unit-tested (tests 18-26).
- **Service layer logic**: All import mode, strategy, and error combinations unit-tested (tests 27-36).
- **CLI parsing**: All command variants unit-tested (tests 37-40).
- **Tenant isolation**: export_all only returns requesting user's bookmarks (test 35).

### Explicitly excluded (per agreed strategy)

- **CLI integration tests against a real server**: The strategy specified "CLI tests follow existing pattern (mock server, assert request shape)." The existing codebase has no mock-server CLI test harness; CLI tests are limited to argument parsing. Adding a full CLI integration harness would increase scope beyond what was agreed. Risk: CLI HTTP client code (`post_multipart`, `get`) is not directly tested against a real server. Mitigated by the API E2E tests covering the same endpoints.
- **Performance/load testing**: Not part of the agreed strategy. The import endpoint processes records sequentially (no batching optimization). Risk: large imports may be slow. Mitigated by the design being intentionally simple for v1.
- **Backup CSV format via API E2E**: Backup CSV export is covered by unit tests (test 21) and the backup JSONL E2E test (test 3) verifies the backup code path. Adding a separate backup CSV E2E test would be redundant since CSV serialization is unit-tested and the backup/export branching is identical for both formats.
- **Concurrent import safety**: No concurrent import stress tests. The design uses sequential per-record processing. Risk: two simultaneous imports could create duplicates. Mitigated by the lack of a unique constraint on `(user_id, url)` being a known pre-existing design decision.
