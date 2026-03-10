# Public API & CLI Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Add API key management UI in settings, complete the public REST API for bookmarks, and build a full-featured CLI that wraps it — all with comprehensive E2E tests.

**Architecture:** The server already has `/api/v1/bookmarks` CRUD routes and `Bearer` token auth via `AuthUser` extractor. The `api_keys` table and `AuthService` methods (create/list/delete/validate) exist. The CLI crate (`boop`) already has basic scaffolding. This plan fills the gaps: API key management in the web UI, API endpoint polish (list/delete keys, tags endpoint, proper error JSON), CLI improvements (update, get, output formats, tags), and E2E test suites for both API and CLI.

**Tech Stack:** Rust, Axum 0.8, SQLx 0.8, Askama 0.12, HTMX 2, Tailwind CSS 4, clap 4, reqwest, Playwright (API E2E), shell-based CLI E2E harness.

**Key design decisions:**
- The E2E auth endpoint is `POST /auth/test-login` (not `/auth/e2e-login`). It returns a 302 redirect with a `Set-Cookie: session=...` header. Playwright's `request.post()` follows redirects and loses the `set-cookie` from the 302, so API E2E tests must use `maxRedirects: 0` to capture the cookie from the redirect response itself.
- The `key_hash` stored in the DB is a SHA-256 hex digest, not the original key. The raw key (`boop_*`) is only returned once at creation time. The settings UI will show just the key name and creation date — no prefix — since we have no way to recover the original key prefix.
- No new crate dependencies are needed for URL encoding. The codebase already has a `urlencoding()` helper in `server/src/web/pages/auth.rs` (line 298) that uses `url::form_urlencoded::byte_serialize`, which is already in `Cargo.toml` via the `url` workspace dependency.

---

### Task 1: Add API key management endpoints to the public API

The server has `POST /api/v1/auth/keys` but is missing `GET` (list) and `DELETE /{id}` for API key management. Add them.

**Files:**
- Modify: `server/src/web/api/auth.rs`

**Step 1: Write the failing test**

No Rust integration test for this yet — we will rely on the E2E tests in Task 7. Instead, verify the route compiles and the handler signatures are correct.

**Step 2: Implement list and delete endpoints**

In `server/src/web/api/auth.rs`, add:

```rust
use axum::extract::Path;
use axum::routing::{get, delete};
use uuid::Uuid;

#[derive(Serialize)]
struct ApiKeyView {
    id: Uuid,
    name: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

async fn list_api_keys(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    match state.auth.list_api_keys(user.id).await {
        Ok(keys) => Ok(Json(
            keys.into_iter()
                .map(|k| ApiKeyView {
                    id: k.id,
                    name: k.name,
                    created_at: k.created_at,
                })
                .collect::<Vec<_>>(),
        )),
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

Update `routes()`:

```rust
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/keys", get(list_api_keys).post(create_api_key))
        .route("/keys/{id}", delete(delete_api_key))
}
```

**Step 3: Verify it compiles**

Run: `cargo check -p boopmark-server`
Expected: compiles without errors

**Step 4: Commit**

```bash
git add server/src/web/api/auth.rs
git commit -m "feat(api): add list and delete API key endpoints"
```

---

### Task 2: Add tags endpoint to the public API

Agents and CLI users need to list available tags. Add `GET /api/v1/bookmarks/tags`.

**Files:**
- Modify: `server/src/web/api/bookmarks.rs`

**Step 1: Implement the tags handler**

```rust
async fn list_tags(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let result = with_bookmarks!(&state.bookmarks, svc => svc.tags_with_counts(user.id).await);
    match result {
        Ok(tags) => Ok(Json(
            tags.into_iter()
                .map(|(name, count)| serde_json::json!({"name": name, "count": count}))
                .collect::<Vec<_>>(),
        )),
        Err(e) => Err(error_response(e)),
    }
}
```

Add the route in the `routes()` function. **Important:** The `/tags` route must be registered before the `/{id}` route to avoid `tags` being parsed as a UUID path parameter:

```rust
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_bookmarks).post(create_bookmark))
        .route("/tags", get(list_tags))
        .route("/metadata", post(extract_metadata))
        .route(
            "/{id}",
            get(get_bookmark)
                .put(update_bookmark)
                .delete(delete_bookmark),
        )
}
```

**Step 2: Verify it compiles**

Run: `cargo check -p boopmark-server`
Expected: compiles without errors

**Step 3: Commit**

```bash
git add server/src/web/api/bookmarks.rs
git commit -m "feat(api): add tags endpoint with counts"
```

---

### Task 3: Add API key management UI to settings page

Users need to create, view, and delete API keys from the settings page. Add an "API Keys" section.

**Files:**
- Modify: `templates/settings/index.html`
- Modify: `server/src/web/pages/settings.rs`

**Step 1: Update the settings page handler to pass API keys data**

In `server/src/web/pages/settings.rs`, add to the `SettingsPage` struct:

```rust
struct ApiKeyView {
    id: String,
    name: String,
    created_at: String,
}

