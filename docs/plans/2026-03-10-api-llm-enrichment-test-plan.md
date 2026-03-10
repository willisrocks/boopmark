# API & CLI LLM Enrichment — Test Plan

## Harness Requirements

### Playwright E2E harness (existing)

Used by API and CLI E2E tests. Playwright launches the E2E server on `http://127.0.0.1:4010` via `scripts/e2e/start-server.sh` with `ENABLE_E2E_AUTH=1` and `STORAGE_BACKEND=local`. Sign-in helper clicks "Sign in for E2E" button. API key creation follows the `api-keys.spec.js` pattern: sign in, delete existing keys, create a fresh key via the settings UI, capture the raw `boop_`-prefixed value from `[data-testid="api-key-raw-value"]`.

For API tests requiring bearer-only auth (no session cookie), a fresh browser context is created via `browser.newContext()`.

For CLI tests, the `boop` binary is built once with `cargo build -p boop` in `beforeAll`, then invoked via `child_process.execSync` with a temporary `$HOME` containing a `boop` config file pointing at the E2E server with the captured API key.

### Rust unit test harness (existing)

Used by CLI parsing tests and EnrichmentService unit tests. Run via `cargo test`.

**No LLM is configured in the E2E environment.** The E2E server has no `ANTHROPIC_API_KEY` in the user's LLM settings (settings are per-user and the E2E user has no saved LLM config). This means:
- `EnrichmentService.suggest()` will attempt scrape but LLM enrichment will be skipped (no API key in user settings).
- Tests must assert structural correctness (correct types, valid JSON shape) rather than specific LLM-generated values.
- Scrape-based metadata (title, description from HTML meta tags) may or may not be present depending on the target URL's HTML content.

---

## Test Plan

### 1. POST /api/v1/bookmarks/suggest returns enrichment data structure

- **Name**: Suggest endpoint returns a valid suggestion response for a given URL
- **Type**: scenario
- **Harness**: Playwright E2E (`tests/e2e/api-enrichment.spec.js`)
- **Preconditions**: User is signed in. Fresh API key created. Fresh browser context (bearer-only auth).
- **Actions**:
  1. POST to `/api/v1/bookmarks/suggest` with `Authorization: Bearer <key>`, `Content-Type: application/json`, body `{ "url": "http://127.0.0.1:4010/" }`.
- **Expected outcome**:
  - Status is 200.
  - Response body has keys: `title` (string or null), `description` (string or null), `tags` (array), `image_url` (string or null), `domain` (string or null).
  - `tags` is an array (may be empty without LLM configured).
- **Interactions**: `EnrichmentService.suggest()` -> `MetadataExtractor::extract()` (scrape), `try_llm_enrich` (skipped without user LLM settings). Bearer auth via `AuthUser` extractor.

### 2. POST /api/v1/bookmarks?suggest=true creates a bookmark with enrichment

- **Name**: Creating a bookmark with suggest=true enriches missing fields from scrape
- **Type**: scenario
- **Harness**: Playwright E2E (`tests/e2e/api-enrichment.spec.js`)
- **Preconditions**: User is signed in. Fresh API key created. Fresh browser context.
- **Actions**:
  1. POST to `/api/v1/bookmarks?suggest=true` with bearer auth, body `{ "url": "http://127.0.0.1:4010/" }`.
- **Expected outcome**:
  - Status is 201.
  - Response body has `id` (UUID string), `url` is `"http://127.0.0.1:4010/"`.
  - Bookmark was created successfully (has valid `id` and `created_at`).
- **Interactions**: `EnrichmentService.suggest()` called before `BookmarkService::create()`. Enrichment fills missing title/description/domain/image_url from scrape. `BookmarkService` may skip its own scrape if fields are already filled.

### 3. POST /api/v1/bookmarks preserves client-provided fields without suggest

