# API Key Management Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Add API key management UI to the Settings page and complete the REST API with GET and DELETE endpoints, with full E2E test coverage.

**Architecture:** The app layer (`AuthService`) already has complete API key CRUD. This plan wires up two missing REST endpoints (`GET /api/v1/auth/keys`, `DELETE /api/v1/auth/keys/{id}`), adds HTMX-driven API key management to the Settings page using page-level routes (`POST /settings/api-keys`, `DELETE /settings/api-keys/{id}`) that return HTML fragments, and adds a new Playwright E2E spec. The Settings template gains a new "API Keys" section below the existing "LLM Integration" section. The legacy `GET /settings/api-keys` redirect is replaced with the HTMX POST route.

**Tech Stack:** Rust, Axum 0.8, Askama 0.12, HTMX 2, Tailwind CSS 4, SQLx/PostgreSQL, Playwright

---

## Key Design Decisions

**1. HTMX fragment routes vs. REST API for the UI.** The Settings UI uses HTMX page-level routes (`POST /settings/api-keys`, `DELETE /settings/api-keys/{id}`) that return HTML fragments, following the same pattern as the existing LLM settings form. The REST API endpoints are separate and return JSON. This is the idiomatic HTMX approach and matches the existing codebase pattern.

**2. Legacy redirect removal.** The existing `GET /settings/api-keys` route is a redirect to `/settings`. This will be replaced: `GET /settings/api-keys` will no longer exist as a standalone page route. Instead, `POST /settings/api-keys` and `DELETE /settings/api-keys/{id}` will serve HTMX fragments. The legacy redirect handler and its E2E test will be removed, and the unauth E2E test will be updated to remove the `legacyResponse` assertion since that route no longer exists.

**3. Raw key display pattern.** When a key is created, the HTMX response returns a fragment that includes the raw key in a prominent warning box plus the updated key list. Subsequent list renders never include the raw key (it is not stored). The create endpoint returns an HTML fragment containing both the one-time key display and the updated key list.

**4. Template structure.** The API Keys section will be added directly to `templates/settings/index.html`. An Askama partial `templates/settings/api_keys_list.html` will hold the key list fragment, reused by both the full page render and the HTMX responses. A second partial `templates/settings/api_keys_created.html` will hold the one-time key display plus the key list (via include), returned only on create.

**5. HTMX swap targeting.** The HTMX create form and delete buttons target a `<div id="api-keys-result">` wrapper that contains only the key list (and, after create, the one-time notice). This wrapper sits below the create form inside the API Keys section. This ensures the section header and create form are never replaced by HTMX swaps. The create response (`api_keys_created.html`) returns notice + list inside the wrapper. The delete response (`api_keys_list.html`) returns just the list inside the wrapper. Both use `hx-swap="innerHTML"` targeting `#api-keys-result"`.

**6. E2E test for API auth.** The E2E test will create an API key through the UI, extract the raw key from the page, then use it in a `fetch()` call with `Authorization: Bearer <key>` to hit `GET /api/v1/bookmarks` and verify a 200 response.

---

### Task 1: Add GET and DELETE REST API endpoints

Wire up the two missing JSON API endpoints for API key management. The app layer already has `list_api_keys` and `delete_api_key`.

**Files:**
- Modify: `server/src/web/api/auth.rs`

**Step 1: Write the list and delete handlers and response types**

Add to `server/src/web/api/auth.rs`:

```rust
use axum::extract::Path;
use axum::routing::{delete, get};
use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Serialize)]
struct ApiKeyListItem {
    id: Uuid,
    name: String,
    created_at: DateTime<Utc>,
}

async fn list_api_keys(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    match state.auth.list_api_keys(user.id).await {
        Ok(keys) => {
            let items: Vec<ApiKeyListItem> = keys
                .into_iter()
                .map(|k| ApiKeyListItem {
                    id: k.id,
                    name: k.name,
                    created_at: k.created_at,
                })
                .collect();
            Ok(Json(items))
        }
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn delete_api_key(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match state.auth.delete_api_key(id, user.id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}
```

