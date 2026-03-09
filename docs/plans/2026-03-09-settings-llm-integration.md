# Settings LLM Integration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Replace the `API Keys` stub with a real `Settings` page that persists per-user LLM integration settings, defaults Anthropic model selection to `claude-haiku-4-5`, and forwards `ANTHROPIC_API_KEY` only through the E2E bootstrap for local browser testing.

**Architecture:** Add a dedicated per-user LLM settings persistence layer backed by its own PostgreSQL table and repository instead of reusing `api_keys`. Expose a `GET /settings` page plus a `POST /settings/llm` save action, while keeping `/settings/api-keys` compatible by redirecting or serving the same settings experience so existing links do not break during rollout. Seed the page’s empty-state Anthropic key input from saved user settings only; the worktree `.env` value is forwarded solely into Playwright startup so tests can submit it explicitly without auto-populating user data.

**Tech Stack:** Rust, Axum, Askama templates, SQLx/PostgreSQL migrations, Playwright E2E, bash bootstrap scripts.

---

### Task 1: Add dedicated LLM settings persistence

**Files:**
- Create: `migrations/005_create_llm_settings.sql`
- Create: `server/src/domain/ports/llm_settings_repo.rs`
- Create: `server/src/app/llm_settings.rs`
- Create: `server/src/adapters/postgres/llm_settings_repo.rs`
- Modify: `server/src/domain/ports/mod.rs`
- Modify: `server/src/app/mod.rs`
- Modify: `server/src/adapters/postgres/mod.rs`
- Modify: `server/src/web/state.rs`
- Modify: `server/src/main.rs`

**Step 1: Write the failing migration and repository test target**

```sql
CREATE TABLE llm_settings (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    anthropic_api_key TEXT,
    anthropic_model TEXT NOT NULL DEFAULT 'claude-haiku-4-5',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

Run: `cargo test -p boopmark-server llm_settings -- --nocapture`
Expected: FAIL because the `llm_settings` module and repository do not exist yet.

**Step 2: Add the domain port and service API**

```rust
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LlmSettings {
    pub user_id: Uuid,
    pub enabled: bool,
    pub anthropic_api_key: Option<String>,
    pub anthropic_model: String,
}

#[trait_variant::make(Send)]
pub trait LlmSettingsRepository: Send + Sync {
    async fn get(&self, user_id: Uuid) -> Result<Option<LlmSettings>, DomainError>;
    async fn upsert(
        &self,
        user_id: Uuid,
        enabled: bool,
        anthropic_api_key: Option<&str>,
        anthropic_model: &str,
    ) -> Result<LlmSettings, DomainError>;
}
```

```rust
pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-haiku-4-5";
```

**Step 3: Add the Postgres adapter and wire it into shared state**

```rust
impl LlmSettingsRepository for PostgresPool {
    async fn get(&self, user_id: Uuid) -> Result<Option<LlmSettings>, DomainError> { /* ... */ }

    async fn upsert(
        &self,
        user_id: Uuid,
        enabled: bool,
        anthropic_api_key: Option<&str>,
        anthropic_model: &str,
    ) -> Result<LlmSettings, DomainError> {
        /* INSERT ... ON CONFLICT (user_id) DO UPDATE ... */
    }
}
```

```rust
pub struct AppState {
    pub bookmarks: Bookmarks,
    pub auth: Arc<AuthService<PostgresPool, PostgresPool, PostgresPool>>,
    pub llm_settings: Arc<LlmSettingsService<PostgresPool>>,
    pub config: Arc<Config>,
}
```

**Step 4: Add small Rust tests only around default resolution if extracted**

```rust
#[test]
fn defaults_use_latest_haiku_identifier() {
    assert_eq!(DEFAULT_ANTHROPIC_MODEL, "claude-haiku-4-5");
}
```

Run: `cargo test -p boopmark-server llm_settings -- --nocapture`
Expected: PASS for the new service/repository/default tests.

**Step 5: Commit**

```bash
git add migrations/005_create_llm_settings.sql \
  server/src/domain/ports/llm_settings_repo.rs \
  server/src/app/llm_settings.rs \
  server/src/adapters/postgres/llm_settings_repo.rs \
  server/src/domain/ports/mod.rs \
  server/src/app/mod.rs \
  server/src/adapters/postgres/mod.rs \
  server/src/web/state.rs \
  server/src/main.rs
git commit -m "feat: add per-user llm settings persistence"
```

### Task 2: Replace the stub with a real Settings page and keep route compatibility

**Files:**
- Create: `templates/settings/index.html`
- Modify: `server/src/web/pages/settings.rs`
- Modify: `server/src/web/pages/mod.rs`
- Modify: `templates/components/header.html`
- Delete: `templates/settings/api_keys.html`

**Step 1: Write the failing browser assertions first**

```javascript
test("settings page shows LLM Integration defaults", async ({ page }) => {
  await signIn(page);
  await page.goto("/settings");
  await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "LLM Integration" })).toBeVisible();
  await expect(page.getByLabel("Anthropic model")).toHaveValue("claude-haiku-4-5");
});
```

Run: `npx playwright test tests/e2e/profile-menu.spec.js --grep "settings"`
Expected: FAIL because `/settings` does not exist and the template still renders `API Keys`.

**Step 2: Rename the page and introduce a settings form model**

```rust
#[derive(Template)]
#[template(path = "settings/index.html")]
struct SettingsPage {
    email: String,
    llm_enabled: bool,
    anthropic_api_key: String,
    anthropic_model: String,
}
```

```rust
#[derive(serde::Deserialize)]
struct SaveLlmSettingsForm {
    enabled: Option<String>,
    anthropic_api_key: String,
    anthropic_model: String,
}
```

**Step 3: Add `GET /settings`, `POST /settings/llm`, and compatibility for `/settings/api-keys`**

```rust
Router::new()
    .route("/settings", get(settings_page))
    .route("/settings/llm", post(save_llm_settings))
    .route("/settings/api-keys", get(legacy_settings_redirect))