- **Name**: Creating a bookmark with explicit fields preserves them exactly
- **Type**: scenario
- **Harness**: Playwright E2E (`tests/e2e/api-enrichment.spec.js`)
- **Preconditions**: User is signed in. Fresh API key created. Fresh browser context.
- **Actions**:
  1. POST to `/api/v1/bookmarks` (no `?suggest=true`) with bearer auth, body `{ "url": "https://example.com/test-preserve", "title": "My Title", "description": "My Description", "tags": ["tag1", "tag2"] }`.
- **Expected outcome**:
  - Status is 201.
  - Returned `title` is `"My Title"`.
  - Returned `description` is `"My Description"`.
  - Returned `tags` contains `"tag1"` and `"tag2"`.
- **Interactions**: `BookmarkService::create()` only. No `EnrichmentService` call (no `?suggest=true`). The existing `needs_metadata()` scrape in `BookmarkService` may fill domain/image_url but must not overwrite provided title/description/tags.

### 4. PUT /api/v1/bookmarks/{id} normal update without suggest

- **Name**: Updating a bookmark title via PUT works without enrichment
- **Type**: scenario
- **Harness**: Playwright E2E (`tests/e2e/api-enrichment.spec.js`)
- **Preconditions**: User is signed in. Fresh API key created. A bookmark exists (created in test setup).
- **Actions**:
  1. Create a bookmark via POST with explicit title "Original Title".
  2. PUT to `/api/v1/bookmarks/{id}` with bearer auth, body `{ "title": "Updated Title" }`.
- **Expected outcome**:
  - Status is 200.
  - Returned `title` is `"Updated Title"`.
- **Interactions**: `BookmarkService::update()` only. No `EnrichmentService` call. SQL `COALESCE` preserves fields not included in the update body.

### 5. PUT /api/v1/bookmarks/{id}?suggest=true enriches missing fields

- **Name**: Updating a bookmark with suggest=true fills missing fields from enrichment
- **Type**: integration
- **Harness**: Playwright E2E (`tests/e2e/api-enrichment.spec.js`)
- **Preconditions**: User is signed in. Fresh API key created. A bookmark exists with a scrapeable URL (`http://127.0.0.1:4010/`).
- **Actions**:
  1. Create a bookmark via POST with URL `http://127.0.0.1:4010/` and title "Keep This".
  2. PUT to `/api/v1/bookmarks/{id}?suggest=true` with bearer auth, body `{}`.
- **Expected outcome**:
  - Status is 200.
  - Response has valid `id` matching the created bookmark.
  - Response has `url`, `tags` (array).
  - The update pathway with enrichment completed without error.
- **Interactions**: Handler calls `BookmarkService::get()` to retrieve the existing bookmark URL, then `EnrichmentService.suggest()`, then `BookmarkService::update()`. The handler fills `None` fields in the `UpdateBookmark` from enrichment results.

### 6. All enrichment endpoints return 401 without authentication

- **Name**: Unauthenticated requests to enrichment endpoints are rejected
- **Type**: boundary
- **Harness**: Playwright E2E (`tests/e2e/api-enrichment.spec.js`)
- **Preconditions**: No authentication headers or session cookies.
- **Actions**:
  1. POST to `/api/v1/bookmarks/suggest` with body `{ "url": "https://example.com" }`, no auth header.
- **Expected outcome**:
  - Status is 401.
- **Interactions**: `AuthUser` extractor rejects the request before any handler logic runs.

### 7. POST /api/v1/bookmarks?suggest=true preserves client-provided fields over enrichment

- **Name**: Client-provided fields are not overwritten by enrichment when suggest=true
- **Type**: boundary
- **Harness**: Playwright E2E (`tests/e2e/api-enrichment.spec.js`)
- **Preconditions**: User is signed in. Fresh API key created. Fresh browser context.
- **Actions**:
  1. POST to `/api/v1/bookmarks?suggest=true` with bearer auth, body `{ "url": "http://127.0.0.1:4010/", "title": "My Custom Title", "description": "My Custom Desc", "tags": ["custom"] }`.