**Step 2: Update the routes function to include all three endpoints**

Replace the existing `routes()` function:

```rust
pub fn routes() -> Router<AppState> {
    Router::new().route("/keys", post(create_api_key).get(list_api_keys))
        .route("/keys/{id}", delete(delete_api_key))
}
```

**Step 3: Verify it compiles**

Run: `cargo build -p boopmark-server`
Expected: PASS

**Step 4: Commit**

```bash
git add server/src/web/api/auth.rs
git commit -m "feat: add GET and DELETE REST API endpoints for API keys"
```

---

### Task 2: Add API Keys section to Settings page with HTMX create/delete

Add the API Keys section to the Settings page template, create the HTMX fragment templates, add the HTMX route handlers, and wire up the routes. This is a single task because the template includes, the Askama structs, and the route handlers are interdependent and must compile together.

**Files:**
- Modify: `server/src/web/pages/settings.rs`
- Modify: `templates/settings/index.html`
- Create: `templates/settings/api_keys_list.html`
- Create: `templates/settings/api_keys_created.html`

**Step 1: Create the API keys list partial template**

Create `templates/settings/api_keys_list.html`:

```html
{% if api_keys.is_empty() %}
<p class="text-sm text-gray-400" data-testid="no-api-keys">No API keys yet.</p>
{% else %}
<div class="space-y-2">
    {% for key in api_keys %}
    <div class="flex items-center justify-between rounded-lg border border-gray-700 bg-[#1a1d2e] px-4 py-3" data-testid="api-key-row">
        <div>
            <p class="text-sm font-medium text-gray-200" data-testid="api-key-name">{{ key.name }}</p>
            <p class="text-xs text-gray-400">Created {{ key.created_at_display }}</p>
        </div>
        <button
            hx-delete="/settings/api-keys/{{ key.id }}"
            hx-target="#api-keys-result"
            hx-swap="innerHTML"
            class="text-sm text-red-400 hover:text-red-300"
            data-testid="delete-api-key"
        >Delete</button>
    </div>
    {% endfor %}
</div>
{% endif %}
```

Note: No wrapping `<div id="api-keys-list">` here — the partial is always rendered inside `#api-keys-result` which provides the stable swap target. The delete button targets `#api-keys-result` and swaps innerHTML, which replaces the content with the updated list returned by the delete handler.

**Step 2: Create the "key created" response template**

Create `templates/settings/api_keys_created.html`:

```html
<div class="rounded-lg border border-amber-500/40 bg-amber-500/10 px-4 py-3 mb-4" data-testid="api-key-created-notice">
    <p class="text-sm font-medium text-amber-200 mb-1">API key created. Copy it now — it won't be shown again.</p>
    <code class="block text-sm text-amber-100 bg-[#1a1d2e] rounded px-3 py-2 font-mono break-all select-all" data-testid="api-key-raw-value">{{ raw_key }}</code>
</div>
{% include "settings/api_keys_list.html" %}
```

This template returns the one-time key notice followed by the key list. It is rendered inside `#api-keys-result` via `hx-swap="innerHTML"`.

**Step 3: Add the API Keys section to the Settings template**

Add the following section to `templates/settings/index.html` after the closing `</section>` of the LLM Integration section and before the closing `</div>` of the card container:

```html
        <section class="space-y-5">
            <div>
                <h2 class="text-lg font-semibold">API Keys</h2>
                <p class="text-sm text-gray-400">Create keys to use the Boopmark API and CLI.</p>
            </div>

            <form hx-post="/settings/api-keys" hx-target="#api-keys-result" hx-swap="innerHTML" class="flex gap-3" data-testid="create-api-key-form">
                <input
                    type="text"
                    name="name"
                    placeholder="Key name (e.g. laptop, ci)"
                    required
                    class="flex-1 px-4 py-2 rounded-lg bg-[#1a1d2e] border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500 text-sm"
                    data-testid="api-key-name-input"
                >
                <button type="submit" class="px-4 py-2 bg-blue-600 hover:bg-blue-700 rounded-lg text-sm font-medium" data-testid="create-api-key-button">
                    Create key
                </button>
            </form>

            <div id="api-keys-result">
                {% include "settings/api_keys_list.html" %}
            </div>
        </section>
```