```

Implementation notes:
- `GET /settings` loads saved per-user settings or falls back to `enabled = false`, empty Anthropic key, and `claude-haiku-4-5`.
- `POST /settings/llm` trims inputs, treats a blank Anthropic key as `None`, persists the record, and redirects back to `/settings`.
- `GET /settings/api-keys` should respond with an HTTP redirect to `/settings` unless Askama/layout constraints make serving the same page simpler.

**Step 4: Replace header labels and template copy**

```html
<a href="/settings" data-testid="profile-menu-settings">Settings</a>
```

```html
<h1>Settings</h1>
<section>
  <h2>LLM Integration</h2>
  <label>
    <input type="checkbox" name="enabled" />
    Enable LLM integration
  </label>
  <input type="password" name="anthropic_api_key" autocomplete="off" />
  <input type="text" name="anthropic_model" value="{{ anthropic_model }}" />
</section>
```

Design constraints:
- Do not prefill the Anthropic key from `.env`.
- Preserve authenticated access through `AuthUser`.
- Keep the UI simple and idiomatic to the existing template styling.

**Step 5: Run the focused Rust and browser tests**

Run: `cargo test -p boopmark-server web::pages::settings -- --nocapture`
Expected: PASS if page helpers or form parsing tests are added; otherwise use this step to run the closest module tests that exist.

Run: `npx playwright test tests/e2e/profile-menu.spec.js --grep "Settings|settings"`
Expected: PASS for navigation into `Settings` and route compatibility from the profile menu.

**Step 6: Commit**

```bash
git add templates/settings/index.html \
  server/src/web/pages/settings.rs \
  server/src/web/pages/mod.rs \
  templates/components/header.html \
  tests/e2e/profile-menu.spec.js
git rm templates/settings/api_keys.html
git commit -m "feat: build settings page for llm integration"
```

### Task 3: Add browser workflow coverage and E2E env forwarding

**Files:**
- Create: `tests/e2e/settings.spec.js`
- Modify: `tests/e2e/profile-menu.spec.js`
- Modify: `scripts/e2e/start-server.sh`
- Modify: `playwright.config.js`

**Step 1: Write the failing end-to-end settings workflow**

```javascript
test("user can save llm settings without auto-populating from env", async ({ page }) => {
  await signIn(page);
  await page.goto("/settings");

  await expect(page.getByLabel("Anthropic API key")).toHaveValue("");
  await page.getByLabel("Enable LLM integration").check();
  await page.getByLabel("Anthropic API key").fill(process.env.ANTHROPIC_API_KEY ?? "");
  await page.getByLabel("Anthropic model").fill("claude-haiku-4-5");
  await page.getByRole("button", { name: "Save settings" }).click();

  await expect(page).toHaveURL(/\/settings$/);
  await expect(page.getByLabel("Enable LLM integration")).toBeChecked();
  await expect(page.getByLabel("Anthropic model")).toHaveValue("claude-haiku-4-5");
});
```

Run: `npx playwright test tests/e2e/settings.spec.js -v`
Expected: FAIL until the form persists data and the bootstrap forwards `ANTHROPIC_API_KEY`.

**Step 2: Forward `ANTHROPIC_API_KEY` only through the E2E bootstrap**

```bash
export ANTHROPIC_API_KEY="${ANTHROPIC_API_KEY:-}"
exec cargo run -p boopmark-server
```

Implementation notes:
- Do not read `.env` from the page handler to prefill user settings.
- The purpose of this export is only to let Playwright and local agent-browser runs access the key for form submission during testing.
- If Playwright needs explicit environment passthrough, set it in `playwright.config.js` or document that the shell-exported value is inherited by the web server and test process.

**Step 3: Expand navigation coverage to the renamed menu item**

```javascript
const settingsLink = page.getByTestId("profile-menu-settings");
await settingsLink.click();
await expect(page).toHaveURL(/\/settings$/);
await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();
```

**Step 4: Run the full approved browser suite**

Run: `npx playwright test tests/e2e/profile-menu.spec.js tests/e2e/settings.spec.js tests/e2e/suggest.spec.js`
Expected: PASS for the renamed menu path, new settings workflow, and existing bookmark suggestion regression coverage.

**Step 5: Commit**

```bash
git add tests/e2e/settings.spec.js \
  tests/e2e/profile-menu.spec.js \
  scripts/e2e/start-server.sh \
  playwright.config.js
git commit -m "test: cover settings llm workflow in e2e"
```

### Task 4: Final verification and cleanup

**Files:**
- Modify: `docs/plans/2026-03-09-settings-llm-integration.md`

**Step 1: Run formatting and targeted verification**

Run: `cargo fmt --all`
Expected: PASS

Run: `cargo test -p boopmark-server`
Expected: PASS

Run: `npx playwright test tests/e2e/profile-menu.spec.js tests/e2e/settings.spec.js tests/e2e/suggest.spec.js`
Expected: PASS

**Step 2: Manually verify route compatibility**

Run: `curl -I http://127.0.0.1:4010/settings/api-keys`
Expected: `302` to `/settings` or `200` with the new `Settings` page if compatibility is implemented by serving the same content directly.

**Step 3: Commit final polish if verification required code or copy adjustments**

```bash
git add server/src/web/pages/settings.rs \
  templates/settings/index.html \
  templates/components/header.html \
  tests/e2e/profile-menu.spec.js \
  tests/e2e/settings.spec.js \
  scripts/e2e/start-server.sh \
  playwright.config.js
git commit -m "chore: finalize settings llm integration"
```
