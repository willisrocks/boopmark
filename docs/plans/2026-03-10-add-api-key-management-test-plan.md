# API Key Management — Test Plan

## Harness Requirements

**No new harnesses need to be built.** The existing Playwright E2E infrastructure (playwright.config.js + scripts/e2e/start-server.sh) provides everything needed:

- **E2E server**: Launched automatically by Playwright via `start-server.sh` on `http://127.0.0.1:4010` with `ENABLE_E2E_AUTH=1` for auto-login.
- **Sign-in helper**: Reusable `signIn(page)` pattern used across all existing specs — click "Sign in for E2E" button, wait for `/bookmarks` redirect.
- **Cleanup helper**: A `deleteAllApiKeys(page)` helper (defined in the new spec) navigates to `/settings` and deletes any existing keys via HTMX before each test, ensuring test isolation.
- **API auth verification**: The created API key can be tested by calling `GET /api/v1/bookmarks` with `Authorization: Bearer <key>` via `page.evaluate(fetch(...))`, using the E2E server's same origin.

All tests use the **Playwright E2E harness** and run against the E2E server with a real Postgres database.

---

## Test Plan

### 1. Full API key lifecycle via the Settings UI

- **Name**: User can create an API key, see it listed, and delete it through the Settings page
- **Type**: scenario
- **Harness**: Playwright E2E
- **Preconditions**: User is signed in. No API keys exist (cleaned up by `deleteAllApiKeys`).
- **Actions**:
  1. Navigate to `/settings`.
  2. Verify the "API Keys" section heading and description ("Create keys to use the Boopmark API and CLI.") are visible.
  3. Verify the empty state message "No API keys yet." is displayed (`[data-testid="no-api-keys"]`).
  4. Fill the key name input (`[data-testid="api-key-name-input"]`) with "my-cli-key".
  5. Click the "Create key" button (`[data-testid="create-api-key-button"]`).
  6. Verify the one-time notice appears (`[data-testid="api-key-created-notice"]`) with text containing "won't be shown again".
  7. Capture the raw key value from `[data-testid="api-key-raw-value"]` and assert it starts with `boop_`.
  8. Verify the key list now shows one row (`[data-testid="api-key-row"]` count = 1) with name "my-cli-key" (`[data-testid="api-key-name"]`).
  9. Reload the page (`page.goto("/settings")`).
  10. Verify the key still appears in the list (persistence check).
  11. Verify the one-time notice is no longer visible (`[data-testid="api-key-created-notice"]` count = 0).
  12. Click the "Delete" button (`[data-testid="delete-api-key"]`).
  13. Verify the key row is removed (`[data-testid="api-key-row"]` count = 0).
  14. Verify the empty state message reappears.
- **Expected outcome**: The full create-view-persist-delete lifecycle works without page reloads (except the explicit persistence verification reload). All assertions pass.
  - Source of truth: User requirements specifying create/list/delete behavior, one-time key display, and `boop_` prefix from `AuthService::create_api_key`.
- **Interactions**: HTMX swap mechanism (create form POST and delete button DELETE both swap `#api-keys-result` innerHTML). Postgres API key storage via `ApiKeyRepository`.

### 2. Created API key authenticates against the REST API

- **Name**: An API key created through the UI can authenticate REST API requests
- **Type**: integration
- **Harness**: Playwright E2E
- **Preconditions**: User is signed in. No API keys exist.
- **Actions**:
  1. Navigate to `/settings`.
  2. Create an API key named "api-test-key" via the UI form.
  3. Capture the raw key from `[data-testid="api-key-raw-value"]`.
  4. Use `page.evaluate()` to call `fetch("/api/v1/bookmarks", { headers: { Authorization: "Bearer <key>" } })`.
  5. Assert the response status is 200.
- **Expected outcome**: The created key is accepted by the `AuthUser` extractor's bearer token path, returning 200 from the bookmarks endpoint.
  - Source of truth: User requirement "Created API key actually works for API auth". `AuthUser` extractor in `server/src/web/extractors.rs` checks `Authorization: Bearer` header and calls `AuthService::validate_api_key`.
