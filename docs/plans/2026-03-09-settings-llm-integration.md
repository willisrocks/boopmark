# Settings + LLM Integration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Replace the placeholder API Keys page with a real `Settings` page that lets each signed-in user enable or disable Anthropic-based LLM integration, save their own Anthropic API key, and use `claude-haiku-4-5` as the default model.

**Architecture:** Land the steady-state page directly at `/settings` instead of extending the old `/settings/api-keys` stub. Persist per-user LLM settings in a dedicated Postgres table plus a small settings service, keep the Anthropic key blank on reload so the browser never receives the stored secret back, and use a Post/Redirect/Get save flow with browser-level Playwright coverage for the actual user workflow. For local agent-browser and Playwright work only, read `ANTHROPIC_API_KEY` from the worktree `.env` as a test input source; do not pre-populate saved user settings from env.

**Tech Stack:** Rust, Axum, Askama templates, SQLx/Postgres, Tailwind CSS, Playwright

---

### Task 1: Rename the settings entry point and capture the new browser-level destination

Replace the profile-menu destination first so the app stops advertising `API Keys` and starts advertising `Settings` before any form logic is added.

**Files:**
- Modify: `templates/components/header.html`
- Modify: `server/src/web/pages/settings.rs`
- Create: `templates/settings/index.html`
- Modify: `tests/e2e/profile-menu.spec.js`

**Step 1: Rewrite the profile-menu link to point at `/settings`**

Update `templates/components/header.html` so the menu item text and href match the product language:

```html
<a
    href="/settings"
    data-testid="profile-menu-settings"
    class="block text-sm text-gray-300 hover:text-white py-1"
>
    Settings
</a>
```

Remove the old `profile-menu-api-keys` selector everywhere in this file.

**Step 2: Rework the settings page handler to render `/settings`**

Update `server/src/web/pages/settings.rs` so the page struct and route describe the new destination:

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

pub fn routes() -> Router<AppState> {
    Router::new().route("/settings", axum::routing::get(settings_page))
}
```

For this task, hard-code `llm_enabled = false`, `anthropic_model = "claude-haiku-4-5".into()`, `has_saved_anthropic_key = false`, `saved = false`, and `error = None`. Persistence comes later.

**Step 3: Replace the placeholder template with a real settings shell**

Create `templates/settings/index.html` with the renamed page and the future form structure already visible:

```html
{% extends "base.html" %}
{% block title %}Settings - BoopMark{% endblock %}
{% block content %}
<main class="max-w-2xl mx-8 py-12 space-y-6">
    <div class="bg-[#1e2235] rounded-xl border border-gray-700 p-8">
        <h1 class="text-2xl font-bold mb-2">Settings</h1>
        <p class="text-sm text-gray-400">{{ email }}</p>
    </div>

    <section class="bg-[#1e2235] rounded-xl border border-gray-700 p-8 space-y-4">
        <div>
            <h2 class="text-lg font-semibold">LLM Integration</h2>
            <p class="text-sm text-gray-400">
                Configure Anthropic for bookmark features that require an LLM.
            </p>
        </div>
    </section>
</main>
{% endblock %}
```

Do not keep `templates/settings/api_keys.html`; the steady-state page path is `templates/settings/index.html`.

**Step 4: Update the existing profile-menu Playwright assertions**

In `tests/e2e/profile-menu.spec.js`, rename the selector variables and destination assertions:

```js
const settingsLink = page.getByTestId("profile-menu-settings");
...
await settingsLink.click();
await expect(page).toHaveURL(/\/settings$/);
await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();
```

Update both the pointer-navigation test and the keyboard-focus test to use `Settings`.

**Step 5: Run the renamed menu coverage and verify it passes**

Run:

```bash
npx playwright test tests/e2e/profile-menu.spec.js
```

Expected:
- The existing hover/focus coverage still passes.
- The menu now lands on `/settings`.
- The page heading says `Settings`.

**Step 6: Commit the route rename**

```bash
git add templates/components/header.html server/src/web/pages/settings.rs templates/settings/index.html tests/e2e/profile-menu.spec.js
git commit -m "feat: rename api keys page to settings"
```

### Task 2: Add dedicated per-user LLM settings persistence

The app needs a retrievable per-user settings record. Do not reuse the hashed inbound API-key table for outbound Anthropic credentials.

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
- Modify: `server/src/web/state.rs`
- Modify: `server/src/main.rs`

**Step 1: Create the new database table**

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

Do not modify `api_keys`; that table solves a different problem.

**Step 2: Define the domain model and repository contract**

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

Wire both modules into `server/src/domain/mod.rs` and `server/src/domain/ports/mod.rs`.

**Step 3: Implement the Postgres adapter**

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

Export the new adapter module from `server/src/adapters/postgres/mod.rs`.

**Step 4: Add a minimal settings service and wire it into app state**

Create `server/src/app/settings.rs`:

```rust
use std::sync::Arc;
use uuid::Uuid;

