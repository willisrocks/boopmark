# Public API & CLI Test Plan

## Harness requirements

### Harness 1: Playwright APIRequestContext (API E2E)

- **What it does:** Exercises the REST API endpoints directly via HTTP requests without a browser. Uses Playwright's built-in `APIRequestContext` available in test fixtures as `request`.
- **What it exposes:** Full HTTP request/response inspection (status codes, headers, JSON bodies). Session token acquisition via `POST /api/v1/auth/test-token`.
- **Estimated complexity:** Low. Playwright already provides `request` in test fixtures. Only a helper function for auth token + API key creation is needed.
- **Tests that depend on it:** Tests 1-8 (all API E2E tests).
- **Server lifecycle:** Managed by Playwright's `webServer` config, which starts `scripts/e2e/start-server.sh` on port 4010 with `ENABLE_E2E_AUTH=1`.

### Harness 2: Playwright browser context (Settings UI E2E)

- **What it does:** Drives the browser to test the API key management UI in the settings page.
- **What it exposes:** DOM assertions via Playwright locators, page navigation, form submission.
- **Estimated complexity:** Low. Reuses the existing `signIn()` helper from `tests/e2e/settings.spec.js`.
- **Tests that depend on it:** Tests 9-11 (settings UI tests).

### Harness 3: Playwright + execFileSync (CLI E2E)

- **What it does:** Builds the `boop` CLI binary once, creates an isolated temporary config directory via `BOOP_CONFIG_DIR`, obtains an API key via the API, then shells out to the binary for each operation.
- **What it exposes:** Exit codes, stdout, stderr from each CLI invocation. JSON-parseable output when `--output json` is used.
- **Estimated complexity:** Medium. Requires: (a) `cargo build -p boop` in `beforeAll`, (b) temporary config directory management, (c) `execFileSync` wrapper functions for success and failure cases.
- **Tests that depend on it:** Tests 12-19 (all CLI E2E tests).
- **Server lifecycle:** Same Playwright `webServer` config as Harness 1.

---

## Test plan

### Scenario tests

#### Test 1: Full API bookmark CRUD lifecycle

- **Name:** Creating, reading, updating, listing, searching, and deleting a bookmark via the API returns correct data at each step
- **Type:** scenario
- **Harness:** Playwright APIRequestContext (Harness 1)
- **Preconditions:** E2E server running. Session token obtained via `POST /api/v1/auth/test-token`. API key created via `POST /api/v1/auth/keys` with session cookie auth.
- **Actions:**
  1. `POST /api/v1/bookmarks` with `{"url": "https://example.com/api-test", "title": "API Test Bookmark", "description": "Created via API", "tags": ["api", "test"]}` using `Authorization: Bearer <apiKey>`
  2. `GET /api/v1/bookmarks/<id>` using Bearer auth
  3. `PUT /api/v1/bookmarks/<id>` with `{"title": "Updated Title", "tags": ["api", "test", "updated"]}` using Bearer auth
  4. `GET /api/v1/bookmarks` using Bearer auth
  5. `GET /api/v1/bookmarks?search=Updated` using Bearer auth
  6. `GET /api/v1/bookmarks/tags` using Bearer auth
  7. `DELETE /api/v1/bookmarks/<id>` using Bearer auth
  8. `GET /api/v1/bookmarks/<id>` using Bearer auth (verify 404)
- **Expected outcome:**
  - Step 1: 201 status. Body has `id` (UUID), `url` = "https://example.com/api-test", `title` = "API Test Bookmark", `tags` = ["api", "test"]. Source: implementation plan Task 7 + existing `CreateBookmark` domain type which defines the accepted fields, and existing `create_bookmark` handler which returns 201 + bookmark JSON.
  - Step 2: 200 status. Body has same `id` and fields from creation. Source: existing `get_bookmark` handler returns 200 + bookmark JSON.
  - Step 3: 200 status. `title` = "Updated Title", `tags` = ["api", "test", "updated"]. Source: existing `update_bookmark` handler returns 200 + updated bookmark JSON; `UpdateBookmark` domain type accepts `title` and `tags`.
  - Step 4: 200 status. Array with at least 1 bookmark. Source: existing `list_bookmarks` handler.
  - Step 5: 200 status. Array contains the updated bookmark (matched by `id`). Source: existing `list_bookmarks` handler with `search` query param.
  - Step 6: 200 status. Array of `{name, count}` objects. At least one tag named "api". Source: implementation plan Task 2 + `tags_with_counts` method on `BookmarkService`.
  - Step 7: 204 status. Source: existing `delete_bookmark` handler returns `NO_CONTENT`.
  - Step 8: 404 status. Source: existing `get_bookmark` handler returns `error_response(DomainError::NotFound)` which maps to 404.