- **Interactions**: Crosses from HTMX UI layer (key creation) to REST API layer (bearer auth). Exercises `AuthUser` extractor, `AuthService::validate_api_key`, SHA-256 hash lookup in `ApiKeyRepository::find_by_hash`.

### 3. GET /api/v1/auth/keys returns the user's API keys without the raw key or hash

- **Name**: REST API lists API keys with id, name, and created_at but not key hash
- **Type**: integration
- **Harness**: Playwright E2E
- **Preconditions**: User is signed in via E2E auth (session cookie). At least one API key exists (created via UI or POST /api/v1/auth/keys).
- **Actions**:
  1. Sign in and create an API key named "rest-list-key" via the Settings UI.
  2. Use `page.evaluate()` to call `fetch("/api/v1/auth/keys")` (session cookie auth).
  3. Parse the JSON response.
  4. Assert the response is an array with at least 1 element.
  5. Assert each element has `id` (UUID string), `name` (string), and `created_at` (ISO datetime string) properties.
  6. Assert no element has `key_hash`, `key`, `raw_key`, or `user_id` properties.
  7. Assert one element has `name === "rest-list-key"`.
- **Expected outcome**: The list endpoint returns metadata only, not secrets. Response shape matches `ApiKeyListItem { id, name, created_at }`.
  - Source of truth: User requirement "returns list of user's API keys (id, name, created_at -- NOT the raw key or hash)". Implementation plan Task 1 `ApiKeyListItem` struct.
- **Interactions**: REST API handler `list_api_keys` -> `AuthService::list_api_keys` -> `ApiKeyRepository::list`.

### 4. DELETE /api/v1/auth/keys/{id} removes a key

- **Name**: REST API deletes an API key by ID and it no longer appears in the list
- **Type**: integration
- **Harness**: Playwright E2E
- **Preconditions**: User is signed in. One API key exists.
- **Actions**:
  1. Sign in and create an API key named "delete-me" via the Settings UI.
  2. Use `page.evaluate()` to call `GET /api/v1/auth/keys` and extract the key's `id`.
  3. Use `page.evaluate()` to call `DELETE /api/v1/auth/keys/{id}` with the extracted ID.
  4. Assert the response status is 204 (No Content).
  5. Use `page.evaluate()` to call `GET /api/v1/auth/keys` again.
  6. Assert the deleted key no longer appears in the list.
- **Expected outcome**: The key is removed and subsequent list calls do not include it.
  - Source of truth: User requirement "DELETE /api/v1/auth/keys/{id} -- deletes a key by ID (scoped to authenticated user)". 204 status from implementation plan Task 1.
- **Interactions**: REST API delete handler -> `AuthService::delete_api_key` -> `ApiKeyRepository::delete`.

### 5. Settings page shows the API Keys section structure

- **Name**: Settings page displays the API Keys section with heading, description, and create form
- **Type**: scenario
- **Harness**: Playwright E2E
- **Preconditions**: User is signed in.
- **Actions**:
  1. Navigate to `/settings`.
  2. Assert heading "API Keys" is visible.
  3. Assert description text "Create keys to use the Boopmark API and CLI." is visible.
  4. Assert the create form (`[data-testid="create-api-key-form"]`) is visible.
  5. Assert the name input (`[data-testid="api-key-name-input"]`) is visible and editable.
  6. Assert the "Create key" button (`[data-testid="create-api-key-button"]`) is visible.
- **Expected outcome**: The API Keys section renders correctly below the LLM Integration section.
  - Source of truth: User requirement for section header, description, create form with text input and button.
- **Interactions**: Askama template rendering. Settings page handler loading API keys alongside LLM settings.

### 6. Empty state displays when no API keys exist