The `<div id="api-keys-result">` is the stable HTMX swap target. The create form and delete buttons both target this div. The section header and form sit outside it and are never affected by swaps.

**Step 4: Add Askama template structs and HTMX handlers to settings.rs**

Add the following to `server/src/web/pages/settings.rs`:

```rust
use axum::extract::Path;
use uuid::Uuid;

struct ApiKeyView {
    id: String,
    name: String,
    created_at_display: String,
}

impl From<crate::domain::ports::api_key_repo::ApiKey> for ApiKeyView {
    fn from(k: crate::domain::ports::api_key_repo::ApiKey) -> Self {
        Self {
            id: k.id.to_string(),
            name: k.name,
            created_at_display: k.created_at.format("%b %d, %Y").to_string(),
        }
    }
}

#[derive(Template)]
#[template(path = "settings/api_keys_list.html")]
struct ApiKeysListFragment {
    api_keys: Vec<ApiKeyView>,
}

#[derive(Template)]
#[template(path = "settings/api_keys_created.html")]
struct ApiKeysCreatedFragment {
    raw_key: String,
    api_keys: Vec<ApiKeyView>,
}

#[derive(Deserialize)]
struct CreateApiKeyForm {
    name: String,
}

async fn create_api_key_htmx(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Form(form): Form<CreateApiKeyForm>,
) -> axum::response::Response {
    let name = form.name.trim().to_string();
    if name.is_empty() {
        return axum::http::StatusCode::BAD_REQUEST.into_response();
    }

    match state.auth.create_api_key(user.id, &name).await {
        Ok(raw_key) => {
            let keys = state.auth.list_api_keys(user.id).await.unwrap_or_default();
            let api_keys: Vec<ApiKeyView> = keys.into_iter().map(Into::into).collect();
            render(&ApiKeysCreatedFragment { raw_key, api_keys })
        }
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn delete_api_key_htmx(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> axum::response::Response {
    match state.auth.delete_api_key(id, user.id).await {
        Ok(()) => {
            let keys = state.auth.list_api_keys(user.id).await.unwrap_or_default();
            let api_keys: Vec<ApiKeyView> = keys.into_iter().map(Into::into).collect();
            render(&ApiKeysListFragment { api_keys })
        }
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
```

**Step 5: Update the SettingsPage struct to include api_keys**

Add `api_keys: Vec<ApiKeyView>` to the `SettingsPage` struct:

```rust
#[derive(Template)]
#[template(path = "settings/index.html")]
struct SettingsPage {
    user: Option<UserView>,
    header_shows_bookmark_actions: bool,
    email: String,
    llm_enabled: bool,
    has_anthropic_api_key: bool,
    anthropic_model_options: Vec<ModelOptionView>,
    success_message: Option<String>,
    api_keys: Vec<ApiKeyView>,
}
```

**Step 6: Update the settings_page handler to load API keys**

Update the `settings_page` handler to also load API keys:

```rust
async fn settings_page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(query): Query<SettingsQuery>,
) -> axum::response::Response {
    let settings_result = state.settings.load(user.id).await;
    let keys_result = state.auth.list_api_keys(user.id).await;

    match (settings_result, keys_result) {
        (Ok(settings), Ok(keys)) => {
            let email = user.email.clone();
            let anthropic_model = settings.anthropic_model;
            let api_keys: Vec<ApiKeyView> = keys.into_iter().map(Into::into).collect();

            render(&SettingsPage {
                user: Some(user.into()),
                header_shows_bookmark_actions: false,
                email,
                llm_enabled: settings.enabled,
                has_anthropic_api_key: settings.has_anthropic_api_key,
                anthropic_model_options: build_model_option_views(&anthropic_model),
                success_message: query
                    .saved
                    .filter(|value| value == "1")
                    .map(|_| "Settings saved".to_string()),
                api_keys,
            })
        }
        _ => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
```