- **Interactions:** Database (Postgres), auth system (API key validation via `AuthUser` extractor).

#### Test 2: Full API key lifecycle

- **Name:** Creating an API key, listing keys, and deleting a key revokes access
- **Type:** scenario
- **Harness:** Playwright APIRequestContext (Harness 1)
- **Preconditions:** Session token obtained via `POST /api/v1/auth/test-token`.
- **Actions:**
  1. `POST /api/v1/auth/keys` with `{"name": "temp-key"}` using `Cookie: session=<token>`
  2. `GET /api/v1/auth/keys` using `Authorization: Bearer <tempKey>`
  3. Find the key entry with name "temp-key" and extract its `id`
  4. `DELETE /api/v1/auth/keys/<id>` using `Authorization: Bearer <tempKey>`
  5. `GET /api/v1/bookmarks` using `Authorization: Bearer <tempKey>`
- **Expected outcome:**
  - Step 1: 201 status. Body has `key` field matching `/^boop_/`. Source: existing `create_api_key` handler + `AuthService::create_api_key` which returns `boop_<token>`.
  - Step 2: 200 status. Array of objects with `id`, `name`, `created_at`. No `key_hash` field exposed. Source: implementation plan Task 1 + `ApiKey` struct fields.
  - Step 3: Entry with `name` = "temp-key" exists in the array.
  - Step 4: 204 status. Source: implementation plan Task 1 delete endpoint.
  - Step 5: 401 status. Source: `AuthUser` extractor calls `validate_api_key` which hashes the key and looks it up; deleted key's hash won't be found, returning `DomainError::Unauthorized`.
- **Interactions:** Database, auth system (session validation for key creation, API key validation for key usage).

#### Test 3: Full CLI bookmark CRUD lifecycle

- **Name:** Adding, listing, getting, updating, searching, and deleting a bookmark via the CLI produces correct output
- **Type:** scenario
- **Harness:** Playwright + execFileSync (Harness 3)
- **Preconditions:** CLI binary built. Isolated config directory with server URL and API key configured via `boop config set-server` and `boop config set-key`.
- **Actions:**
  1. `boop add https://example.com/cli-test --title "CLI Test" --tags cli,test`
  2. `boop --output json list` and extract the bookmark ID from the JSON array
  3. `boop get <id>`
  4. `boop --output json get <id>`
  5. `boop update <id> --title "Updated CLI Test" --tags cli,test,updated`
  6. `boop --output json get <id>`
  7. `boop list` (plain)
  8. `boop search Updated`
  9. `boop tags` (plain)
  10. `boop --output json tags`
  11. `boop delete <id>`
  12. `boop get <id>` (expect failure)
- **Expected outcome:**
  - Step 1: Exit 0. Stdout contains "Added:". Source: implementation plan Task 5 + existing `Commands::Add` handler which prints "Added:".
  - Step 2: Exit 0. Stdout is valid JSON array. Array contains entry with `title` = "CLI Test". Source: plan Task 5 JSON output format.
  - Step 3: Exit 0. Stdout contains "CLI Test". Source: plan Task 5 get command plain output.
  - Step 4: Exit 0. Stdout is valid JSON with `title` = "CLI Test". Source: plan Task 5 JSON output.
  - Step 5: Exit 0. Stdout contains "Updated". Source: plan Task 5 update command.
  - Step 6: Exit 0. JSON body has `title` = "Updated CLI Test". Source: API returns updated bookmark.
  - Step 7: Exit 0. Stdout contains "Updated CLI Test". Source: existing list plain output format.
  - Step 8: Exit 0. Stdout contains "Updated CLI Test". Source: existing search plain output.
  - Step 9: Exit 0. Stdout contains "cli". Source: plan Task 5 tags command.
  - Step 10: Exit 0. JSON array with tag object having `name` = "cli". Source: plan Task 5 tags JSON output.
  - Step 11: Exit 0. Stdout contains "Deleted". Source: existing delete handler prints "Deleted.".
  - Step 12: Non-zero exit. Source: plan Task 5 improved error handling + API returns 404 for missing bookmark.
- **Interactions:** API server (all bookmark and tags endpoints), file system (config directory).

#### Test 4: Settings UI API key create and delete flow

