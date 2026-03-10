# Public API & CLI Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Add API key management UI in settings, complete the public REST API for bookmarks, and build a full-featured CLI that wraps it — all with comprehensive E2E tests.

**Architecture:** The server already has `/api/v1/bookmarks` CRUD routes and `Bearer` token auth via `AuthUser` extractor. The `api_keys` table and `AuthService` methods (create/list/delete/validate) exist. The CLI crate (`boop`) already has basic scaffolding. This plan fills the gaps: API key management in the web UI, API endpoint polish (list/delete keys, tags endpoint, proper error JSON), CLI improvements (update, get, output formats, tags), and E2E test suites for both API and CLI.

**Tech Stack:** Rust, Axum 0.8, SQLx 0.8, Askama 0.12, HTMX 2, Tailwind CSS 4, clap 4, reqwest, Playwright (API E2E), shell-based CLI E2E harness.

---

### Task 1: Add API key management endpoints to the public API

The server has `POST /api/v1/auth/keys` but is missing `GET` (list) and `DELETE /{id}` for API key management. Add them.

**Files:**
- Modify: `server/src/web/api/auth.rs`

**Step 1: Write the failing test**

No Rust integration test for this yet — we will rely on the E2E tests in Task 8. Instead, verify the route compiles and the handler signatures are correct.

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

Add the route in the `routes()` function:

```rust
.route("/tags", get(list_tags))
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
use crate::domain::ports::api_key_repo::ApiKey;

struct ApiKeyView {
    id: String,
    name: String,
    created_at: String,
    prefix: String,
}

// In SettingsPage:
api_keys: Vec<ApiKeyView>,
created_key: Option<String>,
```

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
        prefix: format!("boop_{}...", &k.key_hash[..8]),
    })
    .collect();
```

**Step 2: Add create and delete API key handlers**

Add new handlers in `server/src/web/pages/settings.rs`:

```rust
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
            let encoded = urlencoding::encode(&key);
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

Note: Add `urlencoding` dependency to `server/Cargo.toml`:

```toml
urlencoding = "2"
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
                <span class="text-xs text-gray-500 ml-2">{{ key.prefix }}</span>
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
git add server/src/web/pages/settings.rs templates/settings/index.html server/Cargo.toml
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

### Task 5: Enhance the CLI with update, get, tags, and output format support

The CLI has basic add/list/search/delete. Add: `get`, `update`, `tags`, JSON output mode, and improve error handling.

**Files:**
- Modify: `cli/src/main.rs`

**Step 1: Add new commands to the CLI**

Add these subcommands:

```rust
#[derive(Subcommand)]
enum Commands {
    /// Add a bookmark
    Add { ... }, // existing
    /// List bookmarks
    List { ... }, // existing, add --json flag
    /// Search bookmarks
    Search { ... }, // existing, add --json flag
    /// Get a bookmark by ID
    Get {
        id: String,
        #[arg(long)]
        json: bool,
    },
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
    Tags {
        #[arg(long)]
        json: bool,
    },
    /// Configure CLI
    Config { ... }, // existing
}
```

**Step 2: Implement the new handlers**

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

Add `UpdateBookmarkRequest`:

```rust
#[derive(Serialize)]
struct UpdateBookmarkRequest {
    title: Option<String>,
    description: Option<String>,
    tags: Option<Vec<String>>,
}
```

Add `Tag` response type:

```rust
#[derive(Deserialize, Serialize)]
struct Tag {
    name: String,
    count: i64,
}
```

Implement handlers for `Get`, `Update`, `Tags` in the `run` function.

Add `--json` flag to `List` and `Search` commands.

**Step 3: Improve error messages**

Make error handling return the HTTP response body when the status is not success:

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

**Step 4: Verify it compiles**

Run: `cargo check -p boop`
Expected: compiles without errors

**Step 5: Commit**

```bash
git add cli/src/main.rs
git commit -m "feat(cli): add get, update, tags commands and --json output"
```

---

### Task 6: Add `--json` output as the default format and improve DX

Agents need structured output. Add `--json` as a global flag that applies to all commands. Add `--output` flag with `table`, `json`, `plain` options for human flexibility.

**Files:**
- Modify: `cli/src/main.rs`

**Step 1: Add global output format option**

```rust
#[derive(Parser)]
#[command(name = "boop", about = "Boopmark CLI — manage your bookmarks")]
struct Cli {
    /// Output format: json, table, or plain (default: plain)
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
```

Remove per-command `--json` flags added in Task 5 and use the global flag.

**Step 2: Implement format-aware output**

For JSON format, serialize the API response directly. For plain format, use the current human-readable output.

**Step 3: Verify it compiles**

Run: `cargo check -p boop`
Expected: compiles without errors

**Step 4: Commit**

```bash
git add cli/src/main.rs
git commit -m "feat(cli): add global --output flag for json/plain format"
```

---

### Task 7: Write API E2E tests with Playwright

Test the full API lifecycle: create a key, use it to CRUD bookmarks, manage tags, delete the key.

**Files:**
- Create: `tests/e2e/api.spec.js`

**Step 1: Write the API E2E test suite**

```javascript
const { test, expect } = require("@playwright/test");

const BASE = "http://127.0.0.1:4010";
const API = `${BASE}/api/v1`;

// Helper: sign in via E2E auth and get a session cookie
async function getSessionCookie(request) {
  const resp = await request.post(`${BASE}/auth/e2e-login`, {
    failOnStatusCode: true,
  });
  const cookies = resp.headers()["set-cookie"];
  const match = cookies?.match(/session=([^;]+)/);
  if (!match) throw new Error("No session cookie returned");
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

### Task 8: Write API key management E2E tests in settings

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

### Task 9: Build the CLI E2E test harness

Create a shell-based E2E test harness that tests the CLI against a real server. The harness starts the E2E server (reuses `scripts/e2e/start-server.sh`), creates an API key, then runs the CLI through all operations.

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
BOOP="cargo run -p boop --"
CONFIG_DIR=$(mktemp -d)
export XDG_CONFIG_HOME="$CONFIG_DIR"

echo "=== Boopmark CLI E2E Tests ==="
echo ""

# Step 1: Get an API key
# Use the E2E server's session auth to create an API key
echo "Setting up: creating API key via E2E auth..."
SESSION_COOKIE=$(curl -s -c - "${SERVER_URL}/auth/e2e-login" -X POST | grep session | awk '{print $NF}')
API_KEY=$(curl -s -b "session=${SESSION_COOKIE}" "${SERVER_URL}/api/v1/auth/keys" -X POST -H "Content-Type: application/json" -d '{"name":"cli-e2e"}' | python3 -c "import sys,json; print(json.load(sys.stdin)['key'])")
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
$BOOP list --output json > /tmp/boop-test-stdout 2>/tmp/boop-test-stderr || true
BOOKMARK_ID=$(cat /tmp/boop-test-stdout | python3 -c "import sys,json; bms=json.load(sys.stdin); print([b['id'] for b in bms if b.get('title')=='CLI Test'][0])")

# Step 4: Get a bookmark
echo ""
echo "--- Get command ---"
assert_exit_code 0 $BOOP get "$BOOKMARK_ID"
assert_output_contains "CLI Test"

assert_exit_code 0 $BOOP get "$BOOKMARK_ID" --output json
assert_json_field "['title']" "CLI Test"

# Step 5: Update a bookmark
echo ""
echo "--- Update command ---"
assert_exit_code 0 $BOOP update "$BOOKMARK_ID" --title "Updated CLI Test" --tags "cli,test,updated"
assert_output_contains "Updated"

# Verify update
assert_exit_code 0 $BOOP get "$BOOKMARK_ID" --output json
assert_json_field "['title']" "Updated CLI Test"

# Step 6: List bookmarks
echo ""
echo "--- List command ---"
assert_exit_code 0 $BOOP list
assert_output_contains "Updated CLI Test"

assert_exit_code 0 $BOOP list --output json

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

assert_exit_code 0 $BOOP tags --output json

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

### Task 10: Fix E2E auth login for API key creation

The E2E test harness needs to log in via the E2E auth endpoint and get a session cookie. Verify the existing E2E auth flow (`/auth/e2e-login`) works for programmatic use (returns a `session` cookie on POST). If it doesn't exist as a POST endpoint (it may only be a form button), add a simple POST handler.

**Files:**
- Modify: `server/src/web/pages/auth.rs` (if needed)

**Step 1: Check the existing E2E auth flow**

Read `server/src/web/pages/auth.rs` to see how E2E login works.

**Step 2: Ensure POST /auth/e2e-login returns a session cookie**

If it already does, no changes needed. If it only works as a form submission with redirect, it should still set the cookie — `curl -c -` should capture it.

**Step 3: Verify and commit if changes needed**

```bash
cargo check -p boopmark-server
git add server/src/web/pages/auth.rs
git commit -m "fix(auth): ensure E2E login endpoint works for programmatic access"
```

---

### Task 11: Integration testing — run all E2E tests and fix issues

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

### Task 12: Clean up and final review

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
- `boop list [--search] [--tags] [--sort] [--output json|plain]`
- `boop search <query>`
- `boop get <id>`
- `boop update <id> [--title] [--description] [--tags]`
- `boop delete <id>`
- `boop tags`

**Step 4: Commit any final fixes**

```bash
git add -A
git commit -m "chore: final cleanup for public API and CLI"
```