use crate::domain::error::DomainError;
use crate::domain::ports::llm_settings_repo::LlmSettingsRepository;

pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-haiku-4-5";

pub struct SettingsService<R> {
    repo: Arc<R>,
}

pub struct SettingsSnapshot {
    pub enabled: bool,
    pub anthropic_model: String,
    pub has_saved_anthropic_key: bool,
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

Export the module from `server/src/app/mod.rs`, add `settings: Arc<SettingsService<PostgresPool>>` to `server/src/web/state.rs`, and initialize it in `server/src/main.rs` with `SettingsService::new(db.clone())`.

**Step 5: Build after the persistence wiring lands**

Run:

```bash
cargo build -p boopmark-server
```

Expected:
- The migration compiles into the server startup path.
- The new repository and service types build cleanly.
- No existing bookmark or auth code regresses.

**Step 6: Commit the persistence layer**

```bash
git add migrations/005_create_llm_settings.sql server/src/domain/llm_settings.rs server/src/domain/ports/llm_settings_repo.rs server/src/domain/mod.rs server/src/domain/ports/mod.rs server/src/adapters/postgres/llm_settings_repo.rs server/src/adapters/postgres/mod.rs server/src/app/settings.rs server/src/app/mod.rs server/src/web/state.rs server/src/main.rs
git commit -m "feat: persist per-user llm settings"
```

### Task 3: Implement the settings form save flow with safe key handling

Finish the real user workflow: render saved state, validate submissions, and never echo the stored Anthropic key back into the browser.

**Files:**
- Modify: `server/src/web/pages/settings.rs`
- Modify: `templates/settings/index.html`

**Step 1: Add GET and POST handlers for `/settings`**

Expand `server/src/web/pages/settings.rs` with form parsing and PRG:

```rust
use axum::Form;
use axum::extract::{Query, State};
use axum::response::Redirect;
use serde::Deserialize;

use crate::app::settings::{DEFAULT_ANTHROPIC_MODEL, SettingsService};
```

Add the request types:

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

Use these semantics in the handlers:
- `llm_enabled.is_some()` means enabled.
- `anthropic_model.trim()` falls back to `DEFAULT_ANTHROPIC_MODEL` when blank.
- `anthropic_api_key.trim()` becomes `None` when blank so an existing saved key is preserved.
- If `enabled == true` and there is neither a newly submitted key nor an already-saved key, re-render the page with `error = Some("Anthropic API key is required when LLM integration is enabled.".into())`.
- On success, `return Redirect::to("/settings?saved=1").into_response();`.

**Step 2: Render the actual form controls and saved-state affordances**

Replace the placeholder section in `templates/settings/index.html` with:

```html
{% if saved %}
<div class="bg-green-950/40 border border-green-800 text-green-200 rounded-xl px-4 py-3">
    Settings saved.
</div>
{% endif %}

{% if let Some(message) = error %}
<div class="bg-red-950/40 border border-red-800 text-red-200 rounded-xl px-4 py-3">
    {{ message }}
</div>
{% endif %}

<section class="bg-[#1e2235] rounded-xl border border-gray-700 p-8 space-y-6">
    <div>
        <h2 class="text-lg font-semibold">LLM Integration</h2>
        <p class="text-sm text-gray-400">
            Use your own Anthropic credentials for LLM-powered bookmark features.
        </p>
    </div>

    <form method="post" action="/settings" class="space-y-5">
        <label class="flex items-center justify-between gap-4">
            <span>
                <span class="block text-sm font-medium text-gray-200">Enable integration</span>
                <span class="block text-xs text-gray-500">Turn Anthropic-backed features on or off.</span>
            </span>
            <input type="checkbox" name="llm_enabled" value="1" {% if llm_enabled %}checked{% endif %}>
        </label>

        <div class="space-y-2">
            <label for="anthropic_api_key" class="block text-sm font-medium text-gray-200">Anthropic API key</label>
            <input id="anthropic_api_key" name="anthropic_api_key" type="password" autocomplete="off" class="w-full px-4 py-2 rounded-lg bg-[#1a1d2e] border border-gray-700 text-gray-200">
            {% if has_saved_anthropic_key %}
            <p class="text-xs text-gray-500">A key is already saved. Leave this blank to keep it.</p>
            {% else %}
            <p class="text-xs text-gray-500">Enter the Anthropic key you want this account to use.</p>
            {% endif %}
        </div>

        <div class="space-y-2">
            <label for="anthropic_model" class="block text-sm font-medium text-gray-200">Anthropic model</label>
            <input id="anthropic_model" name="anthropic_model" type="text" value="{{ anthropic_model }}" class="w-full px-4 py-2 rounded-lg bg-[#1a1d2e] border border-gray-700 text-gray-200">
        </div>

        <button type="submit" class="px-4 py-2 bg-blue-600 hover:bg-blue-700 rounded-lg text-sm font-medium">
            Save settings
        </button>
    </form>
</section>
```

Do not render the saved API key back into the `value` attribute.

**Step 3: Build and manually sanity-check the page**

Run:

```bash
cargo build -p boopmark-server
```

Expected:
- The settings handlers compile.
- Askama renders the conditional success and error states.
- The page shows the model field defaulting to `claude-haiku-4-5`.

**Step 4: Commit the form flow**

```bash
git add server/src/web/pages/settings.rs templates/settings/index.html
git commit -m "feat: add llm integration settings form"
```

### Task 4: Add Playwright coverage for the full settings workflow and local `.env` test input

The highest-value confidence is the browser flow: navigate to Settings, save LLM settings, reload, and confirm persisted state without exposing the secret.

**Files:**
- Create: `tests/e2e/settings.spec.js`
- Modify: `.env.example`

**Step 1: Document the optional local Anthropic env var**

Append this line to `.env.example`:

```dotenv
ANTHROPIC_API_KEY=
```

This is test input for local agent-browser and Playwright use only. The settings page still requires the user to submit their own key.

**Step 2: Add a small `.env` reader for Playwright**

Create `tests/e2e/settings.spec.js` with a local helper that reads the worktree `.env` without adding another dependency:

```js
const fs = require("fs");
const { test, expect } = require("@playwright/test");

function readAnthropicKeyFromDotenv() {
  try {
    const lines = fs.readFileSync(".env", "utf8").split(/\r?\n/);
    for (const line of lines) {
      if (line.startsWith("ANTHROPIC_API_KEY=")) {
        return line.slice("ANTHROPIC_API_KEY=".length).trim();
      }
    }
  } catch (_) {
    return "";
  }

  return "";
}
```

Keep the parser intentionally tiny; this repo controls the `.env` format.

**Step 3: Add the authenticated settings workflow test**

In the same file, add a full browser test:

```js
test("user can save llm integration settings and reload the page", async ({ page }) => {
  const anthropicKey = readAnthropicKeyFromDotenv() || "sk-ant-test-placeholder";

  await page.goto("/");
  await page.getByRole("button", { name: "Sign in for E2E" }).click();
  await page.getByTestId("profile-menu-trigger").hover();
  await page.getByTestId("profile-menu-settings").click();

  await expect(page).toHaveURL(/\/settings$/);
  await page.getByLabel("Enable integration").check();
  await page.getByLabel("Anthropic API key").fill(anthropicKey);
  await page.getByLabel("Anthropic model").fill("claude-haiku-4-5");
  await page.getByRole("button", { name: "Save settings" }).click();

  await expect(page).toHaveURL(/\/settings\?saved=1$/);
  await expect(page.getByText("Settings saved.")).toBeVisible();
  await expect(page.getByLabel("Enable integration")).toBeChecked();
  await expect(page.getByLabel("Anthropic model")).toHaveValue("claude-haiku-4-5");
  await expect(page.getByText("A key is already saved. Leave this blank to keep it.")).toBeVisible();
  await expect(page.getByLabel("Anthropic API key")).toHaveValue("");

  await page.reload();
  await expect(page.getByLabel("Enable integration")).toBeChecked();
  await expect(page.getByLabel("Anthropic model")).toHaveValue("claude-haiku-4-5");
  await expect(page.getByLabel("Anthropic API key")).toHaveValue("");
});
```

If Playwright has trouble resolving labels from the current markup, add `for` / `id` pairs until `getByLabel(...)` works cleanly. Do not fall back to brittle CSS selectors.

**Step 4: Verify the worktree `.env` is current before running browser tests**

Run:

```bash
cp /Users/chrisfenton/Code/personal/boopmark/.env /Users/chrisfenton/Code/personal/boopmark/.worktrees/settings-llm-integration/.env
```

Expected:
- The worktree test environment matches the main repo `.env`.
- Local browser automation can read `ANTHROPIC_API_KEY` when present.

**Step 5: Run the browser suites**

Run:

```bash
npx playwright test tests/e2e/profile-menu.spec.js tests/e2e/settings.spec.js
```

Expected:
- The renamed menu navigation still passes.
- The new settings workflow passes end-to-end.
- The saved API key never reappears in the DOM after save or reload.

**Step 6: Commit the browser coverage**

```bash
git add .env.example tests/e2e/settings.spec.js
git commit -m "test: cover settings llm integration flow"
```

### Task 5: Run the final regression pass

Finish with the smallest useful pass that covers the touched app shell and the existing bookmark flow.

**Files:**
- Modify: none

**Step 1: Run the server build and targeted Playwright suite**

```bash
cargo build -p boopmark-server
npx playwright test tests/e2e/profile-menu.spec.js tests/e2e/settings.spec.js tests/e2e/suggest.spec.js
```

Expected:
- The server builds cleanly.
- The new settings coverage passes.
- The existing suggest flow still passes after the header and settings changes.

**Step 2: Commit only if the regression run required a final fix**

```bash
git add -A
git commit -m "test: verify settings and bookmark regressions"
```