- **Expected outcome**:
  - Status is 201.
  - Returned `title` is `"My Custom Title"` (not overwritten by scrape).
  - Returned `description` is `"My Custom Desc"` (not overwritten by scrape).
  - Returned `tags` contains `"custom"`.
- **Interactions**: `EnrichmentService.suggest()` is called, but the handler's fill logic only fills `None`/empty fields. Since all fields are provided, none are overwritten.

### 8. boop add creates a bookmark and shows output

- **Name**: CLI add command creates a bookmark and displays confirmation with ID
- **Type**: scenario
- **Harness**: Playwright E2E (`tests/e2e/cli-enrichment.spec.js`)
- **Preconditions**: CLI binary built. Fresh API key created. Temp config file with E2E server URL and API key.
- **Actions**:
  1. Run `boop add "https://example.com/cli-test-1"`.
- **Expected outcome**:
  - Output contains "Added:".
  - Output contains "(" (UUID in parentheses).
- **Interactions**: CLI sends POST to `/api/v1/bookmarks`, receives created bookmark JSON, prints formatted output.

### 9. boop add --description passes description to API

- **Name**: CLI add command with --description includes description in created bookmark
- **Type**: scenario
- **Harness**: Playwright E2E (`tests/e2e/cli-enrichment.spec.js`)
- **Preconditions**: CLI binary built. Fresh API key created.
- **Actions**:
  1. Run `boop add "https://example.com/cli-desc-test" --description "A test description"`.
- **Expected outcome**:
  - Output contains "Added:".
  - Output contains "A test description".
- **Interactions**: CLI includes `description` field in `CreateBookmarkRequest` JSON body.

### 10. boop suggest returns suggestions without saving

- **Name**: CLI suggest command calls suggest endpoint and displays results
- **Type**: scenario
- **Harness**: Playwright E2E (`tests/e2e/cli-enrichment.spec.js`)
- **Preconditions**: CLI binary built. Fresh API key created.
- **Actions**:
  1. Run `boop suggest "http://127.0.0.1:4010/"`.
- **Expected outcome**:
  - Command exits successfully (no error).
  - Output is defined (may contain Domain, Title, etc. from scrape, or be empty if scrape returns nothing).
- **Interactions**: CLI sends POST to `/api/v1/bookmarks/suggest`, receives `SuggestResponse`, prints fields.

### 11. boop edit with explicit fields updates a bookmark

- **Name**: CLI edit command with --title and --description updates bookmark fields
- **Type**: scenario
- **Harness**: Playwright E2E (`tests/e2e/cli-enrichment.spec.js`)
- **Preconditions**: CLI binary built. Fresh API key created. A bookmark exists (created via API).
- **Actions**:
  1. Create a bookmark via API (using `page.evaluate` with bearer auth), capture its ID.
  2. Run `boop edit <id> --title "New Title" --description "New Desc"`.
- **Expected outcome**:
  - Output contains "Updated: New Title".
- **Interactions**: CLI sends PUT to `/api/v1/bookmarks/{id}`, receives updated bookmark JSON.

### 12. boop edit --suggest updates a bookmark with enrichment

- **Name**: CLI edit command with --suggest triggers enrichment on update
- **Type**: integration
- **Harness**: Playwright E2E (`tests/e2e/cli-enrichment.spec.js`)
- **Preconditions**: CLI binary built. Fresh API key created. A bookmark exists (created via API with URL `http://127.0.0.1:4010/`).
- **Actions**:
  1. Create a bookmark via API with URL `http://127.0.0.1:4010/`, capture its ID.
  2. Run `boop edit <id> --suggest`.
- **Expected outcome**:
  - Output contains "Updated:".
- **Interactions**: CLI sends PUT to `/api/v1/bookmarks/{id}?suggest=true`. Server calls `EnrichmentService.suggest()` then `BookmarkService::update()`.

### 13. boop add --suggest creates with enrichment

