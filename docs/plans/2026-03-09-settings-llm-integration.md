# Settings LLM Integration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Replace the placeholder `API Keys` page with a real `Settings` page that lets each signed-in user enable or disable LLM integration, save an Anthropic API key, and use `claude-haiku-4-5` as the default Anthropic model.

**Architecture:** Do not bolt this onto the inbound `api_keys` auth table. Add a dedicated per-user `user_llm_settings` record with `enabled`, `anthropic_api_key`, and `anthropic_model`, then expose it through a small `SettingsService` used only by the `/settings` page handlers. Keep `.env` strictly in the local test harness: Playwright may read `ANTHROPIC_API_KEY` from the worktree `.env` to populate the form during local/browser testing, but the application must never auto-fill a user's saved settings from environment variables.

**Tech Stack:** Rust, Axum, Askama templates, SQLx/PostgreSQL, Tailwind CSS, Playwright

---

### Task 1: Prepare the local test harness and capture the new Settings expectations in failing Playwright specs

Get the worktree ready for local testing, then write the browser tests first so the current `API Keys` stub fails for the right reasons: wrong route, wrong copy, missing form, and no save/reload behavior.

**Files:**
- Create: `tests/e2e/settings.spec.js`
- Modify: `tests/e2e/profile-menu.spec.js`
- Modify: `.env.example`

**Step 1: Refresh the worktree `.env` from the main checkout before testing**

Run:

```bash
cp /Users/chrisfenton/Code/personal/boopmark/.env /Users/chrisfenton/Code/personal/boopmark/.worktrees/settings-llm-integration/.env
```

Expected:
- The worktree has the same local development secrets as the main checkout.
- This is a local-only prerequisite; `.env` remains ignored by git.

**Step 2: Document the optional Anthropic test secret in `.env.example`**

Append this line to `.env.example`:

```dotenv
# Optional: used only by local Playwright / agent-browser testing when filling the settings form
ANTHROPIC_API_KEY=
```

**Step 3: Replace the profile-menu expectations with `Settings`**

Update `tests/e2e/profile-menu.spec.js` so the existing navigation tests now expect the renamed menu item and the new route:

```js
const settingsLink = page.getByTestId("profile-menu-settings");
const settingsBox = await settingsLink.boundingBox();
if (!triggerBox || !settingsBox) {
  throw new Error("expected trigger and settings link to have bounding boxes");
}

await moveMouseInSteps(page, center(triggerBox), center(settingsBox));
await expect(menu).toBeVisible();

await settingsLink.click();
await expect(page).toHaveURL(/\/settings$/);
await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();
await expect(page.getByRole("heading", { name: "LLM Integration" })).toBeVisible();
```

Also rename the keyboard-navigation assertion to expect `profile-menu-settings`, `/settings`, and the `Settings` heading instead of `API Keys`.

**Step 4: Create a dedicated failing settings workflow spec**

Create `tests/e2e/settings.spec.js`:

```js
const { test, expect } = require("@playwright/test");
const fs = require("node:fs");
const path = require("node:path");

async function signIn(page) {
  await page.goto("/");
  await page.getByRole("button", { name: "Sign in for E2E" }).click();
  await expect(page).toHaveURL(/\/bookmarks$/);
}

function readAnthropicApiKeyFromDotEnv() {
  const envPath = path.resolve(__dirname, "..", "..", ".env");
  if (!fs.existsSync(envPath)) {
    return null;
  }

  const contents = fs.readFileSync(envPath, "utf8");
  const match = contents.match(/^ANTHROPIC_API_KEY=(.+)$/m);
  return match ? match[1].trim() : null;
}

test("settings page shows the default Anthropic model and saves LLM integration", async ({ page }) => {
  const anthropicApiKey = readAnthropicApiKeyFromDotEnv();
  test.skip(!anthropicApiKey, "ANTHROPIC_API_KEY is required in .env for this local test");

  await signIn(page);
  await page.goto("/settings");

  await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "LLM Integration" })).toBeVisible();
  await expect(page.getByLabel("Enable LLM integration")).not.toBeChecked();
  await expect(page.getByLabel("Anthropic model")).toHaveValue("claude-haiku-4-5");

  await page.getByLabel("Enable LLM integration").check();
  await page.getByLabel("Anthropic API key").fill(anthropicApiKey);
  await page.getByRole("button", { name: "Save settings" }).click();

  await expect(page).toHaveURL(/\/settings\?saved=1$/);
  await expect(page.getByText("Settings saved")).toBeVisible();
  await expect(page.getByLabel("Enable LLM integration")).toBeChecked();
  await expect(page.getByText("Anthropic API key saved")).toBeVisible();
  await expect(page.getByLabel("Anthropic API key")).toHaveValue("");

  await page.reload();
  await expect(page.getByLabel("Enable LLM integration")).toBeChecked();
  await expect(page.getByLabel("Anthropic model")).toHaveValue("claude-haiku-4-5");
  await expect(page.getByText("Anthropic API key saved")).toBeVisible();
});

test("settings page can disable llm integration without deleting the saved key", async ({ page }) => {
  const anthropicApiKey = readAnthropicApiKeyFromDotEnv();
  test.skip(!anthropicApiKey, "ANTHROPIC_API_KEY is required in .env for this local test");

  await signIn(page);
  await page.goto("/settings");

  await page.getByLabel("Enable LLM integration").check();
  await page.getByLabel("Anthropic API key").fill(anthropicApiKey);
  await page.getByRole("button", { name: "Save settings" }).click();

  await page.getByLabel("Enable LLM integration").uncheck();
  await page.getByRole("button", { name: "Save settings" }).click();

  await expect(page.getByLabel("Enable LLM integration")).not.toBeChecked();
  await expect(page.getByText("Anthropic API key saved")).toBeVisible();
});
```

The important behavior here is the boundary: the Playwright test reads `.env`, the product page does not.

**Step 5: Run the two Playwright files and verify they fail on the current code**

Run:

```bash
npx playwright test tests/e2e/profile-menu.spec.js tests/e2e/settings.spec.js
```

Expected:
- `profile-menu.spec.js` fails because the menu item is still `API Keys` and navigation still lands on `/settings/api-keys`.
- `settings.spec.js` fails because `/settings` and the LLM Integration form do not exist yet.

**Step 6: Commit the failing browser coverage**

```bash
git add .env.example tests/e2e/profile-menu.spec.js tests/e2e/settings.spec.js
git commit -m "test: capture settings llm integration workflow"
```

### Task 2: Add a dedicated per-user LLM settings persistence layer

Persist these fields separately from inbound API authentication. One row per user is enough for the current product shape and is the cleanest steady-state model.

**Files:**
- Create: `migrations/005_create_user_llm_settings.sql`
- Create: `server/src/domain/llm_settings.rs`
- Create: `server/src/domain/ports/llm_settings_repo.rs`
- Create: `server/src/adapters/postgres/llm_settings_repo.rs`
- Modify: `server/src/domain/mod.rs`
- Modify: `server/src/domain/ports/mod.rs`
- Modify: `server/src/adapters/postgres/mod.rs`

**Step 1: Create the new migration**

Create `migrations/005_create_user_llm_settings.sql`:

```sql
CREATE TABLE user_llm_settings (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    anthropic_api_key TEXT,
    anthropic_model TEXT NOT NULL DEFAULT 'claude-haiku-4-5',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

Do not touch `api_keys`; that table is for inbound bearer auth and cannot serve outbound Anthropic credentials.

**Step 2: Add the domain model and the default model constant**

Create `server/src/domain/llm_settings.rs`:

```rust
use chrono::{DateTime, Utc};
use uuid::Uuid;

pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-haiku-4-5";

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

Then export it from `server/src/domain/mod.rs`:

```rust
pub mod llm_settings;
```

**Step 3: Define the repository contract**

Create `server/src/domain/ports/llm_settings_repo.rs`:

```rust
use crate::domain::error::DomainError;
use crate::domain::llm_settings::LlmSettings;
use uuid::Uuid;

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

Then export it from `server/src/domain/ports/mod.rs`:

```rust
pub mod llm_settings_repo;
```

**Step 4: Implement the PostgreSQL adapter**

Create `server/src/adapters/postgres/llm_settings_repo.rs`:

```rust
use super::PostgresPool;
use crate::domain::error::DomainError;
use crate::domain::llm_settings::LlmSettings;
use crate::domain::ports::llm_settings_repo::LlmSettingsRepository;
use uuid::Uuid;

impl LlmSettingsRepository for PostgresPool {
    async fn get(&self, user_id: Uuid) -> Result<Option<LlmSettings>, DomainError> {
        sqlx::query_as::<_, LlmSettings>(
            "SELECT user_id, enabled, anthropic_api_key, anthropic_model, created_at, updated_at
             FROM user_llm_settings
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
            "INSERT INTO user_llm_settings (user_id, enabled, anthropic_api_key, anthropic_model)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (user_id) DO UPDATE
             SET enabled = EXCLUDED.enabled,
                 anthropic_api_key = EXCLUDED.anthropic_api_key,
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

Wire it into `server/src/adapters/postgres/mod.rs`:

```rust
pub mod llm_settings_repo;
```

**Step 5: Run the migration-bearing build**

Run:

```bash
cargo build -p boopmark-server
```

Expected:
- The new domain, port, adapter, and migration compile cleanly.
- No page code changes are required yet.

**Step 6: Commit the persistence layer**

```bash
git add migrations/005_create_user_llm_settings.sql server/src/domain/mod.rs server/src/domain/llm_settings.rs server/src/domain/ports/mod.rs server/src/domain/ports/llm_settings_repo.rs server/src/adapters/postgres/mod.rs server/src/adapters/postgres/llm_settings_repo.rs
git commit -m "feat: add llm settings persistence"
```

### Task 3: Add a Settings service with test-covered normalization and validation logic

Keep the business rules out of the page handler. This task owns the default model alias, blank-key preservation, and the one real validation rule: enabled LLM integration requires an Anthropic API key, either newly submitted or already stored.

**Files:**
- Create: `server/src/app/settings.rs`
- Modify: `server/src/app/mod.rs`
- Modify: `server/src/web/state.rs`
- Modify: `server/src/main.rs`

**Step 1: Create the settings service and its helper types**

Create `server/src/app/settings.rs`:

```rust
use crate::domain::error::DomainError;
use crate::domain::llm_settings::{DEFAULT_ANTHROPIC_MODEL, LlmSettings};
use crate::domain::ports::llm_settings_repo::LlmSettingsRepository;
use std::sync::Arc;
use uuid::Uuid;

pub struct SettingsService<R> {
    repo: Arc<R>,
}

pub struct SettingsView {
    pub enabled: bool,
    pub has_anthropic_api_key: bool,
    pub anthropic_model: String,
}

pub struct SaveLlmSettingsInput {
    pub enabled: bool,
    pub anthropic_api_key: Option<String>,
    pub anthropic_model: Option<String>,
}

impl<R> SettingsService<R>
where
    R: LlmSettingsRepository + Send + Sync,
{
    pub fn new(repo: Arc<R>) -> Self {
        Self { repo }
    }

    pub async fn load(&self, user_id: Uuid) -> Result<SettingsView, DomainError> {
        let settings = self.repo.get(user_id).await?;
        Ok(to_view(settings.as_ref()))
    }

    pub async fn save(
        &self,
        user_id: Uuid,
        input: SaveLlmSettingsInput,
    ) -> Result<SettingsView, DomainError> {
        let existing = self.repo.get(user_id).await?;
        let resolved_key = resolve_api_key(
            input.anthropic_api_key,
            existing.as_ref().and_then(|settings| settings.anthropic_api_key.clone()),
        );

        if input.enabled && resolved_key.is_none() {
            return Err(DomainError::InvalidInput(
                "Anthropic API key is required when LLM integration is enabled".into(),
            ));
        }

        let saved = self
            .repo
            .upsert(
                user_id,
                input.enabled,
                resolved_key.as_deref(),
                &normalize_model(input.anthropic_model),
            )
            .await?;

        Ok(to_view(Some(&saved)))
    }
}

fn normalize_model(input: Option<String>) -> String {
    let value = input.unwrap_or_default().trim().to_string();
    if value.is_empty() {
        DEFAULT_ANTHROPIC_MODEL.to_string()
    } else {
        value
    }
}

fn resolve_api_key(submitted: Option<String>, existing: Option<String>) -> Option<String> {
    match submitted {
        Some(value) if !value.trim().is_empty() => Some(value.trim().to_string()),
        _ => existing,
    }
}

fn to_view(settings: Option<&LlmSettings>) -> SettingsView {
    SettingsView {
        enabled: settings.map(|settings| settings.enabled).unwrap_or(false),
        has_anthropic_api_key: settings
            .and_then(|settings| settings.anthropic_api_key.as_ref())
            .is_some(),
        anthropic_model: settings
            .map(|settings| normalize_model(Some(settings.anthropic_model.clone())))
            .unwrap_or_else(|| DEFAULT_ANTHROPIC_MODEL.to_string()),
    }
}
```

**Step 2: Add focused unit tests inside `server/src/app/settings.rs`**

Append:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_model_defaults_to_latest_haiku_alias() {
        assert_eq!(normalize_model(None), "claude-haiku-4-5");
        assert_eq!(normalize_model(Some("   ".into())), "claude-haiku-4-5");
    }

    #[test]
    fn blank_api_key_keeps_existing_secret() {
        let resolved = resolve_api_key(Some("   ".into()), Some("sk-ant-existing".into()));
        assert_eq!(resolved.as_deref(), Some("sk-ant-existing"));
    }

    #[test]
    fn to_view_uses_defaults_when_no_settings_exist() {
        let view = to_view(None);
        assert!(!view.enabled);
        assert!(!view.has_anthropic_api_key);
        assert_eq!(view.anthropic_model, "claude-haiku-4-5");
    }
}
```

`claude-haiku-4-5` is deliberate here. Anthropic's official Claude Haiku 4.5 announcement on March 4, 2026 says to use that alias via the API.

**Step 3: Wire the service into the app**

Update `server/src/app/mod.rs`:

```rust
pub mod settings;
```

Update `server/src/web/state.rs`:

```rust
use crate::app::settings::SettingsService;

#[derive(Clone)]
pub struct AppState {
    pub bookmarks: Bookmarks,
    pub auth: Arc<AuthService<PostgresPool, PostgresPool, PostgresPool>>,
    pub settings: Arc<SettingsService<PostgresPool>>,
    pub config: Arc<Config>,
}
```

Update `server/src/main.rs`:

```rust
use app::settings::SettingsService;

let settings_service = Arc::new(SettingsService::new(db.clone()));

let state = AppState {
    bookmarks,
    auth: auth_service,
    settings: settings_service,
    config: Arc::new(config.clone()),
};
```

**Step 4: Run the focused Rust tests**

Run:

```bash
cargo test -p boopmark-server settings::tests
```

Expected:
- The helper behavior is locked in before the page code lands.
- The default model and blank-key preservation rules are proven without browser setup.

**Step 5: Commit the service layer**

```bash
git add server/src/app/mod.rs server/src/app/settings.rs server/src/web/state.rs server/src/main.rs
git commit -m "feat: add llm settings service"
```

### Task 4: Replace the API Keys stub with the real Settings page and save flow

Land the user-facing change here: `/settings`, `Settings` in the menu, `LLM Integration` on the page, GET + POST handlers, default model rendering, validation feedback, and a redirect from the old `/settings/api-keys` path.

**Files:**
- Modify: `server/src/web/pages/settings.rs`
- Modify: `templates/components/header.html`
- Create: `templates/settings/index.html`
- Delete: `templates/settings/api_keys.html`

**Step 1: Replace the page handler with GET + POST settings routes**

Rewrite `server/src/web/pages/settings.rs`:

```rust
use askama::Template;
use axum::Form;
use axum::Router;
use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse, Redirect};
use serde::Deserialize;

use crate::domain::error::DomainError;
use crate::web::extractors::AuthUser;
use crate::web::state::AppState;

#[derive(Template)]
#[template(path = "settings/index.html")]
struct SettingsPage {
    email: String,
    llm_enabled: bool,
    has_anthropic_api_key: bool,
    anthropic_model: String,
    success_message: Option<String>,
    error_message: Option<String>,
}

#[derive(Deserialize, Default)]
struct SettingsQuery {
    saved: Option<String>,
}

#[derive(Deserialize)]
struct SettingsForm {
    llm_enabled: Option<String>,
    anthropic_api_key: Option<String>,
    anthropic_model: Option<String>,
}

fn render(t: &impl Template) -> axum::response::Response {
    match t.render() {
        Ok(body) => Html(body).into_response(),
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn settings_page(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(query): Query<SettingsQuery>,
) -> axum::response::Response {
    match state.settings.load(user.id).await {
        Ok(view) => render(&SettingsPage {
            email: user.email,
            llm_enabled: view.enabled,
            has_anthropic_api_key: view.has_anthropic_api_key,
            anthropic_model: view.anthropic_model,
            success_message: query.saved.map(|_| "Settings saved".to_string()),
            error_message: None,
        }),
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn save_settings(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Form(form): Form<SettingsForm>,
) -> axum::response::Response {
    match state
        .settings
        .save(
            user.id,
            crate::app::settings::SaveLlmSettingsInput {
                enabled: form.llm_enabled.is_some(),
                anthropic_api_key: form.anthropic_api_key,
                anthropic_model: form.anthropic_model,
            },
        )
        .await
    {
        Ok(_) => Redirect::to("/settings?saved=1").into_response(),
        Err(DomainError::InvalidInput(message)) => render(&SettingsPage {
            email: user.email,
            llm_enabled: form.llm_enabled.is_some(),
            has_anthropic_api_key: state
                .settings
                .load(user.id)
                .await
                .map(|view| view.has_anthropic_api_key)
                .unwrap_or(false),
            anthropic_model: form
                .anthropic_model
                .unwrap_or_else(|| "claude-haiku-4-5".to_string()),
            success_message: None,
            error_message: Some(message),
        }),
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn legacy_api_keys_redirect() -> Redirect {
    Redirect::to("/settings")
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/settings", axum::routing::get(settings_page).post(save_settings))
        .route("/settings/api-keys", axum::routing::get(legacy_api_keys_redirect))
}
```

Do not add `.env` reads here. That boundary belongs only in the test harness.

**Step 2: Rename the header menu item and keep the hover/focus behavior intact**

Update `templates/components/header.html` so the menu link becomes:

```html
<a href="/settings" data-testid="profile-menu-settings" class="block text-sm text-gray-300 hover:text-white py-1">Settings</a>
```

Remove the old `profile-menu-api-keys` test ID entirely.

**Step 3: Create the real settings template**

Create `templates/settings/index.html`:

```html
{% extends "base.html" %}
{% block title %}Settings - BoopMark{% endblock %}
{% block content %}
<main class="max-w-2xl mx-auto px-6 py-12">
    <div class="bg-[#1e2235] rounded-xl border border-gray-700 p-8 space-y-8">
        <div>
            <h1 class="text-2xl font-bold">Settings</h1>
            <p class="text-sm text-gray-400 mt-1">{{ email }}</p>
        </div>

        {% if let Some(message) = success_message %}
        <div class="rounded-lg border border-emerald-500/40 bg-emerald-500/10 px-4 py-3 text-sm text-emerald-200">
            {{ message }}
        </div>
        {% endif %}

        {% if let Some(message) = error_message %}
        <div class="rounded-lg border border-red-500/40 bg-red-500/10 px-4 py-3 text-sm text-red-200">
            {{ message }}
        </div>
        {% endif %}

        <section class="space-y-5">
            <div>
                <h2 class="text-lg font-semibold">LLM Integration</h2>
                <p class="text-sm text-gray-400">Configure Anthropic access for this signed-in account.</p>
            </div>

            <form method="post" action="/settings" class="space-y-5">
                <label class="flex items-center gap-3 text-sm font-medium text-gray-200">
                    <input type="checkbox" name="llm_enabled" {% if llm_enabled %}checked{% endif %}>
                    Enable LLM integration
                </label>

                <div class="space-y-2">
                    <label for="anthropic_api_key" class="block text-sm font-medium text-gray-200">Anthropic API key</label>
                    <input
                        id="anthropic_api_key"
                        name="anthropic_api_key"
                        type="password"
                        autocomplete="off"
                        placeholder="{% if has_anthropic_api_key %}Leave blank to keep the saved key{% else %}sk-ant-...{% endif %}"
                        class="w-full px-4 py-2 rounded-lg bg-[#1a1d2e] border border-gray-700 text-gray-200 placeholder-gray-500 focus:outline-none focus:border-blue-500"
                    >
                    {% if has_anthropic_api_key %}
                    <p class="text-xs text-gray-400">Anthropic API key saved.</p>
                    {% else %}
                    <p class="text-xs text-gray-400">No Anthropic API key saved yet.</p>
                    {% endif %}
                </div>

                <div class="space-y-2">
                    <label for="anthropic_model" class="block text-sm font-medium text-gray-200">Anthropic model</label>
                    <input
                        id="anthropic_model"
                        name="anthropic_model"
                        type="text"
                        value="{{ anthropic_model }}"
                        class="w-full px-4 py-2 rounded-lg bg-[#1a1d2e] border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500"
                    >
                    <p class="text-xs text-gray-400">Default: claude-haiku-4-5.</p>
                </div>

                <button type="submit" class="px-4 py-2 bg-blue-600 hover:bg-blue-700 rounded-lg text-sm font-medium">
                    Save settings
                </button>
            </form>
        </section>
    </div>
</main>
{% endblock %}
```

Delete `templates/settings/api_keys.html`; the page is now `Settings`, not `API Keys`.

**Step 4: Keep the page module wired in**

`server/src/web/pages/mod.rs` already merges `settings::routes()`. Leave that intact; do not create a second settings entry point.

**Step 5: Run the focused build and browser suites**

Run:

```bash
cargo build -p boopmark-server
npx playwright test tests/e2e/profile-menu.spec.js tests/e2e/settings.spec.js
```

Expected:
- The app builds successfully with the new settings service and routes.
- The profile menu navigates to `/settings`.
- The new settings workflow passes, including default model rendering and reload-after-save behavior.

**Step 6: Commit the user-facing settings page**

```bash
git add server/src/web/pages/settings.rs templates/components/header.html templates/settings/index.html
git rm templates/settings/api_keys.html
git commit -m "feat: add llm integration settings page"
```

### Task 5: Run the final regression pass and only keep follow-up fixes that are actually needed

The shared header, settings route, migration path, and local test helper all touch authenticated flows. Finish with the smallest full pass that can catch cross-feature breakage.

**Files:**
- Modify: only files required by any follow-up fixes discovered here

**Step 1: Run the Rust test suite**

Run:

```bash
cargo test -p boopmark-server
```

Expected:
- The existing bookmark and scraper unit tests still pass.
- The new settings service tests pass.

**Step 2: Run the targeted Playwright regression suite**

Run:

```bash
npx playwright test tests/e2e/profile-menu.spec.js tests/e2e/settings.spec.js tests/e2e/suggest.spec.js
```

Expected:
- `profile-menu.spec.js` passes with the renamed Settings destination.
- `settings.spec.js` passes with local `.env`-backed test input and persisted settings.
- `suggest.spec.js` still passes, proving the shared authenticated shell still works.

**Step 3: If a regression requires a code change, make the smallest fix and re-run the affected tests**

Run only the smallest commands needed after each follow-up fix, for example:

```bash
cargo test -p boopmark-server
npx playwright test tests/e2e/profile-menu.spec.js tests/e2e/settings.spec.js tests/e2e/suggest.spec.js
```

Expected:
- Every failing regression is fixed before the branch is finalized.
- No speculative cleanup lands here.

**Step 4: Commit only if the regression pass forced a real code change**

```bash
git add -A
git commit -m "test: verify settings llm integration regressions"
```