// In SettingsPage:
api_keys: Vec<ApiKeyView>,
created_key: Option<String>,
```

Note: The `ApiKeyView` intentionally does NOT include a `prefix` field. The `key_hash` in the database is a SHA-256 hex digest of the original key, not the key itself. There is no way to recover a meaningful prefix from it. The UI will show only the key name and creation date.

Add `created_key` query param support:

```rust
#[derive(Deserialize, Default)]
struct SettingsQuery {
    saved: Option<String>,
    created_key: Option<String>,
}
```

In the `settings_page` handler, fetch API keys and pass them:

```rust
let api_keys_result = state.auth.list_api_keys(user.id).await.unwrap_or_default();
let api_keys: Vec<ApiKeyView> = api_keys_result
    .iter()
    .map(|k| ApiKeyView {
        id: k.id.to_string(),
        name: k.name.clone(),
        created_at: k.created_at.format("%Y-%m-%d").to_string(),
    })
    .collect();
```

**Step 2: Add create and delete API key handlers**

Add new handlers in `server/src/web/pages/settings.rs`. Use the existing `urlencoding()` function from `server/src/web/pages/auth.rs` — do NOT add a new crate dependency. Either move the `urlencoding` function to a shared location (e.g., a helper in `server/src/web/mod.rs`) or inline the same `url::form_urlencoded::byte_serialize` call:

```rust
fn url_encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

#[derive(Deserialize)]
struct CreateApiKeyForm {
    key_name: String,
}

async fn create_settings_api_key(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Form(form): Form<CreateApiKeyForm>,
) -> axum::response::Response {
    match state.auth.create_api_key(user.id, &form.key_name).await {
        Ok(key) => {
            let encoded = url_encode(&key);
            Redirect::to(&format!("/settings?created_key={encoded}")).into_response()
        }
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Deserialize)]
struct DeleteApiKeyForm {
    key_id: String,
}