- **Name**: Settings page shows "No API keys yet." when the user has no keys
- **Type**: boundary
- **Harness**: Playwright E2E
- **Preconditions**: User is signed in. All API keys deleted via `deleteAllApiKeys`.
- **Actions**:
  1. Navigate to `/settings`.
  2. Assert `[data-testid="no-api-keys"]` is visible with text "No API keys yet."
  3. Assert `[data-testid="api-key-row"]` count is 0.
- **Expected outcome**: The empty state message appears and no key rows are rendered.
  - Source of truth: User requirement for empty state message.
- **Interactions**: Askama conditional rendering in `api_keys_list.html`.

### 7. Multiple API keys can coexist and be individually deleted

- **Name**: Creating multiple keys shows all in the list; deleting one preserves the others
- **Type**: scenario
- **Harness**: Playwright E2E
- **Preconditions**: User is signed in. No API keys exist.
- **Actions**:
  1. Create key named "key-alpha" via the UI. Wait for HTMX response.
  2. Create key named "key-beta" via the UI. Wait for HTMX response.
  3. Assert `[data-testid="api-key-row"]` count is 2.
  4. Assert key names "key-alpha" and "key-beta" are both present in `[data-testid="api-key-name"]` elements.
  5. Delete the first key in the list by clicking its `[data-testid="delete-api-key"]` button.
  6. Assert `[data-testid="api-key-row"]` count is 1.
  7. Assert the remaining key name matches one of the two created keys.
- **Expected outcome**: Multiple keys render correctly, and deletion of one does not affect the other.
  - Source of truth: User requirement for key list showing each key's name. HTMX swap design ensuring `#api-keys-result` is fully re-rendered on each operation.
- **Interactions**: Multiple sequential HTMX create/delete operations targeting the same swap container.

### 8. Deleted API key can no longer authenticate

- **Name**: After deleting an API key, it is rejected by the REST API
- **Type**: integration
- **Harness**: Playwright E2E
- **Preconditions**: User is signed in. No API keys exist.
- **Actions**:
  1. Create an API key named "ephemeral-key" via the UI.
  2. Capture the raw key value.
  3. Verify it works: `fetch("/api/v1/bookmarks", { headers: { Authorization: "Bearer <key>" } })` returns 200.
  4. Delete the key via the UI delete button.
  5. Attempt the same fetch again.
  6. Assert the response status is 401.
- **Expected outcome**: Once deleted, the key hash is removed from the database and `validate_api_key` returns `Unauthorized`.
  - Source of truth: `AuthService::validate_api_key` returns `DomainError::Unauthorized` when `find_by_hash` returns `None`. `AuthUser` extractor maps this to 401.
- **Interactions**: Full round-trip: HTMX create -> capture raw key -> REST API auth -> HTMX delete -> REST API auth rejection.

### 9. Existing settings E2E tests still pass after legacy redirect removal

- **Name**: Updated settings tests pass with the legacy api-keys redirect replaced by the API Keys section test
- **Type**: regression
- **Harness**: Playwright E2E
- **Preconditions**: Implementation tasks 2 and 3 complete (legacy redirect removed, new routes added, settings.spec.js updated).
- **Actions**:
  1. Run `npx playwright test tests/e2e/settings.spec.js`.
  2. All tests pass.
- **Expected outcome**: The replaced "legacy api keys route redirects to settings" test now verifies the API Keys section. The unauth test no longer asserts on `GET /settings/api-keys`. All other settings tests (render, add/delete Anthropic key, forged model rejection) pass unchanged.
  - Source of truth: Implementation plan Task 3 specifying which lines to modify.
- **Interactions**: Verifies no regression in the LLM Integration settings behavior.

### 10. Unauthenticated POST /settings/api-keys returns 401

- **Name**: HTMX create endpoint rejects unauthenticated requests
- **Type**: boundary
- **Harness**: Playwright E2E
- **Preconditions**: No user signed in.
- **Actions**:
  1. Use Playwright `request.post("/settings/api-keys", { form: { name: "hacker-key" } })`.
  2. Assert response status is 401.