- **Name**: CLI add command with --suggest creates bookmark with enrichment
- **Type**: integration
- **Harness**: Playwright E2E (`tests/e2e/cli-enrichment.spec.js`)
- **Preconditions**: CLI binary built. Fresh API key created.
- **Actions**:
  1. Run `boop add "http://127.0.0.1:4010/" --suggest`.
- **Expected outcome**:
  - Output contains "Added:".
- **Interactions**: CLI sends POST to `/api/v1/bookmarks?suggest=true`. Server calls `EnrichmentService.suggest()` then `BookmarkService::create()`.

### 14. Existing suggest page handler still works after EnrichmentService migration

- **Name**: Web app suggest-on-blur flow continues to work after refactoring inline try_llm_enrich to EnrichmentService
- **Type**: regression
- **Harness**: Playwright E2E (`tests/e2e/suggest.spec.js` -- existing test, no changes)
- **Preconditions**: User is signed in via E2E auth.
- **Actions**:
  1. Run the existing `suggest.spec.js` test as-is.
- **Expected outcome**: All existing assertions pass. The suggest-on-blur flow fills title and description from scrape, the bookmark is created, the card shows the stored preview image.
- **Interactions**: Page handler `suggest()` now delegates to `EnrichmentService.suggest()` instead of calling `MetadataExtractor` and `try_llm_enrich` directly. Behavior must be identical.

### 15. CLI parses `edit` command with --suggest flag

- **Name**: CLI argument parser recognizes the edit command with --suggest
- **Type**: unit
- **Harness**: Rust unit tests (`cargo test -p boop`)
- **Preconditions**: None.
- **Actions**:
  1. Parse `["boop", "edit", "some-id", "--suggest"]` via `Cli::try_parse_from`.
- **Expected outcome**: Parses successfully. `command` matches `Commands::Edit { suggest: true, .. }`.
- **Interactions**: clap argument parsing only.

### 16. CLI parses `edit` command without --suggest

- **Name**: CLI argument parser recognizes edit with explicit fields and no --suggest
- **Type**: unit
- **Harness**: Rust unit tests (`cargo test -p boop`)
- **Preconditions**: None.
- **Actions**:
  1. Parse `["boop", "edit", "some-id", "--title", "New Title"]` via `Cli::try_parse_from`.
- **Expected outcome**: Parses successfully. `command` matches `Commands::Edit { suggest: false, .. }`.
- **Interactions**: clap argument parsing only.

### 17. CLI parses `suggest` command

- **Name**: CLI argument parser recognizes the suggest command
- **Type**: unit
- **Harness**: Rust unit tests (`cargo test -p boop`)
- **Preconditions**: None.
- **Actions**:
  1. Parse `["boop", "suggest", "https://example.com"]` via `Cli::try_parse_from`.
- **Expected outcome**: Parses successfully. `command` matches `Commands::Suggest { .. }`.
- **Interactions**: clap argument parsing only.

### 18. CLI parses `add` with --description

- **Name**: CLI argument parser recognizes --description on the add command
- **Type**: unit
- **Harness**: Rust unit tests (`cargo test -p boop`)
- **Preconditions**: None.
- **Actions**:
  1. Parse `["boop", "add", "https://example.com", "--description", "A test"]` via `Cli::try_parse_from`.
- **Expected outcome**: Parses successfully. `command` matches `Commands::Add { .. }`.
- **Interactions**: clap argument parsing only.

### 19. CLI parses `add` with --suggest

- **Name**: CLI argument parser recognizes --suggest on the add command
- **Type**: unit
- **Harness**: Rust unit tests (`cargo test -p boop`)
- **Preconditions**: None.
- **Actions**:
  1. Parse `["boop", "add", "https://example.com", "--suggest"]` via `Cli::try_parse_from`.
- **Expected outcome**: Parses successfully. `command` matches `Commands::Add { suggest: true, .. }`.
- **Interactions**: clap argument parsing only.