async fn delete_settings_api_key(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Form(form): Form<DeleteApiKeyForm>,
) -> axum::response::Response {
    if let Ok(id) = form.key_id.parse::<uuid::Uuid>() {
        let _ = state.auth.delete_api_key(id, user.id).await;
    }
    Redirect::to("/settings?saved=1").into_response()
}
```

Add routes:

```rust
.route("/settings/api-keys/create", axum::routing::post(create_settings_api_key))
.route("/settings/api-keys/delete", axum::routing::post(delete_settings_api_key))
```

**Step 3: Update the settings template**

Add an "API Keys" section in `templates/settings/index.html` after the LLM Integration section's closing `</section>`, but before the parent `</div>`:

```html
<section class="space-y-5">
    <div>
        <h2 class="text-lg font-semibold">API Keys</h2>
        <p class="text-sm text-gray-400">Manage API keys for CLI and programmatic access.</p>
    </div>

    {% if let Some(key) = created_key %}
    <div class="rounded-lg border border-emerald-500/40 bg-emerald-500/10 px-4 py-3 text-sm text-emerald-200" data-testid="new-api-key-banner">
        <p class="font-medium">API key created! Copy it now — it won't be shown again:</p>
        <code class="block mt-2 px-3 py-2 bg-[#0d1117] rounded text-xs font-mono break-all select-all" data-testid="new-api-key-value">{{ key }}</code>
    </div>
    {% endif %}

    <form method="post" action="/settings/api-keys/create" class="flex gap-3">
        <input
            name="key_name"
            type="text"
            placeholder="Key name (e.g. laptop-cli)"
            required
            class="flex-1 px-4 py-2 rounded-lg bg-[#1a1d2e] border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500 text-sm"
            data-testid="api-key-name-input"
        >
        <button type="submit" class="px-4 py-2 bg-blue-600 hover:bg-blue-700 rounded-lg text-sm font-medium" data-testid="create-api-key-button">
            Create key
        </button>
    </form>

    {% if !api_keys.is_empty() %}
    <div class="space-y-2" data-testid="api-keys-list">
        {% for key in api_keys %}
        <div class="flex items-center justify-between px-4 py-3 rounded-lg border border-gray-700 bg-[#1a1d2e]" data-testid="api-key-row">
            <div>
                <span class="text-sm font-medium text-gray-200">{{ key.name }}</span>
                <span class="text-xs text-gray-500 ml-2">Created {{ key.created_at }}</span>
            </div>
            <form method="post" action="/settings/api-keys/delete" class="inline">
                <input type="hidden" name="key_id" value="{{ key.id }}">
                <button type="submit" class="text-xs text-red-400 hover:text-red-300" data-testid="delete-api-key-button">Delete</button>
            </form>
        </div>
        {% endfor %}
    </div>
    {% endif %}
</section>
```

**Step 4: Verify it compiles**

Run: `cargo check -p boopmark-server`
Expected: compiles without errors

**Step 5: Commit**

```bash
git add server/src/web/pages/settings.rs templates/settings/index.html
git commit -m "feat(settings): add API key management UI"
```

---

### Task 4: Improve the API error responses

Currently the API auth endpoint returns bare status codes on error. Make all API endpoints return consistent JSON error bodies `{"error": "message"}` and return `401` with a JSON body when the `AuthUser` extractor rejects an unauthenticated request.

**Files:**
- Modify: `server/src/web/extractors.rs`
- Modify: `server/src/web/api/auth.rs`

**Step 1: Improve AuthUser extractor rejection for JSON APIs**

The current `AuthUser` extractor returns a bare `401`. For API clients, a JSON body is much better. However, since the extractor is shared with page routes, we need to be careful. The simplest approach: create a dedicated `ApiAuthUser` extractor that returns JSON errors, or keep the current approach and let API routes handle it.

Decision: Keep the current `AuthUser` returning bare `401` — this is fine for both web and API since the status code is the primary signal. API clients check status codes. The `error_response` helper in `bookmarks.rs` already provides JSON for domain errors.

Update `server/src/web/api/auth.rs` to use JSON error bodies:

```rust
#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

// Use this in error handling for create/list/delete
```

**Step 2: Verify it compiles**

Run: `cargo check -p boopmark-server`
Expected: compiles without errors

**Step 3: Commit**

```bash
git add server/src/web/api/auth.rs server/src/web/extractors.rs
git commit -m "feat(api): consistent JSON error responses for auth endpoints"
```

---

### Task 5: Enhance the CLI with update, get, tags, global output format, and improved error handling

The CLI has basic add/list/search/delete. Add: `get`, `update`, `tags`, a global `--output` flag (json/plain), and improve error handling. This task combines all CLI feature work into a single step to avoid wasteful intermediate states.

**Files:**
- Modify: `cli/src/main.rs`

**Step 1: Add global output format and new commands**

```rust
#[derive(Parser)]
#[command(name = "boop", about = "Boopmark CLI — manage your bookmarks")]
struct Cli {
    /// Output format: json or plain (default: plain)
    #[arg(long, short, global = true, default_value = "plain")]
    output: OutputFormat,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Clone, clap::ValueEnum)]
enum OutputFormat {
    Json,
    Plain,
}

