# Settings + LLM Integration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Replace the placeholder `API Keys` page with a real `Settings` page that lets each signed-in user enable or disable LLM integration, save an Anthropic API key, clear a saved Anthropic API key, and use `claude-haiku-4-5` as the default Anthropic model.

**Architecture:** Add a dedicated per-user `user_llm_settings` record instead of reusing inbound `api_keys`, but do not store the Anthropic secret in plaintext. Persist only encrypted-at-rest key material using an application encryption key supplied by config, keep the settings page itself limited to save/replace/clear behavior, and leave actual Anthropic usage out of scope for this feature. Keep `.env` strictly in the local test harness: the worktree `.env` must be present and contain `ANTHROPIC_API_KEY`, `scripts/e2e/start-server.sh` must forward it for local agent-browser and Playwright runs, and the Playwright settings test must fail if that propagation is missing instead of silently falling back to a fake key.

**Tech Stack:** Rust, Axum, Askama templates, SQLx/PostgreSQL, Tailwind CSS, Playwright

---

### Task 1: Verify local `.env` propagation and capture failing browser expectations

Start with the E2E harness so the current `API Keys` stub fails for the right reasons: wrong route, wrong copy, missing form, missing clear-key flow, and no save/reload behavior. The test harness must prove the copied worktree `.env` is present and contains both `ANTHROPIC_API_KEY` and `LLM_SETTINGS_ENCRYPTION_KEY` before any build or browser step depends on them.

**Files:**
- Modify: `scripts/e2e/start-server.sh`
- Modify: `tests/e2e/profile-menu.spec.js`
- Create: `tests/e2e/settings.spec.js`

**Step 1: Refresh the copied worktree `.env` and ensure both required keys exist**

Run:

```bash
cp /Users/chrisfenton/Code/personal/boopmark/.env /Users/chrisfenton/Code/personal/boopmark/.worktrees/settings-llm-integration/.env
if ! rg -q '^LLM_SETTINGS_ENCRYPTION_KEY=' .env; then
  printf '\nLLM_SETTINGS_ENCRYPTION_KEY=%s\n' "$(openssl rand -base64 32)" >> .env
fi
test -f .env
rg '^ANTHROPIC_API_KEY=' .env
rg '^LLM_SETTINGS_ENCRYPTION_KEY=' .env
```

Expected:
- PASS.
- The worktree `.env` has been refreshed from the main checkout.
- `ANTHROPIC_API_KEY` is present for local agent-browser and Playwright testing.
- `LLM_SETTINGS_ENCRYPTION_KEY` is present before any server startup path depends on it.

This remains a local prerequisite only. `.env` stays untracked, but the worktree copy must be current before the rest of the plan runs.

**Step 2: Forward `ANTHROPIC_API_KEY` only through the E2E bootstrap**

Update `scripts/e2e/start-server.sh` so the copied worktree `.env` feeds local agent-browser and Playwright runs without seeding saved user settings:

```bash
if [ -f .env ] && [ -z "${ANTHROPIC_API_KEY:-}" ]; then
  export ANTHROPIC_API_KEY="$(awk -F= '/^ANTHROPIC_API_KEY=/{print substr($0, index($0,$2))}' .env)"
fi
```

Leave the server behavior unchanged when the variable is absent. This is test harness wiring only. `LLM_SETTINGS_ENCRYPTION_KEY` is read by normal app config loading from the refreshed worktree `.env`; it does not need a separate bootstrap export.

**Step 3: Replace the profile-menu expectations with `Settings`**

Update `tests/e2e/profile-menu.spec.js` so the existing navigation tests expect the renamed menu item and new route:

```js
const settingsLink = page.getByTestId("profile-menu-settings");
...
await settingsLink.click();
await expect(page).toHaveURL(/\/settings$/);
await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();
await expect(page.getByRole("heading", { name: "LLM Integration" })).toBeVisible();
```

Also update the keyboard-navigation assertions to use `profile-menu-settings`, `/settings`, and `Settings` instead of `API Keys`.

**Step 4: Create a failing settings workflow spec with no fake-key fallback**

Create `tests/e2e/settings.spec.js`:

```js
const fs = require("node:fs");
const path = require("node:path");
const { test, expect } = require("@playwright/test");

async function signIn(page) {
  await page.goto("/");
  await page.getByRole("button", { name: "Sign in for E2E" }).click();
  await expect(page).toHaveURL(/\/bookmarks$/);
}

function readAnthropicApiKeyFromDotEnv() {
  const envPath = path.resolve(__dirname, "..", "..", ".env");
  const contents = fs.readFileSync(envPath, "utf8");
  const match = contents.match(/^ANTHROPIC_API_KEY=(.+)$/m);
  if (!match || !match[1].trim()) {
    throw new Error("ANTHROPIC_API_KEY must exist in the copied worktree .env");
  }
  return match[1].trim();
}

test("settings page shows the default Anthropic model and saves llm integration", async ({ page }) => {
  const anthropicApiKey = readAnthropicApiKeyFromDotEnv();

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
  await expect(page.getByText("Anthropic API key saved")).toBeVisible();
  await expect(page.getByLabel("Anthropic API key")).toHaveValue("");

  await page.reload();
  await expect(page.getByLabel("Enable LLM integration")).toBeChecked();
  await expect(page.getByLabel("Anthropic model")).toHaveValue("claude-haiku-4-5");
  await expect(page.getByText("Anthropic API key saved")).toBeVisible();
});

test("settings page can clear a saved anthropic key", async ({ page }) => {
  const anthropicApiKey = readAnthropicApiKeyFromDotEnv();

  await signIn(page);
  await page.goto("/settings");

  await page.getByLabel("Enable LLM integration").check();
  await page.getByLabel("Anthropic API key").fill(anthropicApiKey);
  await page.getByRole("button", { name: "Save settings" }).click();

  await page.getByLabel("Clear saved Anthropic API key").check();
  await page.getByRole("button", { name: "Save settings" }).click();

  await expect(page.getByText("No Anthropic API key saved yet.")).toBeVisible();
  await expect(page.getByLabel("Clear saved Anthropic API key")).not.toBeChecked();
});
```

This test is intentionally strict: it reads the copied worktree `.env` and fails if the local secret is missing instead of masking the problem with a placeholder.

**Step 5: Run the browser specs and verify they fail on current code**

Run:

```bash
npx playwright test tests/e2e/profile-menu.spec.js tests/e2e/settings.spec.js
```

Expected:
- `profile-menu.spec.js` fails because the menu item is still `API Keys` and still lands on `/settings/api-keys`.
- `settings.spec.js` fails because `/settings` and the LLM Integration form do not exist yet.

**Step 6: Commit the failing browser coverage**

```bash
git add scripts/e2e/start-server.sh tests/e2e/profile-menu.spec.js tests/e2e/settings.spec.js
git commit -m "test: capture settings llm integration workflow"
```

### Task 2: Add encrypted-at-rest LLM settings persistence

Persist the LLM settings separately from inbound API authentication, but do not introduce plaintext storage for the Anthropic key. Add the minimum crypto/config surface needed to encrypt at save time and support clear-or-replace semantics.

**Files:**
- Modify: `server/Cargo.toml`
- Modify: `server/src/config.rs`
- Create: `server/src/app/secrets.rs`
- Modify: `server/src/app/mod.rs`
- Create: `migrations/005_create_user_llm_settings.sql`
- Create: `server/src/domain/llm_settings.rs`
- Modify: `server/src/domain/mod.rs`
- Create: `server/src/domain/ports/llm_settings_repo.rs`
- Modify: `server/src/domain/ports/mod.rs`
- Create: `server/src/adapters/postgres/llm_settings_repo.rs`
- Modify: `server/src/adapters/postgres/mod.rs`
- Modify: `.env.example`

**Step 1: Add the encryption dependency and config key**

Update `server/Cargo.toml` to add a small AEAD dependency:

```toml
ring = "0.17"
```

Update `server/src/config.rs`:

```rust
pub struct Config {
    ...
    pub llm_settings_encryption_key: String,
}
```

```rust
llm_settings_encryption_key: env::var("LLM_SETTINGS_ENCRYPTION_KEY")
    .expect("LLM_SETTINGS_ENCRYPTION_KEY required"),
```

Append this to `.env.example`:

```dotenv
LLM_SETTINGS_ENCRYPTION_KEY=
```

**Step 2: Create the encryption helper**

Create `server/src/app/secrets.rs`:

```rust
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use ring::rand::{SecureRandom, SystemRandom};

pub struct SecretBox {
    key: LessSafeKey,
    random: SystemRandom,
}

impl SecretBox {
    pub fn new(raw_key: &str) -> Self { /* decode/validate key bytes here */ }

    pub fn encrypt(&self, plaintext: &str) -> Result<Vec<u8>, String> { /* prefix nonce to ciphertext */ }

    pub fn decrypt(&self, blob: &[u8]) -> Result<String, String> { /* split nonce + ciphertext */ }
}
```