- **Name:** Creating and deleting API keys from the settings page shows the key value once and updates the key list
- **Type:** scenario
- **Harness:** Playwright browser context (Harness 2)
- **Preconditions:** Signed in via E2E auth button. On the settings page.
- **Actions:**
  1. Count initial `[data-testid="api-key-row"]` elements
  2. Fill `[data-testid="api-key-name-input"]` with "test-cli-key"
  3. Click `[data-testid="create-api-key-button"]`
  4. Assert `[data-testid="new-api-key-banner"]` is visible
  5. Read text from `[data-testid="new-api-key-value"]`
  6. Assert `[data-testid="api-key-row"]` count is initial + 1
  7. Assert text "test-cli-key" is visible in the key list
  8. Find the row containing "test-cli-key" and click its `[data-testid="delete-api-key-button"]`
  9. Assert `[data-testid="api-key-row"]` count is back to initial
- **Expected outcome:**
  - Step 4: Banner visible with copy-once warning. Source: implementation plan Task 3 template.
  - Step 5: Text matches `/^boop_/`. Source: `AuthService::create_api_key` prepends "boop_".
  - Step 6: One more key row than initial count. Source: plan Task 3 — key list populated from `list_api_keys`.
  - Step 9: Key count returns to initial. Source: plan Task 3 — delete form posts to `/settings/api-keys/delete`, key removed from DB.
- **Interactions:** Database (API key table), settings page handler, redirect flow.

### Integration tests

#### Test 5: API key auth grants access to bookmark endpoints

- **Name:** A valid API key in the Authorization header authenticates requests to bookmark endpoints
- **Type:** integration
- **Harness:** Playwright APIRequestContext (Harness 1)
- **Preconditions:** API key created via session auth.
- **Actions:**
  1. `GET /api/v1/bookmarks` with `Authorization: Bearer <apiKey>`
- **Expected outcome:** 200 status. Source: `AuthUser` extractor checks Bearer token via `validate_api_key`, which hashes and looks up the key.
- **Interactions:** Auth system (API key validation), bookmarks system (list endpoint).

#### Test 6: List API keys does not expose key_hash

- **Name:** The list keys endpoint returns key metadata without exposing the hash
- **Type:** integration
- **Harness:** Playwright APIRequestContext (Harness 1)
- **Preconditions:** API key created.
- **Actions:**
  1. `GET /api/v1/auth/keys` with Bearer auth
- **Expected outcome:** 200 status. Each key object has `id` (UUID), `name` (string), `created_at` (ISO datetime). No `key_hash` field. Source: implementation plan Task 1 — `ApiKeyView` struct explicitly maps only `id`, `name`, `created_at`.
- **Interactions:** Auth system, database.

#### Test 7: CLI config show displays configured values

- **Name:** `boop config show` outputs the server URL and a masked API key
- **Type:** integration
- **Harness:** Playwright + execFileSync (Harness 3)
- **Preconditions:** CLI configured with server URL and API key.
- **Actions:**
  1. `boop config show`
- **Expected outcome:** Exit 0. Stdout contains "Server:" and "API Key:". Source: existing `ConfigAction::Show` handler.
- **Interactions:** File system (config TOML read).

### Boundary and edge-case tests

#### Test 8: Unauthenticated API request returns 401 with JSON body

- **Name:** Requesting a protected endpoint without credentials returns 401 and a JSON error body
- **Type:** boundary
- **Harness:** Playwright APIRequestContext (Harness 1)
- **Preconditions:** No auth headers.
- **Actions:**
  1. `GET /api/v1/bookmarks` with no Authorization header
- **Expected outcome:** 401 status. Body is JSON `{"error": "unauthorized"}`. Source: implementation plan Task 4 — the JSON error middleware transforms bare 401 from `AuthUser` extractor into JSON. The `AuthUser` extractor returns a bare `StatusCode::UNAUTHORIZED` (no content-type header), so the middleware intercepts it and adds the JSON body.
- **Interactions:** Auth system, JSON error middleware.

#### Test 9: Invalid API key returns 401

- **Name:** An invalid Bearer token returns 401
- **Type:** boundary
- **Harness:** Playwright APIRequestContext (Harness 1)
- **Preconditions:** None.
- **Actions:**
  1. `GET /api/v1/bookmarks` with `Authorization: Bearer boop_invalid_key`
- **Expected outcome:** 401 status. Source: `AuthUser` extractor hashes the key, lookup fails, returns `UNAUTHORIZED`.
- **Interactions:** Auth system (API key validation fails).

#### Test 10: API tags endpoint returns structured tag data