#[derive(Subcommand)]
enum Commands {
    /// Add a bookmark
    Add {
        url: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        tags: Option<String>,
    },
    /// List bookmarks
    List {
        #[arg(long)]
        search: Option<String>,
        #[arg(long)]
        tags: Option<String>,
        #[arg(long, default_value = "newest")]
        sort: String,
    },
    /// Search bookmarks
    Search { query: String },
    /// Get a bookmark by ID
    Get { id: String },
    /// Update a bookmark
    Update {
        id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        tags: Option<String>,
    },
    /// Delete a bookmark
    Delete { id: String },
    /// List all tags
    Tags,
    /// Configure CLI
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}
```

**Step 2: Implement new commands and output formatting**

Add the `put` method to `ApiClient`:

```rust
async fn put_json(
    &self,
    path: &str,
    body: &impl Serialize,
) -> Result<reqwest::Response, String> {
    self.client
        .put(self.url(path))
        .bearer_auth(&self.api_key)
        .json(body)
        .send()
        .await
        .map_err(|e| e.to_string())
}
```

Add request/response types:

```rust
#[derive(Serialize)]
struct UpdateBookmarkRequest {
    title: Option<String>,
    description: Option<String>,
    tags: Option<Vec<String>>,
}

#[derive(Deserialize, Serialize)]
struct Tag {
    name: String,
    count: i64,
}
```

**Step 3: Improve error messages**

Add a response checker that extracts error bodies:

```rust
async fn check_response(resp: reqwest::Response) -> Result<reqwest::Response, String> {
    if resp.status().is_success() {
        Ok(resp)
    } else {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Err(format!("HTTP {status}: {body}"))
    }
}
```

**Step 4: Implement format-aware output**

For JSON format, serialize the API response directly with `serde_json::to_string_pretty`. For plain format, use human-readable output. The `output` field from `Cli` is passed into the `run` function.

**Step 5: Verify it compiles**

Run: `cargo check -p boop`
Expected: compiles without errors

**Step 6: Commit**

```bash
git add cli/src/main.rs
git commit -m "feat(cli): add get, update, tags commands and global --output flag"
```

---

### Task 6: Write API E2E tests with Playwright

Test the full API lifecycle: create a key, use it to CRUD bookmarks, manage tags, delete the key.

**Important:** The E2E auth endpoint is `POST /auth/test-login` (NOT `/auth/e2e-login`). This endpoint returns a 302 redirect with a `Set-Cookie` header. Playwright's `request.post()` follows redirects by default, which means the `set-cookie` from the 302 is consumed by the redirect and may not be accessible on the final 200 response headers. Use `maxRedirects: 0` to stop at the 302 and extract the cookie directly.

**Files:**
- Create: `tests/e2e/api.spec.js`

**Step 1: Write the API E2E test suite**

```javascript
const { test, expect } = require("@playwright/test");

const BASE = "http://127.0.0.1:4010";
const API = `${BASE}/api/v1`;

// Helper: sign in via E2E test-login and get a session cookie.
// The /auth/test-login endpoint returns a 302 with Set-Cookie.
// We use maxRedirects: 0 to capture the cookie from the redirect response.
async function getSessionCookie(request) {
  const resp = await request.post(`${BASE}/auth/test-login`, {
    maxRedirects: 0,
  });
  // The server returns 302; Playwright gives us the redirect response
  expect([200, 302]).toContain(resp.status());
  const cookies = resp.headers()["set-cookie"];
  if (!cookies) {
    // Fallback: if Playwright followed the redirect and stored the cookie,
    // try reading it from the storage state
    throw new Error(
      "No set-cookie header found. The /auth/test-login endpoint should return a 302 with Set-Cookie."
    );
  }
  const match = cookies.match(/session=([^;]+)/);
  if (!match) throw new Error("No session cookie in set-cookie header");
  return match[1];
}

// Helper: create an API key using session auth
async function createApiKey(request, sessionCookie, name) {
  const resp = await request.post(`${API}/auth/keys`, {
    headers: { Cookie: `session=${sessionCookie}` },
    data: { name },
  });
  expect(resp.status()).toBe(201);
  const body = await resp.json();
  expect(body.key).toBeTruthy();
  expect(body.key).toMatch(/^boop_/);
  return body.key;
}