- **Expected outcome**: The `AuthUser` extractor rejects the request before the handler runs.
  - Source of truth: `AuthUser` extractor returns `StatusCode::UNAUTHORIZED` when no valid session or bearer token is present (`server/src/web/extractors.rs` line 52).
- **Interactions**: Axum extractor layer.

### 11. Unauthenticated DELETE /settings/api-keys/{id} returns 401

- **Name**: HTMX delete endpoint rejects unauthenticated requests
- **Type**: boundary
- **Harness**: Playwright E2E
- **Preconditions**: No user signed in.
- **Actions**:
  1. Use Playwright `request.delete("/settings/api-keys/00000000-0000-0000-0000-000000000000")`.
  2. Assert response status is 401.
- **Expected outcome**: The `AuthUser` extractor rejects the request.
  - Source of truth: Same as test 10.
- **Interactions**: Axum extractor layer.

### 12. Unauthenticated GET /api/v1/auth/keys returns 401

- **Name**: REST API list endpoint rejects unauthenticated requests
- **Type**: boundary
- **Harness**: Playwright E2E
- **Preconditions**: No user signed in.
- **Actions**:
  1. Use Playwright `request.get("/api/v1/auth/keys")`.
  2. Assert response status is 401.
- **Expected outcome**: Unauthenticated list requests are rejected.
  - Source of truth: User requirement "Both require AuthUser". Extractor behavior.
- **Interactions**: Axum extractor layer.

### 13. Unauthenticated DELETE /api/v1/auth/keys/{id} returns 401

- **Name**: REST API delete endpoint rejects unauthenticated requests
- **Type**: boundary
- **Harness**: Playwright E2E
- **Preconditions**: No user signed in.
- **Actions**:
  1. Use Playwright `request.delete("/api/v1/auth/keys/00000000-0000-0000-0000-000000000000")`.
  2. Assert response status is 401.
- **Expected outcome**: Unauthenticated delete requests are rejected.
  - Source of truth: Same as test 12.
- **Interactions**: Axum extractor layer.

---

## Coverage Summary

### Covered

| Area | Tests |
|------|-------|
| Full UI lifecycle (create, view, persist, delete) | 1, 7 |
| One-time raw key display and `boop_` prefix | 1 |
| Empty state rendering | 1, 6 |
| API key list persistence across page reloads | 1 |
| Raw key not shown after reload | 1 |
| REST API `GET /api/v1/auth/keys` response shape | 3 |
| REST API `DELETE /api/v1/auth/keys/{id}` | 4 |
| Bearer token auth with created key | 2 |
| Deleted key rejected by auth | 8 |
| Multiple keys coexistence | 7 |
| Auth boundary (unauth rejected) for all 4 new endpoints | 10, 11, 12, 13 |
| Regression: existing settings tests still pass | 9 |
| HTMX swap behavior (no page reload for create/delete) | 1, 7 |

### Explicitly Excluded

| Area | Reason | Risk |
|------|--------|------|
| Visual/pixel-level styling verification | No visual regression baseline exists in the project. Styling correctness is verified structurally via data-testid presence and text content assertions. | Low — the implementation follows existing Tailwind patterns, and structural assertions catch rendering failures. |
| Cross-user key isolation (user A cannot see/delete user B's keys) | The E2E harness uses a single auto-login user (ENABLE_E2E_AUTH=1). Testing multi-user isolation would require a custom harness or modifying the E2E auth mechanism. | Low — the `AuthService` methods accept `user_id` from the authenticated user and pass it to the repository, which scopes queries by `user_id`. The scoping is enforced at the SQL level. |
| POST /api/v1/auth/keys (REST create) | This endpoint already exists and is not being modified. | Negligible — it is tested indirectly if the implementer needs to verify the full REST surface. |
| Performance benchmarks | No performance-sensitive operations added (simple CRUD). | Negligible. |
