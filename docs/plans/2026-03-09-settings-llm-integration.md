# Settings + LLM Integration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Replace the `API Keys` stub with a real `Settings` page where each signed-in user can enable or disable Anthropic-backed LLM integration, save their own Anthropic API key, and default the model to `claude-haiku-4-5`.

**Architecture:** Land the steady-state page directly at `/settings`, not `/settings/api-keys`. Add a dedicated per-user `llm_settings` persistence path in Postgres plus a small settings service, render the saved state through Askama, and submit through a Post/Redirect/Get save flow that never sends the stored Anthropic key back to the browser. For local agent-browser and Playwright E2E only, forward `ANTHROPIC_API_KEY` from the worktree `.env` into the E2E server bootstrap and let browser tests reuse that value when present; do not auto-populate user settings from env.

**Tech Stack:** Rust, Axum, Askama, SQLx/Postgres, Tailwind CSS, Playwright

---

### Task 1: Rename the product surface from API Keys to Settings

Switch the visible product language and route first so the app shell points to the right destination before form and persistence work lands.

**Files:**
- Modify: `templates/components/header.html:47`
- Modify: `server/src/web/pages/settings.rs:8-26`
- Create: `templates/settings/index.html`
- Delete: `templates/settings/api_keys.html`
- Modify: `tests/e2e/profile-menu.spec.js:50-99`

**Step 1: Write the failing browser expectation for Settings**

In `tests/e2e/profile-menu.spec.js`, replace the old API Keys selectors and assertions:

```js
const settingsLink = page.getByTestId("profile-menu-settings");
...
await settingsLink.click();
await expect(page).toHaveURL(/\/settings$/);
await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();
```

Update both navigation tests and the keyboard-focus test to stop referring to `profile-menu-api-keys`, `/settings/api-keys`, and `API Keys`.

**Step 2: Run the existing profile-menu spec and verify it fails**

Run:

```bash
npx playwright test tests/e2e/profile-menu.spec.js
```

Expected: FAIL because the header still renders the old selector, link text, and route.

**Step 3: Rewrite the header and route to `/settings`**

Update `templates/components/header.html`:

```html
<a href="/settings" data-testid="profile-menu-settings" class="block text-sm text-gray-300 hover:text-white py-1">
    Settings
</a>
```

Replace the route shell in `server/src/web/pages/settings.rs`:

```rust
#[derive(Template)]
#[template(path = "settings/index.html")]
struct SettingsPage {
    email: String,
    llm_enabled: bool,
    anthropic_model: String,
    has_saved_anthropic_key: bool,
    saved: bool,
    error: Option<String>,
}

async fn settings_page(AuthUser(user): AuthUser) -> axum::response::Response {
    render(&SettingsPage {
        email: user.email,
        llm_enabled: false,
        anthropic_model: "claude-haiku-4-5".into(),
        has_saved_anthropic_key: false,
        saved: false,
        error: None,
    })
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/settings", axum::routing::get(settings_page))
}
```

Create `templates/settings/index.html` with the new page heading and an `LLM Integration` section shell. Delete `templates/settings/api_keys.html`.

**Step 4: Run the renamed menu coverage and verify it passes**

Run:

```bash
npx playwright test tests/e2e/profile-menu.spec.js
```

Expected:
- PASS.
- The profile menu still stays open during hover and keyboard transitions.
- The menu item lands on `/settings`.
- The page heading reads `Settings`.

**Step 5: Commit the Settings rename**

```bash
git add templates/components/header.html server/src/web/pages/settings.rs templates/settings/index.html templates/settings/api_keys.html tests/e2e/profile-menu.spec.js
git commit -m "feat: rename api keys page to settings"
```

### Task 2: Add dedicated per-user LLM settings persistence

The outbound Anthropic configuration needs its own storage model. Do not overload the hashed inbound `api_keys` table.

**Files:**
- Create: `migrations/005_create_llm_settings.sql`
- Create: `server/src/domain/llm_settings.rs`
- Create: `server/src/domain/ports/llm_settings_repo.rs`
- Modify: `server/src/domain/mod.rs`
- Modify: `server/src/domain/ports/mod.rs`
- Create: `server/src/adapters/postgres/llm_settings_repo.rs`
- Modify: `server/src/adapters/postgres/mod.rs`
- Create: `server/src/app/settings.rs`
- Modify: `server/src/app/mod.rs`
- Modify: `server/src/web/state.rs:15-19`
- Modify: `server/src/main.rs:37-80`