test.describe("Public API", () => {
  let apiKey;
  let sessionCookie;

  test.beforeAll(async ({ request }) => {
    sessionCookie = await getSessionCookie(request);
    apiKey = await createApiKey(request, sessionCookie, "e2e-test-key");
  });

  function authHeaders() {
    return { Authorization: `Bearer ${apiKey}` };
  }

  test("unauthenticated requests return 401", async ({ request }) => {
    const resp = await request.get(`${API}/bookmarks`);
    expect(resp.status()).toBe(401);
  });

  test("invalid API key returns 401", async ({ request }) => {
    const resp = await request.get(`${API}/bookmarks`, {
      headers: { Authorization: "Bearer boop_invalid_key" },
    });
    expect(resp.status()).toBe(401);
  });

  test("CRUD bookmark lifecycle", async ({ request }) => {
    // Create
    const createResp = await request.post(`${API}/bookmarks`, {
      headers: authHeaders(),
      data: {
        url: "https://example.com/api-test",
        title: "API Test Bookmark",
        description: "Created via API",
        tags: ["api", "test"],
      },
    });
    expect(createResp.status()).toBe(201);
    const bookmark = await createResp.json();
    expect(bookmark.id).toBeTruthy();
    expect(bookmark.url).toBe("https://example.com/api-test");
    expect(bookmark.title).toBe("API Test Bookmark");
    expect(bookmark.tags).toEqual(["api", "test"]);

    // Get
    const getResp = await request.get(`${API}/bookmarks/${bookmark.id}`, {
      headers: authHeaders(),
    });
    expect(getResp.status()).toBe(200);
    const fetched = await getResp.json();
    expect(fetched.id).toBe(bookmark.id);

    // Update
    const updateResp = await request.put(`${API}/bookmarks/${bookmark.id}`, {
      headers: authHeaders(),
      data: {
        title: "Updated Title",
        tags: ["api", "test", "updated"],
      },
    });
    expect(updateResp.status()).toBe(200);
    const updated = await updateResp.json();
    expect(updated.title).toBe("Updated Title");
    expect(updated.tags).toEqual(["api", "test", "updated"]);

    // List
    const listResp = await request.get(`${API}/bookmarks`, {
      headers: authHeaders(),
    });
    expect(listResp.status()).toBe(200);
    const bookmarks = await listResp.json();
    expect(bookmarks.length).toBeGreaterThanOrEqual(1);

    // Search
    const searchResp = await request.get(`${API}/bookmarks?search=Updated`, {
      headers: authHeaders(),
    });
    expect(searchResp.status()).toBe(200);
    const searched = await searchResp.json();
    expect(searched.some((b) => b.id === bookmark.id)).toBe(true);

    // Tags
    const tagsResp = await request.get(`${API}/bookmarks/tags`, {
      headers: authHeaders(),
    });
    expect(tagsResp.status()).toBe(200);
    const tags = await tagsResp.json();
    expect(tags.some((t) => t.name === "api")).toBe(true);

    // Delete
    const deleteResp = await request.delete(`${API}/bookmarks/${bookmark.id}`, {
      headers: authHeaders(),
    });
    expect(deleteResp.status()).toBe(204);

    // Verify deleted
    const verifyResp = await request.get(`${API}/bookmarks/${bookmark.id}`, {
      headers: authHeaders(),
    });
    expect(verifyResp.status()).toBe(404);
  });

  test("list API keys", async ({ request }) => {
    const resp = await request.get(`${API}/auth/keys`, {
      headers: authHeaders(),
    });
    expect(resp.status()).toBe(200);
    const keys = await resp.json();
    expect(keys.length).toBeGreaterThanOrEqual(1);
    expect(keys[0].id).toBeTruthy();
    expect(keys[0].name).toBeTruthy();
    expect(keys[0].created_at).toBeTruthy();
    // Ensure key_hash is not exposed
    expect(keys[0].key_hash).toBeUndefined();
  });

  test("delete API key", async ({ request }) => {
    // Create a temporary key
    const tempKey = await createApiKey(request, sessionCookie, "temp-key");

    // List to get ID
    const listResp = await request.get(`${API}/auth/keys`, {
      headers: { Authorization: `Bearer ${tempKey}` },
    });
    const keys = await listResp.json();
    const tempKeyEntry = keys.find((k) => k.name === "temp-key");
    expect(tempKeyEntry).toBeTruthy();

    // Delete it
    const deleteResp = await request.delete(`${API}/auth/keys/${tempKeyEntry.id}`, {
      headers: { Authorization: `Bearer ${tempKey}` },
    });
    expect(deleteResp.status()).toBe(204);

    // Verify the key no longer works
    const verifyResp = await request.get(`${API}/bookmarks`, {
      headers: { Authorization: `Bearer ${tempKey}` },
    });
    expect(verifyResp.status()).toBe(401);
  });
});
```

**Step 2: Run the test to verify it fails (no server running yet is expected)**

Run: `npx playwright test tests/e2e/api.spec.js`
Expected: Tests should either fail to connect or fail on assertions if the endpoints are not complete yet. This confirms the tests are wired up.

**Step 3: Commit**

```bash
git add tests/e2e/api.spec.js
git commit -m "test(e2e): add API endpoint E2E tests"
```

---

### Task 7: Write API key management E2E tests in settings

Test the settings UI flow: create a key, see it listed, copy the value, delete it.

**Files:**
- Modify: `tests/e2e/settings.spec.js`

**Step 1: Add API key UI tests**

Append these tests to `tests/e2e/settings.spec.js`:

```javascript
test("can create and delete API keys from settings", async ({ page }) => {
  await signIn(page);
  await page.goto("/settings");

  // Create a key
  await page.getByTestId("api-key-name-input").fill("test-cli-key");
  await page.getByTestId("create-api-key-button").click();

  // Should see the created key banner
  await expect(page.getByTestId("new-api-key-banner")).toBeVisible();
  const keyValue = await page.getByTestId("new-api-key-value").textContent();
  expect(keyValue).toMatch(/^boop_/);

  // Should see the key in the list
  await expect(page.getByTestId("api-key-row")).toHaveCount(1);
  await expect(page.getByText("test-cli-key")).toBeVisible();

  // Delete the key
  await page.getByTestId("delete-api-key-button").click();

  // Verify key is gone
  await expect(page.getByTestId("api-key-row")).toHaveCount(0);
});
```

**Step 2: Run the test**

Run: `npx playwright test tests/e2e/settings.spec.js`
Expected: All tests pass including the new one.

**Step 3: Commit**

```bash
git add tests/e2e/settings.spec.js
git commit -m "test(e2e): add API key management settings UI tests"
```

---

### Task 8: Build the CLI E2E test harness

Create a shell-based E2E test harness that tests the CLI against a real server. The harness assumes the E2E server is already running (started by Playwright or manually via `scripts/e2e/start-server.sh`), creates an API key, then runs the pre-built CLI binary through all operations.

**Important:** The E2E login endpoint is `POST /auth/test-login`. The script must build the CLI binary once upfront with `cargo build -p boop` and use the resulting `target/debug/boop` binary for all assertions, avoiding the overhead of `cargo run` on each invocation.

**Files:**
- Create: `scripts/e2e/test-cli.sh`

**Step 1: Write the CLI E2E test script**

```bash
#!/usr/bin/env bash
set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'
PASS=0
FAIL=0