**Step 7: Update the routes function**

Remove the `legacy_api_keys_redirect` handler and replace the routes function. The `GET /settings/api-keys` legacy redirect is removed; the new HTMX routes are added:

```rust
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/settings",
            axum::routing::get(settings_page).post(save_settings),
        )
        .route(
            "/settings/api-keys",
            axum::routing::post(create_api_key_htmx),
        )
        .route(
            "/settings/api-keys/{id}",
            axum::routing::delete(delete_api_key_htmx),
        )
}
```

**Step 8: Verify it compiles**

Run: `cargo build -p boopmark-server`
Expected: PASS

**Step 9: Commit**

```bash
git add server/src/web/pages/settings.rs templates/settings/index.html templates/settings/api_keys_list.html templates/settings/api_keys_created.html
git commit -m "feat: add API Keys section to Settings page with HTMX create and delete"
```

---

### Task 3: Update existing E2E tests for the legacy redirect removal

The existing `settings.spec.js` has a test for `GET /settings/api-keys` redirecting to `/settings`. Since that route no longer exists (replaced by POST-only HTMX route), this test needs to be updated. The unauth test also asserts on that legacy route.

**Files:**
- Modify: `tests/e2e/settings.spec.js`

**Step 1: Replace the legacy redirect test**

Replace the `"legacy api keys route redirects to settings"` test (lines 112-120) with a test that verifies the API Keys section is visible on the settings page:

```js
test("settings page shows API Keys section", async ({ page }) => {
  await signIn(page);
  await page.goto("/settings");

  await expect(page.getByRole("heading", { name: "API Keys" })).toBeVisible();
  await expect(
    page.getByText("Create keys to use the Boopmark API and CLI.")
  ).toBeVisible();
  await expect(page.getByTestId("create-api-key-form")).toBeVisible();
});
```

**Step 2: Update the unauthenticated test**

In the `"unauthenticated requests cannot read or save settings"` test, remove the `legacyResponse` block (lines 126-129) and the `page.goto("/settings/api-keys")` assertion (lines 142-143), since `GET /settings/api-keys` no longer exists as a route. The remaining assertions for `GET /settings` and `POST /settings` still verify unauthenticated access is blocked.

**Step 3: Run existing settings E2E tests**

Run: `npx playwright test tests/e2e/settings.spec.js`
Expected: All tests PASS

**Step 4: Commit**

```bash
git add tests/e2e/settings.spec.js
git commit -m "test: update settings E2E tests for API Keys section"
```

---

### Task 4: Add E2E tests for API key management

Create a comprehensive E2E test file covering create, list, delete, and API auth with a created key.

**Files:**
- Create: `tests/e2e/api-keys.spec.js`

**Step 1: Write the E2E test file**

Create `tests/e2e/api-keys.spec.js`:

```js
const { test, expect } = require("@playwright/test");

async function signIn(page) {
  await page.goto("/");
  await page.getByRole("button", { name: "Sign in for E2E" }).click();
  await expect(page).toHaveURL(/\/bookmarks$/);
}

async function deleteAllApiKeys(page) {
  await page.goto("/settings");
  // Delete any existing keys one at a time
  while ((await page.getByTestId("delete-api-key").count()) > 0) {
    await page.getByTestId("delete-api-key").first().click();
    // Wait for HTMX swap to complete
    await page.waitForResponse((resp) =>
      resp.url().includes("/settings/api-keys/")
    );
  }
}

test("settings page shows API Keys section with empty state", async ({
  page,
}) => {
  await signIn(page);
  await deleteAllApiKeys(page);

  await expect(page.getByRole("heading", { name: "API Keys" })).toBeVisible();
  await expect(
    page.getByText("Create keys to use the Boopmark API and CLI.")
  ).toBeVisible();
  await expect(page.getByTestId("no-api-keys")).toBeVisible();
});

test("creating an API key shows the raw key once", async ({ page }) => {
  await signIn(page);
  await deleteAllApiKeys(page);

  await page.getByTestId("api-key-name-input").fill("test-key");
  await page.getByTestId("create-api-key-button").click();

  // Wait for HTMX response
  await expect(page.getByTestId("api-key-created-notice")).toBeVisible();
  await expect(
    page.getByText("Copy it now — it won't be shown again.")
  ).toBeVisible();

  const rawKey = await page.getByTestId("api-key-raw-value").textContent();
  expect(rawKey).toMatch(/^boop_/);

  // The key should also appear in the list
  await expect(page.getByTestId("api-key-name").first()).toHaveText(
    "test-key"
  );
});

test("created key appears in the list with correct name", async ({ page }) => {
  await signIn(page);
  await deleteAllApiKeys(page);

  // Create a key
  await page.getByTestId("api-key-name-input").fill("key-alpha");
  await page.getByTestId("create-api-key-button").click();
  await expect(page.getByTestId("api-key-created-notice")).toBeVisible();

  // Reload page to clear the one-time notice and verify persistence
  await page.goto("/settings");

  await expect(page.getByTestId("api-key-row")).toHaveCount(1);
  await expect(page.getByTestId("api-key-name").first()).toHaveText(
    "key-alpha"
  );
  // The raw key should NOT be visible after reload
  await expect(page.getByTestId("api-key-created-notice")).toHaveCount(0);
});

test("deleting a key removes it from the list", async ({ page }) => {
  await signIn(page);
  await deleteAllApiKeys(page);

  // Create a key first
  await page.getByTestId("api-key-name-input").fill("doomed-key");
  await page.getByTestId("create-api-key-button").click();
  await expect(page.getByTestId("api-key-row")).toHaveCount(1);

  // Delete it
  await page.getByTestId("delete-api-key").first().click();
  await expect(page.getByTestId("api-key-row")).toHaveCount(0);
  await expect(page.getByTestId("no-api-keys")).toBeVisible();
});

test("created API key works for REST API auth", async ({ page }) => {
  await signIn(page);
  await deleteAllApiKeys(page);

  // Create a key and capture the raw value
  await page.getByTestId("api-key-name-input").fill("api-test-key");
  await page.getByTestId("create-api-key-button").click();
  await expect(page.getByTestId("api-key-created-notice")).toBeVisible();

  const rawKey = await page.getByTestId("api-key-raw-value").textContent();

  // Use the key to call the bookmarks API
  const status = await page.evaluate(async (key) => {
    const response = await fetch("/api/v1/bookmarks", {
      headers: { Authorization: `Bearer ${key}` },
    });
    return response.status;
  }, rawKey);

  expect(status).toBe(200);
});
```

**Step 2: Run the new E2E tests**

Run: `npx playwright test tests/e2e/api-keys.spec.js`
Expected: All 5 tests PASS

**Step 3: Commit**

```bash
git add tests/e2e/api-keys.spec.js
git commit -m "test: add E2E tests for API key management"
```

---

### Task 5: Run full test suite and verify

**Step 1: Run Rust tests**

Run: `cargo test`
Expected: All tests PASS

**Step 2: Run all E2E tests**

Run: `npx playwright test`
Expected: All tests PASS (including the updated settings tests and new api-keys tests)

**Step 3: Commit any fixes if needed**

If any tests fail, fix the issues and commit.

---

## Summary of Changes

| File | Action |
|------|--------|
| `server/src/web/api/auth.rs` | Add `list_api_keys` and `delete_api_key` REST handlers + routes |
| `server/src/web/pages/settings.rs` | Add HTMX handlers, `ApiKeyView`, fragment templates, update `SettingsPage` struct and handler, replace legacy redirect with HTMX routes |
| `templates/settings/index.html` | Add "API Keys" section with create form, swap target div, and key list include |
| `templates/settings/api_keys_list.html` | New partial: key list with delete buttons and empty state |
| `templates/settings/api_keys_created.html` | New partial: one-time raw key display + key list include |
| `tests/e2e/settings.spec.js` | Replace legacy redirect test with API Keys section test, remove legacy route from unauth test |
| `tests/e2e/api-keys.spec.js` | New: 5 E2E tests for API key management |