- **Name:** The tags endpoint returns name and count for each tag
- **Type:** boundary
- **Harness:** Playwright APIRequestContext (Harness 1)
- **Preconditions:** At least one bookmark with tags exists (created in the CRUD lifecycle test or beforeAll).
- **Actions:**
  1. `GET /api/v1/bookmarks/tags` with Bearer auth
- **Expected outcome:** 200 status. Array of objects each having `name` (string) and `count` (integer). Source: implementation plan Task 2 + `tags_with_counts` returns `Vec<(String, i64)>`.
- **Interactions:** Database (unnest + group by query).

#### Test 11: Getting a nonexistent bookmark returns 404

- **Name:** Requesting a bookmark with a valid UUID format but no matching record returns 404
- **Type:** boundary
- **Harness:** Playwright APIRequestContext (Harness 1)
- **Preconditions:** API key created.
- **Actions:**
  1. `GET /api/v1/bookmarks/00000000-0000-0000-0000-000000000000` with Bearer auth
- **Expected outcome:** 404 status. JSON body with `{"error": "not found"}`. Source: `get_bookmark` handler returns `error_response(DomainError::NotFound)` which maps to 404 + JSON.
- **Interactions:** Database (bookmark lookup).

#### Test 12: CLI get nonexistent bookmark fails with non-zero exit

- **Name:** `boop get` with a nonexistent UUID exits with an error
- **Type:** boundary
- **Harness:** Playwright + execFileSync (Harness 3)
- **Preconditions:** CLI configured.
- **Actions:**
  1. `boop get 00000000-0000-0000-0000-000000000000`
- **Expected outcome:** Non-zero exit code. Source: implementation plan Task 5 — `check_response` extracts error body and returns `Err(...)`, which `main` prints to stderr and exits with code 1.
- **Interactions:** API server (404 response).

### Invariant tests

#### Test 13: API key is only shown once at creation time

- **Name:** The raw API key (`boop_*`) appears only in the creation response and never in list or settings responses
- **Type:** invariant
- **Harness:** Playwright APIRequestContext (Harness 1)
- **Preconditions:** API key created.
- **Actions:**
  1. `POST /api/v1/auth/keys` — capture the `key` field from the response
  2. `GET /api/v1/auth/keys` — check that no entry contains the raw key value
- **Expected outcome:**
  - Step 1: Response contains `key` matching `/^boop_/`.
  - Step 2: No object in the array has a field value equal to the raw key. `key_hash` field is absent. Source: `ApiKeyView` only exposes `id`, `name`, `created_at`. The `key_hash` is a SHA-256 digest, not the raw key.
- **Interactions:** Auth system.

#### Test 14: JSON error responses always have content-type application/json

- **Name:** Error responses from API routes include the application/json content-type header
- **Type:** invariant
- **Harness:** Playwright APIRequestContext (Harness 1)
- **Preconditions:** None.
- **Actions:**
  1. `GET /api/v1/bookmarks` with no auth (triggers 401)
- **Expected outcome:** Response has `content-type` header containing "application/json". Source: implementation plan Task 4 — middleware converts bare status codes to `(status, Json(body))` which sets the content-type.
- **Interactions:** JSON error middleware.

### Regression tests

#### Test 15: Existing settings page renders correctly with API keys section

- **Name:** The settings page still renders the LLM Integration section and now also shows the API Keys section
- **Type:** regression
- **Harness:** Playwright browser context (Harness 2)
- **Preconditions:** Signed in.
- **Actions:**
  1. Navigate to `/settings`
  2. Assert "LLM Integration" heading visible
  3. Assert "API Keys" heading visible
  4. Assert `[data-testid="api-key-name-input"]` is visible
  5. Assert `[data-testid="create-api-key-button"]` is visible
- **Expected outcome:** Both sections render. The existing LLM Integration section is not broken by the new API Keys section. Source: implementation plan Task 3 template changes add a new `<section>` after the LLM section.
- **Interactions:** Settings page handler (now loads API keys data too).

#### Test 16: Legacy /settings/api-keys route still redirects

- **Name:** The legacy API keys route continues to redirect to /settings
- **Type:** regression
- **Harness:** Playwright browser context (Harness 2)
- **Preconditions:** Signed in.
- **Actions:**
  1. Navigate to `/settings/api-keys`
- **Expected outcome:** Page URL ends with `/settings`. Settings heading visible. Source: existing `legacy_api_keys_redirect` handler returns `Redirect::to("/settings")`. This is already tested in the existing `settings.spec.js` but the test plan notes it to ensure the new routes don't shadow it.
- **Interactions:** Router (route matching order).