assert_exit_code() {
  local expected="$1"
  shift
  local actual
  set +e
  "$@" > /tmp/boop-test-stdout 2>/tmp/boop-test-stderr
  actual=$?
  set -e
  if [ "$actual" -ne "$expected" ]; then
    echo -e "${RED}FAIL${NC}: Expected exit code $expected, got $actual"
    echo "  Command: $*"
    echo "  stdout: $(cat /tmp/boop-test-stdout)"
    echo "  stderr: $(cat /tmp/boop-test-stderr)"
    FAIL=$((FAIL + 1))
    return 1
  fi
  PASS=$((PASS + 1))
  return 0
}

assert_output_contains() {
  local pattern="$1"
  if ! grep -q "$pattern" /tmp/boop-test-stdout; then
    echo -e "${RED}FAIL${NC}: Output does not contain '$pattern'"
    echo "  stdout: $(cat /tmp/boop-test-stdout)"
    FAIL=$((FAIL + 1))
    return 1
  fi
  PASS=$((PASS + 1))
  return 0
}

assert_json_field() {
  local field="$1"
  local expected="$2"
  local actual
  actual=$(cat /tmp/boop-test-stdout | python3 -c "import sys,json; print(json.load(sys.stdin)$field)" 2>/dev/null || echo "PARSE_ERROR")
  if [ "$actual" != "$expected" ]; then
    echo -e "${RED}FAIL${NC}: JSON field $field expected '$expected', got '$actual'"
    FAIL=$((FAIL + 1))
    return 1
  fi
  PASS=$((PASS + 1))
  return 0
}