**Step 1: Write a failing compile target for the new settings service wiring**

Add the new module declarations and `settings` field references first, then try to build before the implementations exist.

```rust
pub settings: Arc<SettingsService<PostgresPool>>,
```

Expected missing-symbol errors:
- `settings` module not found
- `LlmSettingsRepository` not found
- `llm_settings_repo` module not found

**Step 2: Run the server build and verify it fails for the missing settings modules**

Run:

```bash
cargo build -p boopmark-server
```

Expected: FAIL with unresolved module/type errors for the new settings persistence pieces.

**Step 3: Create the database table, domain model, repository contract, and Postgres adapter**

Create `migrations/005_create_llm_settings.sql`:

```sql
CREATE TABLE llm_settings (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    anthropic_api_key TEXT,
    anthropic_model TEXT NOT NULL DEFAULT 'claude-haiku-4-5',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

Create `server/src/domain/llm_settings.rs`:

```rust
use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LlmSettings {
    pub user_id: Uuid,
    pub enabled: bool,
    pub anthropic_api_key: Option<String>,
    pub anthropic_model: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

Create `server/src/domain/ports/llm_settings_repo.rs`:

```rust
use crate::domain::error::DomainError;
use crate::domain::llm_settings::LlmSettings;
use uuid::Uuid;

#[trait_variant::make(Send)]
pub trait LlmSettingsRepository: Send + Sync {
    async fn find_by_user_id(&self, user_id: Uuid) -> Result<Option<LlmSettings>, DomainError>;
    async fn upsert(
        &self,
        user_id: Uuid,
        enabled: bool,
        anthropic_api_key: Option<&str>,
        anthropic_model: &str,
    ) -> Result<LlmSettings, DomainError>;
}
```

Create `server/src/adapters/postgres/llm_settings_repo.rs`:

```rust
impl LlmSettingsRepository for PostgresPool {
    async fn find_by_user_id(&self, user_id: Uuid) -> Result<Option<LlmSettings>, DomainError> {
        sqlx::query_as::<_, LlmSettings>(
            "SELECT user_id, enabled, anthropic_api_key, anthropic_model, created_at, updated_at
             FROM llm_settings
             WHERE user_id = $1",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))
    }

    async fn upsert(
        &self,
        user_id: Uuid,
        enabled: bool,
        anthropic_api_key: Option<&str>,
        anthropic_model: &str,
    ) -> Result<LlmSettings, DomainError> {
        sqlx::query_as::<_, LlmSettings>(
            "INSERT INTO llm_settings (user_id, enabled, anthropic_api_key, anthropic_model)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (user_id) DO UPDATE
             SET enabled = EXCLUDED.enabled,
                 anthropic_api_key = COALESCE(EXCLUDED.anthropic_api_key, llm_settings.anthropic_api_key),
                 anthropic_model = EXCLUDED.anthropic_model,
                 updated_at = now()
             RETURNING user_id, enabled, anthropic_api_key, anthropic_model, created_at, updated_at",
        )
        .bind(user_id)
        .bind(enabled)
        .bind(anthropic_api_key)
        .bind(anthropic_model)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))
    }
}
```

Wire the new modules into `server/src/domain/mod.rs`, `server/src/domain/ports/mod.rs`, `server/src/adapters/postgres/mod.rs`, `server/src/app/mod.rs`, `server/src/web/state.rs`, and `server/src/main.rs`.

**Step 4: Create the settings service with the default model constant**

Create `server/src/app/settings.rs`:

```rust
use std::sync::Arc;
use uuid::Uuid;

use crate::domain::error::DomainError;
use crate::domain::ports::llm_settings_repo::LlmSettingsRepository;

pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-haiku-4-5";

pub struct SettingsSnapshot {
    pub enabled: bool,
    pub anthropic_model: String,
    pub has_saved_anthropic_key: bool,
}

pub struct SettingsService<R> {
    repo: Arc<R>,
}

impl<R> SettingsService<R>
where
    R: LlmSettingsRepository + Send + Sync,
{
    pub fn new(repo: Arc<R>) -> Self {
        Self { repo }
    }

    pub async fn get(&self, user_id: Uuid) -> Result<SettingsSnapshot, DomainError> {
        let row = self.repo.find_by_user_id(user_id).await?;
        Ok(SettingsSnapshot {
            enabled: row.as_ref().map(|settings| settings.enabled).unwrap_or(false),
            anthropic_model: row
                .as_ref()
                .map(|settings| settings.anthropic_model.clone())
                .unwrap_or_else(|| DEFAULT_ANTHROPIC_MODEL.to_string()),
            has_saved_anthropic_key: row
                .as_ref()
                .and_then(|settings| settings.anthropic_api_key.as_ref())
                .map(|value| !value.is_empty())
                .unwrap_or(false),
        })
    }

    pub async fn save(
        &self,
        user_id: Uuid,
        enabled: bool,
        anthropic_api_key: Option<&str>,
        anthropic_model: &str,
    ) -> Result<(), DomainError> {
        self.repo
            .upsert(user_id, enabled, anthropic_api_key, anthropic_model)
            .await?;
        Ok(())
    }
}
```

Keep default-resolution logic inside the service. It is trivial enough that no extra Rust test file is required unless execution ends up extracting more logic than shown above.

**Step 5: Run the server build and verify it passes**

Run:

```bash
cargo build -p boopmark-server
```

Expected:
- PASS.
- The migration is picked up by `sqlx::migrate!`.
- The server builds with the new settings service and repository wiring.

**Step 6: Commit the persistence layer**

```bash
git add migrations/005_create_llm_settings.sql server/src/domain/llm_settings.rs server/src/domain/ports/llm_settings_repo.rs server/src/domain/mod.rs server/src/domain/ports/mod.rs server/src/adapters/postgres/llm_settings_repo.rs server/src/adapters/postgres/mod.rs server/src/app/settings.rs server/src/app/mod.rs server/src/web/state.rs server/src/main.rs
git commit -m "feat: persist per-user llm settings"
```

### Task 3: Implement the Settings form with safe secret-handling semantics

Finish the user-facing workflow: render persisted state, validate the submission, and never echo the saved key back into the browser.

**Files:**
- Modify: `server/src/web/pages/settings.rs`
- Modify: `templates/settings/index.html`

**Step 1: Write the failing browser workflow test for save and reload**

Create `tests/e2e/settings.spec.js` with a single end-to-end scenario that:
- signs in
- opens Settings from the profile menu
- enables LLM integration
- fills the Anthropic key and model
- saves
- verifies the success flash, checked state, and model value
- reloads and verifies the saved key input stays blank

Use stable selectors and labels:

```js
await page.getByTestId("profile-menu-settings").click();
await page.getByLabel("Enable integration").check();
await page.getByLabel("Anthropic API key").fill(anthropicKey);
await page.getByLabel("Anthropic model").fill("claude-haiku-4-5");
await page.getByRole("button", { name: "Save settings" }).click();
```

**Step 2: Run the new settings spec and verify it fails**

Run:

```bash
npx playwright test tests/e2e/settings.spec.js
```

Expected: FAIL because the page still has no POST handler, no form controls, and no persistence-backed render state.

**Step 3: Add GET/POST handlers and form parsing to `server/src/web/pages/settings.rs`**

Use Axum `State`, `Query`, `Form`, and `Redirect` with a PRG flow:

```rust
#[derive(Deserialize, Default)]
struct SettingsQuery {
    saved: Option<u8>,
}

#[derive(Deserialize)]
struct SettingsForm {
    llm_enabled: Option<String>,
    anthropic_api_key: Option<String>,
    anthropic_model: Option<String>,
}
```

Handler rules:
- `llm_enabled.is_some()` means enabled.
- `anthropic_model.trim()` falls back to `DEFAULT_ANTHROPIC_MODEL` when blank.
- `anthropic_api_key.trim()` becomes `None` when blank so an existing saved key remains unchanged.
- If integration is enabled and there is neither a new key nor a saved key, re-render with `error = Some("Anthropic API key is required when LLM integration is enabled.".into())`.
- On success, `Redirect::to("/settings?saved=1")`.

The GET handler should load `state.settings.get(user.id).await` and map it into `SettingsPage`.

**Step 4: Render the real form in `templates/settings/index.html`**

Replace the placeholder section with a form containing:
- a checkbox labeled `Enable integration`
- a password input labeled `Anthropic API key`
- a text input labeled `Anthropic model`
- a `Save settings` submit button
- a success flash for `saved`
- an error flash for `error`
- copy that tells the user a key is already saved and blank preserves it

Use `for` and `id` pairs so Playwright can use `getByLabel(...)`. Keep the API key input `value` empty even when `has_saved_anthropic_key` is true.

**Step 5: Run the settings browser workflow and verify it passes**

Run:

```bash
npx playwright test tests/e2e/settings.spec.js
```

Expected:
- PASS.
- The default model shown before save is `claude-haiku-4-5`.
- After save and reload, the checkbox and model stay persisted.
- The page never renders the saved key value back into the DOM.

**Step 6: Commit the settings form flow**

```bash
git add server/src/web/pages/settings.rs templates/settings/index.html tests/e2e/settings.spec.js
git commit -m "feat: add llm integration settings form"
```

### Task 4: Wire E2E env forwarding and finish browser-level coverage

The user explicitly wants `ANTHROPIC_API_KEY` forwarded only for agent-browser and Playwright E2E work. Keep that test-only and separate from the saved user settings flow.

**Files:**
- Modify: `scripts/e2e/start-server.sh:1-24`
- Modify: `tests/e2e/settings.spec.js`
- Modify: `tests/e2e/profile-menu.spec.js`

**Step 1: Make the E2E server bootstrap forward `ANTHROPIC_API_KEY` from the worktree `.env`**

Update `scripts/e2e/start-server.sh` so it exports the copied worktree key when present before `cargo run`:

```bash
if [ -f .env ]; then
  export ANTHROPIC_API_KEY="${ANTHROPIC_API_KEY:-$(awk -F= '/^ANTHROPIC_API_KEY=/{print substr($0, index($0,$2))}' .env)}"
fi
```

If the env var is absent, keep behavior unchanged. Do not write any code that seeds the user settings table from this env var.

**Step 2: Make the Playwright settings spec prefer the real test key when available**

At the top of `tests/e2e/settings.spec.js`, resolve the key like this:

```js
const fs = require("fs");

function loadAnthropicKey() {
  if (process.env.ANTHROPIC_API_KEY) {
    return process.env.ANTHROPIC_API_KEY;
  }

  try {
    const line = fs
      .readFileSync(".env", "utf8")
      .split(/\r?\n/)
      .find((entry) => entry.startsWith("ANTHROPIC_API_KEY="));
    return line ? line.slice("ANTHROPIC_API_KEY=".length).trim() : "";
  } catch (_) {
    return "";
  }
}
```

Use `loadAnthropicKey() || "sk-ant-test-placeholder"` when filling the form. This keeps E2E deterministic while still checking the copied `.env` first.

**Step 3: Expand the browser assertions to cover the approved workflow**

In `tests/e2e/settings.spec.js`, assert all of these:
- initial heading is `Settings`
- section heading is `LLM Integration`
- default model value is `claude-haiku-4-5`
- success redirect lands on `/settings?saved=1`
- reload preserves enabled/model state
- saved key field stays blank

Keep the existing menu-navigation assertions in `tests/e2e/profile-menu.spec.js` passing with the new `profile-menu-settings` selector.

**Step 4: Run the browser suites and verify they pass**

Run:

```bash
npx playwright test tests/e2e/profile-menu.spec.js tests/e2e/settings.spec.js
```

Expected:
- PASS.
- Menu navigation still works.
- The end-to-end settings workflow passes using the real `.env` key when available.
- The stored key never reappears in rendered HTML after save or reload.

**Step 5: Commit the E2E wiring**

```bash
git add scripts/e2e/start-server.sh tests/e2e/settings.spec.js tests/e2e/profile-menu.spec.js
git commit -m "test: cover settings llm integration flow"
```

### Task 5: Run the final regression pass

Finish with the smallest useful regression sweep covering the touched app shell and the existing bookmark flow.

**Files:**
- Modify: none

**Step 1: Run the final server build and targeted Playwright coverage**

Run:

```bash
cargo build -p boopmark-server
npx playwright test tests/e2e/profile-menu.spec.js tests/e2e/settings.spec.js tests/e2e/suggest.spec.js
```

Expected:
- PASS.
- The server still builds cleanly.
- The new settings flow remains green.
- The existing suggest flow still passes after the header and settings changes.

**Step 2: Commit only if the regression run required a follow-up fix**

```bash
git add -A
git commit -m "test: verify settings regressions"
```