### Performance tests

#### Test 17: API bookmark list responds within 2 seconds

- **Name:** Listing bookmarks via the API completes in under 2 seconds
- **Type:** boundary (performance)
- **Harness:** Playwright APIRequestContext (Harness 1)
- **Preconditions:** API key created. Some bookmarks exist.
- **Actions:**
  1. Record timestamp before `GET /api/v1/bookmarks`
  2. Record timestamp after response received
- **Expected outcome:** Elapsed time < 2000ms. This is a generous threshold to catch catastrophic regressions (e.g., missing DB index, N+1 query), not to benchmark normal performance. Source: general API performance expectation for a database-backed list endpoint.
- **Interactions:** Database, network.

#### Test 18: CLI command responds within 5 seconds

- **Name:** A CLI list command completes within the 30-second timeout configured in execFileSync
- **Type:** boundary (performance)
- **Harness:** Playwright + execFileSync (Harness 3)
- **Preconditions:** CLI configured.
- **Actions:**
  1. `boop list` (timeout is 30s in the execFileSync wrapper)
- **Expected outcome:** Command completes without timeout. Source: CLI makes a single HTTP request; 30s timeout catches only catastrophic issues.
- **Interactions:** API server, network.

*Note: Tests 17 and 18 are implicit in the scenario tests (Playwright has a 60s test timeout, execFileSync has a 30s timeout). They do not need separate test functions but are documented here for completeness. The scenario tests will fail on timeout if performance degrades catastrophically.*

---

## Coverage summary

### Covered areas

| Action surface | Tests covering it |
|---|---|
| `POST /api/v1/auth/test-token` (E2E auth) | Tests 1, 2, 3 (via beforeAll) |
| `POST /api/v1/auth/keys` (create API key) | Tests 2, 4, 13 |
| `GET /api/v1/auth/keys` (list API keys) | Tests 2, 6, 13 |
| `DELETE /api/v1/auth/keys/{id}` (delete API key) | Test 2 |
| `POST /api/v1/bookmarks` (create bookmark) | Test 1 |
| `GET /api/v1/bookmarks` (list bookmarks) | Tests 1, 5, 8, 9 |
| `GET /api/v1/bookmarks/{id}` (get bookmark) | Tests 1, 11 |
| `PUT /api/v1/bookmarks/{id}` (update bookmark) | Test 1 |
| `DELETE /api/v1/bookmarks/{id}` (delete bookmark) | Test 1 |
| `GET /api/v1/bookmarks/tags` (list tags) | Tests 1, 10 |
| `POST /api/v1/bookmarks/metadata` (extract metadata) | Not covered (see exclusions) |
| Settings UI: create API key | Test 4 |
| Settings UI: view API key list | Tests 4, 15 |
| Settings UI: delete API key | Test 4 |
| Settings UI: new key banner display | Tests 4, 15 |
| CLI: `config set-server` | Test 3 (via beforeAll) |
| CLI: `config set-key` | Test 3 (via beforeAll) |
| CLI: `config show` | Test 7 |
| CLI: `add` | Test 3 |
| CLI: `list` (plain + JSON) | Test 3 |
| CLI: `get` (plain + JSON) | Tests 3, 12 |
| CLI: `update` | Test 3 |
| CLI: `delete` | Test 3 |
| CLI: `search` | Test 3 |
| CLI: `tags` (plain + JSON) | Test 3 |
| CLI: `--output json` global flag | Test 3 |
| CLI: `BOOP_CONFIG_DIR` env override | Test 3 (entire CLI E2E depends on it) |
| JSON error middleware | Tests 8, 14 |
| Auth: unauthenticated request rejection | Test 8 |
| Auth: invalid API key rejection | Test 9 |

### Explicitly excluded

| Area | Reason | Risk |
|---|---|---|
| `POST /api/v1/bookmarks/metadata` (URL metadata extraction) | Pre-existing endpoint, not changed in this feature. Would require mocking external HTTP responses or a known live URL, adding test complexity without testing new code. | Low. Endpoint already works in production. |
| Google OAuth flow | Not changed in this feature. Requires OAuth credentials. | None for this feature. |
| LLM Integration settings | Not changed in this feature. Existing tests in `settings.spec.js` cover it. | None. Regression test 15 verifies the section still renders. |
| Concurrent API key operations | Multiple simultaneous key creation/deletion. Would require a custom concurrent test harness. | Low. Database constraints handle uniqueness. |
| Rate limiting / abuse prevention | Not in scope for this feature. | Medium long-term, but not a correctness concern for initial release. |