SERVER_URL="http://127.0.0.1:4010"

# Build the CLI binary once upfront
echo "Building CLI binary..."
cargo build -p boop
BOOP="./target/debug/boop"

# Use an isolated config directory so we don't clobber the user's real config
CONFIG_DIR=$(mktemp -d)
export XDG_CONFIG_HOME="$CONFIG_DIR"

echo "=== Boopmark CLI E2E Tests ==="
echo ""

# Step 1: Get an API key via the E2E test-login endpoint
# /auth/test-login returns a 302 with Set-Cookie; curl -c captures cookies from redirects
echo "Setting up: creating API key via E2E auth..."
COOKIE_JAR=$(mktemp)
curl -s -L -c "$COOKIE_JAR" "${SERVER_URL}/auth/test-login" -X POST > /dev/null
SESSION_COOKIE=$(grep session "$COOKIE_JAR" | awk '{print $NF}')
if [ -z "$SESSION_COOKIE" ]; then
  echo "ERROR: Failed to get session cookie from /auth/test-login"
  exit 1
fi
API_KEY=$(curl -s -b "session=${SESSION_COOKIE}" "${SERVER_URL}/api/v1/auth/keys" -X POST -H "Content-Type: application/json" -d '{"name":"cli-e2e"}' | python3 -c "import sys,json; print(json.load(sys.stdin)['key'])")
rm -f "$COOKIE_JAR"
echo "Got API key: ${API_KEY:0:12}..."

# Step 2: Configure the CLI
echo ""
echo "--- Config commands ---"
assert_exit_code 0 $BOOP config set-server "$SERVER_URL"
assert_output_contains "Server URL saved"

assert_exit_code 0 $BOOP config set-key "$API_KEY"
assert_output_contains "API key saved"

assert_exit_code 0 $BOOP config show
assert_output_contains "Server:"
assert_output_contains "API Key:"

# Step 3: Create a bookmark
echo ""
echo "--- Add command ---"
assert_exit_code 0 $BOOP add "https://example.com/cli-test" --title "CLI Test" --tags "cli,test"
assert_output_contains "Added:"

# Get the bookmark ID from JSON output
$BOOP --output json list > /tmp/boop-test-stdout 2>/tmp/boop-test-stderr || true
BOOKMARK_ID=$(cat /tmp/boop-test-stdout | python3 -c "import sys,json; bms=json.load(sys.stdin); print([b['id'] for b in bms if b.get('title')=='CLI Test'][0])")

# Step 4: Get a bookmark
echo ""
echo "--- Get command ---"
assert_exit_code 0 $BOOP get "$BOOKMARK_ID"
assert_output_contains "CLI Test"

assert_exit_code 0 $BOOP --output json get "$BOOKMARK_ID"
assert_json_field "['title']" "CLI Test"

# Step 5: Update a bookmark
echo ""
echo "--- Update command ---"
assert_exit_code 0 $BOOP update "$BOOKMARK_ID" --title "Updated CLI Test" --tags "cli,test,updated"
assert_output_contains "Updated"

# Verify update
assert_exit_code 0 $BOOP --output json get "$BOOKMARK_ID"
assert_json_field "['title']" "Updated CLI Test"

# Step 6: List bookmarks
echo ""
echo "--- List command ---"
assert_exit_code 0 $BOOP list
assert_output_contains "Updated CLI Test"

assert_exit_code 0 $BOOP --output json list