Export it from `server/src/app/mod.rs`:

```rust
pub mod secrets;
```

**Step 3: Create the migration and domain model**

Create `migrations/005_create_user_llm_settings.sql`:

```sql
CREATE TABLE user_llm_settings (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    anthropic_api_key_encrypted BYTEA,
    anthropic_model TEXT NOT NULL DEFAULT 'claude-haiku-4-5',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

Create `server/src/domain/llm_settings.rs`:

```rust
use chrono::{DateTime, Utc};
use uuid::Uuid;

pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-haiku-4-5";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LlmSettings {
    pub user_id: Uuid,
    pub enabled: bool,
    pub anthropic_api_key_encrypted: Option<Vec<u8>>,
    pub anthropic_model: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

Export it from `server/src/domain/mod.rs`.

**Step 4: Define the repository contract with keep, replace, and clear semantics**

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
        replace_anthropic_api_key_encrypted: Option<&[u8]>,
        clear_anthropic_api_key: bool,
        anthropic_model: &str,
    ) -> Result<LlmSettings, DomainError>;
}
```

Export it from `server/src/domain/ports/mod.rs`.

**Step 5: Implement the PostgreSQL adapter**

Create `server/src/adapters/postgres/llm_settings_repo.rs`:

```rust
impl LlmSettingsRepository for PostgresPool {
    async fn get(&self, user_id: Uuid) -> Result<Option<LlmSettings>, DomainError> { /* SELECT ... */ }

    async fn upsert(
        &self,
        user_id: Uuid,
        enabled: bool,
        replace_anthropic_api_key_encrypted: Option<&[u8]>,
        clear_anthropic_api_key: bool,
        anthropic_model: &str,
    ) -> Result<LlmSettings, DomainError> {
        sqlx::query_as::<_, LlmSettings>(
            "INSERT INTO user_llm_settings (user_id, enabled, anthropic_api_key_encrypted, anthropic_model)
             VALUES ($1, $2, $3, $5)
             ON CONFLICT (user_id) DO UPDATE
             SET enabled = EXCLUDED.enabled,
                 anthropic_api_key_encrypted = CASE
                     WHEN $4 THEN NULL
                     WHEN $3 IS NOT NULL THEN $3
                     ELSE user_llm_settings.anthropic_api_key_encrypted
                 END,
                 anthropic_model = EXCLUDED.anthropic_model,
                 updated_at = now()
             RETURNING user_id, enabled, anthropic_api_key_encrypted, anthropic_model, created_at, updated_at",
        )
        .bind(user_id)
        .bind(enabled)
        .bind(replace_anthropic_api_key_encrypted)
        .bind(clear_anthropic_api_key)
        .bind(anthropic_model)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))
    }
}
```

Wire it into `server/src/adapters/postgres/mod.rs`.

**Step 6: Run the build for the new crypto/config/persistence layer**

Run:

```bash
cargo build -p boopmark-server
```

Expected:
- The new config field, crypto helper, migration, domain model, and repository compile cleanly.
- The plan no longer depends on plaintext secret storage.

**Step 7: Commit the encrypted persistence layer**

```bash
git add server/Cargo.toml server/src/config.rs server/src/app/secrets.rs server/src/app/mod.rs migrations/005_create_user_llm_settings.sql server/src/domain/mod.rs server/src/domain/llm_settings.rs server/src/domain/ports/mod.rs server/src/domain/ports/llm_settings_repo.rs server/src/adapters/postgres/mod.rs server/src/adapters/postgres/llm_settings_repo.rs .env.example
git commit -m "feat: add encrypted llm settings persistence"
```

### Task 3: Add a Settings service with replace-and-clear key semantics

Keep the page handler thin. This task owns default model normalization, encrypted secret replacement, and explicit secret clearing.

**Files:**
- Create: `server/src/app/settings.rs`
- Modify: `server/src/web/state.rs`
- Modify: `server/src/main.rs`

**Step 1: Create the settings service and helper types**

Create `server/src/app/settings.rs`:

```rust
use crate::app::secrets::SecretBox;
use crate::domain::error::DomainError;
use crate::domain::llm_settings::{DEFAULT_ANTHROPIC_MODEL, LlmSettings};
use crate::domain::ports::llm_settings_repo::LlmSettingsRepository;
use std::sync::Arc;
use uuid::Uuid;

pub struct SettingsService<R> {
    repo: Arc<R>,
    secret_box: Arc<SecretBox>,
}