# Step 7: Search
echo ""
echo "--- Search command ---"
assert_exit_code 0 $BOOP search "Updated"
assert_output_contains "Updated CLI Test"

# Step 8: Tags
echo ""
echo "--- Tags command ---"
assert_exit_code 0 $BOOP tags
assert_output_contains "cli"

assert_exit_code 0 $BOOP --output json tags

# Step 9: Delete
echo ""
echo "--- Delete command ---"
assert_exit_code 0 $BOOP delete "$BOOKMARK_ID"
assert_output_contains "Deleted"

# Verify deleted
assert_exit_code 1 $BOOP get "$BOOKMARK_ID"

# Step 10: Error handling
echo ""
echo "--- Error handling ---"
assert_exit_code 1 $BOOP get "00000000-0000-0000-0000-000000000000"

# Summary
echo ""
echo "==========================="
echo -e "Passed: ${GREEN}${PASS}${NC}"
echo -e "Failed: ${RED}${FAIL}${NC}"
echo "==========================="

# Cleanup
rm -rf "$CONFIG_DIR"

if [ "$FAIL" -gt 0 ]; then
  exit 1
fi
```

**Step 2: Verify script is executable**

```bash
chmod +x scripts/e2e/test-cli.sh
```

**Step 3: Commit**

```bash
git add scripts/e2e/test-cli.sh
git commit -m "test(e2e): add CLI E2E test harness"
```

---

### Task 9: Integration testing — run all E2E tests and fix issues

This is the integration task. Run all tests and fix any issues found.

**Files:**
- Any files that need fixes based on test failures

**Step 1: Run `cargo test`**

Run: `cargo test`
Expected: All unit tests pass

**Step 2: Run `cargo build`**

Run: `cargo build`
Expected: Full workspace builds without errors

**Step 3: Run Playwright API E2E tests**

Run: `npx playwright test tests/e2e/api.spec.js`
Expected: All tests pass

**Step 4: Run Playwright settings E2E tests**

Run: `npx playwright test tests/e2e/settings.spec.js`
Expected: All tests pass

**Step 5: Run CLI E2E tests**

The CLI E2E test needs the E2E server to be running. Either:
- Start the server manually: `bash scripts/e2e/start-server.sh &` then run `bash scripts/e2e/test-cli.sh`
- Or add a Playwright config that starts the server, then runs the shell script

Run: `bash scripts/e2e/test-cli.sh` (with E2E server running)
Expected: All tests pass

**Step 6: Fix any issues found and commit**

```bash
git add -A
git commit -m "fix: resolve E2E test failures for API and CLI"
```

---

### Task 10: Clean up and final review

Review all changes for code quality, consistency, and completeness.

**Files:**
- All modified files

**Step 1: Run cargo clippy**

Run: `cargo clippy -- -D warnings`
Expected: No warnings

**Step 2: Verify the API surface is complete**

Checklist:
- `POST /api/v1/auth/keys` - create API key
- `GET /api/v1/auth/keys` - list API keys
- `DELETE /api/v1/auth/keys/{id}` - delete API key
- `GET /api/v1/bookmarks` - list bookmarks (with search, tags, sort, limit, offset)
- `POST /api/v1/bookmarks` - create bookmark
- `GET /api/v1/bookmarks/{id}` - get bookmark
- `PUT /api/v1/bookmarks/{id}` - update bookmark
- `DELETE /api/v1/bookmarks/{id}` - delete bookmark
- `GET /api/v1/bookmarks/tags` - list tags with counts
- `POST /api/v1/bookmarks/metadata` - extract URL metadata

**Step 3: Verify the CLI surface is complete**

Checklist:
- `boop config set-server <url>`
- `boop config set-key <key>`
- `boop config show`
- `boop add <url> [--title] [--tags]`
- `boop list [--search] [--tags] [--sort]`
- `boop search <query>`
- `boop get <id>`
- `boop update <id> [--title] [--description] [--tags]`
- `boop delete <id>`
- `boop tags`
- All commands support `--output json|plain` (global flag)

**Step 4: Commit any final fixes**

```bash
git add -A
git commit -m "chore: final cleanup for public API and CLI"
```