pub struct SettingsView {
    pub enabled: bool,
    pub has_anthropic_api_key: bool,
    pub anthropic_model: String,
}

pub struct SaveLlmSettingsInput {
    pub enabled: bool,
    pub anthropic_api_key: Option<String>,
    pub clear_anthropic_api_key: bool,
    pub anthropic_model: Option<String>,
}

impl<R> SettingsService<R>
where
    R: LlmSettingsRepository + Send + Sync,
{
    pub fn new(repo: Arc<R>, secret_box: Arc<SecretBox>) -> Self {
        Self { repo, secret_box }
    }

    pub async fn load(&self, user_id: Uuid) -> Result<SettingsView, DomainError> {
        let settings = self.repo.get(user_id).await?;
        Ok(to_view(settings.as_ref()))
    }

    pub async fn save(&self, user_id: Uuid, input: SaveLlmSettingsInput) -> Result<SettingsView, DomainError> {
        let normalized_model = normalize_model(input.anthropic_model);
        let key_change = resolve_api_key_change(input.anthropic_api_key, input.clear_anthropic_api_key);

        let (replace_key, clear_key) = match key_change {
            ApiKeyChange::KeepExisting => (None, false),
            ApiKeyChange::Clear => (None, true),
            ApiKeyChange::Replace(value) => (
                Some(
                    self.secret_box
                        .encrypt(&value)
                        .map_err(DomainError::InvalidInput)?,
                ),
                false,
            ),
        };

        let saved = self
            .repo
            .upsert(
                user_id,
                input.enabled,
                replace_key.as_deref(),
                clear_key,
                &normalized_model,
            )
            .await?;

        Ok(to_view(Some(&saved)))
    }
}

enum ApiKeyChange {
    KeepExisting,
    Clear,
    Replace(String),
}
```

Add `normalize_model`, `resolve_api_key_change`, and `to_view` helpers in the same file.

**Step 2: Add focused unit tests in `server/src/app/settings.rs`**

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
    fn blank_key_keeps_existing_key() {
        assert!(matches!(
            resolve_api_key_change(Some("   ".into()), false),
            ApiKeyChange::KeepExisting
        ));
    }

    #[test]
    fn clear_checkbox_removes_saved_key() {
        assert!(matches!(
            resolve_api_key_change(None, true),
            ApiKeyChange::Clear
        ));
    }

    #[test]
    fn non_blank_key_replaces_saved_key() {
        assert!(matches!(
            resolve_api_key_change(Some("sk-ant-new".into()), false),
            ApiKeyChange::Replace(_)
        ));
    }
}
```

**Step 3: Wire the service into app state**

Update `server/src/web/state.rs`:

```rust
use crate::app::settings::SettingsService;

pub struct AppState {
    pub bookmarks: Bookmarks,
    pub auth: Arc<AuthService<PostgresPool, PostgresPool, PostgresPool>>,
    pub settings: Arc<SettingsService<PostgresPool>>,
    pub config: Arc<Config>,
}
```

Update `server/src/main.rs`:

```rust
use app::secrets::SecretBox;
use app::settings::SettingsService;

let secret_box = Arc::new(SecretBox::new(&config.llm_settings_encryption_key));
let settings_service = Arc::new(SettingsService::new(db.clone(), secret_box));
```

**Step 4: Run the focused Rust tests**

Run:

```bash
cargo test -p boopmark-server settings::tests
```

Expected:
- Default-model normalization is locked in.
- Blank submission keeps the saved key.
- The explicit clear control removes the key.
- Replacing the key takes the encryption path instead of storing plaintext.

**Step 5: Commit the service layer**

```bash
git add server/src/app/settings.rs server/src/web/state.rs server/src/main.rs
git commit -m "feat: add llm settings service"
```

### Task 4: Replace the API Keys stub with the real Settings page and save flow

Land the user-facing change here: `/settings`, `Settings` in the menu, `LLM Integration` on the page, GET + POST handlers, default model rendering, clear-key UI, and a redirect from the old `/settings/api-keys` path. Because this repo serves the checked-in Tailwind artifact, rebuilding `static/css/output.css` is part of the task.

**Files:**
- Modify: `server/src/web/pages/settings.rs`
- Modify: `templates/components/header.html`
- Create: `templates/settings/index.html`
- Delete: `templates/settings/api_keys.html`
- Modify: `static/css/output.css`

**Step 1: Replace the page handler with GET + POST settings routes**

Rewrite `server/src/web/pages/settings.rs`:

```rust
use askama::Template;
use axum::Form;
use axum::Router;
use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse, Redirect};
use serde::Deserialize;

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
}

#[derive(Deserialize, Default)]
struct SettingsQuery {
    saved: Option<String>,
}

#[derive(Deserialize)]
struct SettingsForm {
    llm_enabled: Option<String>,
    anthropic_api_key: Option<String>,
    clear_anthropic_api_key: Option<String>,
    anthropic_model: Option<String>,
}

async fn save_settings(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Form(form): Form<SettingsForm>,
) -> axum::response::Response {
    let enabled = form.llm_enabled.is_some();
    let clear_anthropic_api_key = form.clear_anthropic_api_key.is_some();

    match state
        .settings
        .save(
            user.id,
            crate::app::settings::SaveLlmSettingsInput {
                enabled,
                anthropic_api_key: form.anthropic_api_key,
                clear_anthropic_api_key,
                anthropic_model: form.anthropic_model,
            },
        )
        .await
    {
        Ok(_) => Redirect::to("/settings?saved=1").into_response(),
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn legacy_api_keys_redirect() -> Redirect {
    Redirect::to("/settings")
}
```

Keep the GET handler loading `state.settings.load(user.id).await` and mapping it into `SettingsPage`. Do not add `.env` reads here.

**Step 2: Rename the header menu item**

Update `templates/components/header.html`:

```html
<a href="/settings" data-testid="profile-menu-settings" class="block text-sm text-gray-300 hover:text-white py-1">Settings</a>
```

Remove `profile-menu-api-keys` entirely.

**Step 3: Create the real settings template with clear-key UI**

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
                        class="w-full px-4 py-2 rounded-lg bg-[#1a1d2e] border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500"
                    >
                    {% if has_anthropic_api_key %}
                    <p class="text-xs text-gray-400">Anthropic API key saved.</p>
                    <label class="flex items-center gap-2 text-xs text-gray-300">
                        <input type="checkbox" name="clear_anthropic_api_key">
                        Clear saved Anthropic API key
                    </label>
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

**Step 4: Rebuild the checked-in Tailwind artifact**

Run:

```bash
just css-build
```

Expected:
- `static/css/output.css` is regenerated.
- The new layout, spacing, flash-message, and clear-key control utilities are present in the served CSS artifact.

**Step 5: Run the focused build and browser suites**

Run:

```bash
cargo build -p boopmark-server
npx playwright test tests/e2e/profile-menu.spec.js tests/e2e/settings.spec.js
```

Expected:
- The app builds successfully with the encrypted settings service and routes.
- The profile menu navigates to `/settings`.
- The new settings workflow passes, including default model rendering, save/reload behavior, and explicit key clearing.

**Step 6: Commit the user-facing settings page**

```bash
git add server/src/web/pages/settings.rs templates/components/header.html templates/settings/index.html static/css/output.css
git rm templates/settings/api_keys.html
git commit -m "feat: add llm integration settings page"
```

### Task 5: Run the final regression pass

The shared header, settings route, CSS artifact, migration path, and local test helper all touch authenticated flows. Finish with the smallest full pass that can catch cross-feature breakage.

**Files:**
- Modify: only files required by any follow-up fixes discovered here

**Step 1: Run the server test suite**

Run:

```bash
cargo test -p boopmark-server
```

Expected:
- The existing bookmark and scraper tests still pass.
- The new settings and encryption helper tests pass.

**Step 2: Run the targeted Playwright regression suite**

Run:

```bash
npx playwright test tests/e2e/profile-menu.spec.js tests/e2e/settings.spec.js tests/e2e/suggest.spec.js
```

Expected:
- `profile-menu.spec.js` passes with the renamed Settings destination.
- `settings.spec.js` passes using the real copied worktree `.env` key.
- `suggest.spec.js` still passes, proving the shared authenticated shell still works.

**Step 3: Rebuild CSS again only if follow-up template changes introduced new classes**

Run:

```bash
just css-build
```

Expected:
- `static/css/output.css` stays in sync with any last-minute template adjustments.

**Step 4: If a regression requires a code change, make the smallest fix and re-run the affected checks**

Run only the smallest commands needed after each follow-up fix, for example:

```bash
cargo test -p boopmark-server
npx playwright test tests/e2e/profile-menu.spec.js tests/e2e/settings.spec.js tests/e2e/suggest.spec.js
just css-build
```

Expected:
- Every failing regression is fixed before the branch is finalized.
- No speculative cleanup lands here.

**Step 5: Commit only if the regression pass forced a real code change**

```bash
git add -A
git commit -m "test: verify settings llm integration regressions"
```
