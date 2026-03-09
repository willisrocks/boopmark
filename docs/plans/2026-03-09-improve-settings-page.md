# Settings Page UX Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Make `/settings` clearly reflect saved Anthropic configuration, offer the current official Anthropic model IDs for new selections, and render the page inside BoopMark's normal app shell without breaking encrypted settings persistence.

**Architecture:** Keep the existing encrypted `user_llm_settings` storage and route structure, but move the official Anthropic choices to a server-owned allow-list so the service, template, and tests all use the same labels and full API values. Do not silently migrate or overwrite previously saved non-official model strings: if a user already has one stored, render it as the current selected value alongside the three current official options and preserve it on unrelated saves until the user explicitly chooses a different model. Enforce that distinction on save: new submissions may only choose one of the official allow-listed values, except that the currently stored legacy/custom value may be re-submitted unchanged so unrelated saves do not destroy it, and omitted or blank `anthropic_model` input must preserve the existing stored value instead of defaulting it away. Only fall back to `claude-haiku-4-5-20251001` when creating a brand-new settings record with no saved model yet. Reuse the existing header component on Settings by passing the same user view data plus a simple page-context flag so BoopMark branding and navigation stay visible while header controls remain safe off the bookmarks grid. Preserve the current keep/replace/clear key semantics: the UI never rehydrates the secret, it only shows truthful saved-state messaging, replace state, or cleared state.

**Tech Stack:** Rust, Axum, Askama templates, Tailwind CSS, Playwright

---

Anthropic model source already confirmed from the official live docs on March 9, 2026: `claude-opus-4-6`, `claude-sonnet-4-6`, and `claude-haiku-4-5-20251001`. Keep Haiku as the default family, but update the stored default to the full current identifier `claude-haiku-4-5-20251001`.

### Task 1: Refresh the worktree `.env` and capture failing browser expectations

Start with the browser contract so the implementation is driven by the user-visible behavior: truthful saved-key status, a single explicit keep/replace/clear key flow, current model select options, and the restored app shell on `/settings`.

**Files:**
- Modify: `tests/e2e/settings.spec.js`

**Step 1: Refresh the copied `.env` in this worktree before any implementation work**

Run:

```bash
cp /Users/chrisfenton/Code/personal/boopmark/.env /Users/chrisfenton/Code/personal/boopmark/.worktrees/improve-settings-page/.env
test -f /Users/chrisfenton/Code/personal/boopmark/.worktrees/improve-settings-page/.env
rg '^ANTHROPIC_API_KEY=' /Users/chrisfenton/Code/personal/boopmark/.worktrees/improve-settings-page/.env
```

Expected:
- PASS.
- The worktree copy exists before any code changes are implemented.
- `ANTHROPIC_API_KEY` is available for the Playwright workflow in this worktree.

This file stays untracked. Do not skip this step during execution.

**Step 2: Rewrite the settings E2E spec around the new UX**

Update `tests/e2e/settings.spec.js` so it asserts:

```js
await expect(page.getByRole("banner")).toBeVisible();
await expect(page.getByRole("link", { name: "BoopMark" })).toHaveAttribute("href", "/bookmarks");
await expect(page.getByLabel("Anthropic model")).toHaveValue("claude-haiku-4-5-20251001");
await expect(page.getByRole("option", { name: "Claude Opus 4.6" })).toHaveValue("claude-opus-4-6");
await expect(page.getByRole("option", { name: "Claude Sonnet 4.6" })).toHaveValue("claude-sonnet-4-6");
await expect(page.getByRole("option", { name: "Claude Haiku 4.5" })).toHaveValue("claude-haiku-4-5-20251001");
```

Add one full saved-key workflow test:

```js
await page.getByLabel("Enable LLM integration").check();
await page.getByLabel("Anthropic API key").fill(anthropicApiKey);
await page.getByLabel("Anthropic model").selectOption("claude-sonnet-4-6");
await page.getByRole("button", { name: "Save settings" }).click();

await expect(page.getByTestId("anthropic-api-key-status")).toBeVisible();
await expect(page.getByText("Anthropic API key saved securely")).toBeVisible();
await expect(page.getByLabel("Anthropic API key")).toHaveCount(0);
await expect(page.getByLabel("Keep current saved key")).toBeChecked();
await page.getByLabel("Replace saved key").check();
await expect(page.getByTestId("anthropic-api-key-replacement")).toBeVisible();
await page.getByLabel("Replacement Anthropic API key").fill(anthropicApiKey);
await page.getByLabel("Clear saved key").check();
await expect(page.getByTestId("anthropic-api-key-replacement")).toHaveCount(0);
await expect(page.getByText("Saving will remove the stored Anthropic API key.")).toBeVisible();
await page.getByRole("button", { name: "Save settings" }).click();
```

Finish that test by asserting the clear flow is explicit:

```js
await expect(page.getByText("No Anthropic API key saved yet.")).toBeVisible();
await expect(page.getByLabel("Anthropic API key")).toBeEditable();
```

Keep the helper that reads `ANTHROPIC_API_KEY` from the copied worktree `.env`; do not add a fake fallback.

Also add one browser assertion for the fresh/default path after `resetSettings(page)` has already saved an official model value:

```js
await expect(page.locator("#anthropic_model option")).toHaveValues([
  "claude-opus-4-6",
  "claude-sonnet-4-6",
  "claude-haiku-4-5-20251001",
]);
```

Document in the test name or comments that this exact-three-options assertion is for the normal official-only path. The preserved-legacy path is covered separately in unit tests because the E2E reset helper intentionally leaves the account on an official model before the main assertions run.

Add one authenticated request-level browser test for the invalid-model HTTP contract:

```js
await signIn(page);
const status = await page.evaluate(async () => {
  const response = await fetch("/settings", {
    method: "POST",
    headers: {
      "Content-Type": "application/x-www-form-urlencoded",
    },
    body: new URLSearchParams({
      llm_enabled: "on",
      anthropic_model: "claude-3-7-sonnet-latest",
    }),
  });
  return response.status;
});

expect(status).toBe(400);
```

**Step 3: Run the browser spec and verify it fails on current code**

Run:

```bash
npx playwright test tests/e2e/settings.spec.js
```

Expected:
- FAIL because the current page does not include the shared header.
- FAIL because the current page still uses a free-text model input instead of a `select`.
- FAIL because the current page still shows an empty editable password input even when a key is saved.
- FAIL because the current page still exposes contradictory replace-and-clear API key actions.
- FAIL because the invalid-model POST still returns the wrong status code.

**Step 4: Commit the failing browser coverage**

```bash
git add tests/e2e/settings.spec.js
git commit -m "test: capture settings page ux expectations"
```

### Task 2: Lock backend behavior with unit tests before touching the page

Drive the service changes from tests so the new UI stays backed by explicit model metadata and stable keep/replace/clear semantics.

**Files:**
- Modify: `server/src/domain/llm_settings.rs`
- Modify: `server/src/app/settings.rs`

**Step 1: Add failing unit tests for model metadata and normalization**

Extend `server/src/app/settings.rs` tests with assertions like:

```rust
#[test]
fn normalize_model_defaults_to_latest_full_haiku_id() {
    assert_eq!(normalize_model(None), "claude-haiku-4-5-20251001");
    assert_eq!(normalize_model(Some("   ".into())), "claude-haiku-4-5-20251001");
}

#[test]
fn normalize_model_accepts_the_current_official_model_ids() {
    assert_eq!(normalize_model(Some("claude-opus-4-6".into())), "claude-opus-4-6");
    assert_eq!(normalize_model(Some("claude-sonnet-4-6".into())), "claude-sonnet-4-6");
    assert_eq!(
        normalize_model(Some("claude-haiku-4-5-20251001".into())),
        "claude-haiku-4-5-20251001"
    );
}

#[test]
fn normalize_model_preserves_a_preexisting_custom_value() {
    assert_eq!(
        normalize_model(Some("claude-3-7-sonnet-latest".into())),
        "claude-3-7-sonnet-latest"
    );
}
```

Add a view-level assertion so saved state remains presence-only:

```rust
assert!(view.has_anthropic_api_key);
assert_eq!(view.anthropic_model, "claude-sonnet-4-6");
```

Add a metadata test in `server/src/domain/llm_settings.rs` for the shared allow-list:

```rust
assert_eq!(DEFAULT_ANTHROPIC_MODEL, "claude-haiku-4-5-20251001");
assert_eq!(ANTHROPIC_MODEL_OPTIONS.len(), 3);
```

Add one load-path test proving an unsupported stored value is preserved before rendering:

```rust
repo.stored.lock().expect("stored lock").replace(LlmSettings {
    user_id,
    enabled: true,
    anthropic_api_key_encrypted: Some(vec![1, 2, 3]),
    anthropic_model: "claude-3-7-sonnet-latest".into(),
    created_at: Utc::now(),
    updated_at: Utc::now(),
});

let view = service.load(user_id).await.expect("load");
assert_eq!(view.anthropic_model, "claude-3-7-sonnet-latest");
```

Add one save-path test proving unrelated saves keep a preserved current model value intact:

```rust
repo.stored.lock().expect("stored lock").replace(LlmSettings {
    user_id,
    enabled: true,
    anthropic_api_key_encrypted: Some(vec![1, 2, 3]),
    anthropic_model: "claude-3-7-sonnet-latest".into(),
    created_at: Utc::now(),
    updated_at: Utc::now(),
});

let view = service
    .save(
        user_id,
        SaveLlmSettingsInput {
            enabled: true,
            anthropic_api_key: None,
            clear_anthropic_api_key: false,
            anthropic_model: Some("claude-3-7-sonnet-latest".into()),
        },
    )
    .await
    .expect("save");

assert_eq!(view.anthropic_model, "claude-3-7-sonnet-latest");
```

Add one omitted-field save-path test proving an unrelated save preserves the existing model instead of defaulting it:

```rust
repo.stored.lock().expect("stored lock").replace(LlmSettings {
    user_id,
    enabled: true,
    anthropic_api_key_encrypted: Some(vec![1, 2, 3]),
    anthropic_model: "claude-3-7-sonnet-latest".into(),
    created_at: Utc::now(),
    updated_at: Utc::now(),
});

let view = service
    .save(
        user_id,
        SaveLlmSettingsInput {
            enabled: false,
            anthropic_api_key: None,
            clear_anthropic_api_key: true,
            anthropic_model: None,
        },
    )
    .await
    .expect("save");

assert_eq!(view.anthropic_model, "claude-3-7-sonnet-latest");
```

Add one blank-field save-path test proving a blank submitted model also preserves the existing value:

```rust
let view = service
    .save(
        user_id,
        SaveLlmSettingsInput {
            enabled: true,
            anthropic_api_key: None,
            clear_anthropic_api_key: false,
            anthropic_model: Some("   ".into()),
        },
    )
    .await
    .expect("save");

assert_eq!(view.anthropic_model, "claude-3-7-sonnet-latest");
```

Add one save-path test proving a newly submitted unsupported model is rejected instead of being written through:

```rust
let err = service
    .save(
        user_id,
        SaveLlmSettingsInput {
            enabled: true,
            anthropic_api_key: None,
            clear_anthropic_api_key: false,
            anthropic_model: Some("claude-3-7-sonnet-latest".into()),
        },
    )
    .await
    .expect_err("unsupported submitted model should fail");

assert!(matches!(err, DomainError::InvalidInput(_)));
```

**Step 2: Run the unit tests and verify they fail**

Run:

```bash
cargo test -p boopmark-server app::settings -- --nocapture
```

Expected:
- FAIL because the default model constant is still `claude-haiku-4-5`.
- FAIL because there is no shared allow-list for the three official model IDs.
- FAIL because there is no explicit test coverage protecting existing custom model values from being overwritten on load or unrelated saves.
- FAIL because omitted or blank `anthropic_model` still overwrites an existing saved model with the default instead of preserving it.
- FAIL because a newly submitted unsupported model still saves successfully instead of being rejected.

**Step 3: Implement the shared model metadata and normalization**

Update `server/src/domain/llm_settings.rs` to own the official model metadata:

```rust
pub struct AnthropicModelOption {
    pub label: &'static str,
    pub value: &'static str,
}

pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-haiku-4-5-20251001";

pub const ANTHROPIC_MODEL_OPTIONS: [AnthropicModelOption; 3] = [
    AnthropicModelOption {
        label: "Claude Opus 4.6",
        value: "claude-opus-4-6",
    },
    AnthropicModelOption {
        label: "Claude Sonnet 4.6",
        value: "claude-sonnet-4-6",
    },
    AnthropicModelOption {
        label: "Claude Haiku 4.5",
        value: "claude-haiku-4-5-20251001",
    },
];
```

Keep `normalize_model` for load/display normalization only:

```rust
fn normalize_model(model: Option<String>) -> String {
    match model {
        Some(value) if !value.trim().is_empty() => value.trim().to_string(),
        _ => DEFAULT_ANTHROPIC_MODEL.to_string(),
    }
}
```

Use that helper only for display and for brand-new records. Do not reuse it directly for save-path validation, because omitted or blank form values on an existing record must preserve the stored model instead of silently switching to the default.

Add a dedicated save-path validator in `server/src/app/settings.rs` that enforces the allow-list while still permitting the already-stored legacy value to round-trip unchanged:

```rust
fn resolve_model_for_save(
    existing: Option<&LlmSettings>,
    submitted: Option<String>,
) -> Result<String, DomainError> {
    match submitted.as_deref().map(str::trim) {
        None | Some("") => {
            if let Some(settings) = existing {
                return Ok(settings.anthropic_model.trim().to_string());
            }
            Ok(DEFAULT_ANTHROPIC_MODEL.to_string())
        }
        Some(value)
            if ANTHROPIC_MODEL_OPTIONS
                .iter()
                .any(|option| option.value == value) =>
        {
            Ok(value.to_string())
        }
        Some(value)
            if existing
                .map(|settings| settings.anthropic_model.trim() == value)
                .unwrap_or(false) =>
        {
            Ok(value.to_string())
        }
        Some(_) => Err(DomainError::InvalidInput(
            "Unsupported Anthropic model selection".into(),
        )),
    }
}
```

Update `SettingsService::save` to load the existing settings before validation, call `resolve_model_for_save(existing.as_ref(), input.anthropic_model)`, and preserve the stored model whenever the request omits or blanks `anthropic_model`. Only a new record should default to `claude-haiku-4-5-20251001`, and only an explicit submitted official option should replace the current value. Return `DomainError::InvalidInput` for any forged unsupported submission that is not the already-stored current value.

**Step 4: Run the unit tests and verify they pass**

Run:

```bash
cargo test -p boopmark-server app::settings -- --nocapture
```

Expected:
- PASS for all settings service tests.
- The service now defaults to the full current Haiku ID and keeps encrypted key semantics intact.

**Step 5: Commit the backend behavior lock**

```bash
git add server/src/domain/llm_settings.rs server/src/app/settings.rs
git commit -m "feat: add supported anthropic model metadata"
```

### Task 3: Render Settings inside the shared shell and implement the new saved-key UX

Reuse the existing header instead of creating a second Settings-only shell, but make the header template safe on non-bookmarks pages. The page should show the saved-key state by default, hide direct editing until the user chooses replace, and render the official model options from the shared server metadata.

**Files:**
- Create: `server/src/web/pages/shared.rs`
- Modify: `server/src/web/pages/mod.rs`
- Modify: `templates/components/header.html`
- Modify: `templates/settings/index.html`
- Modify: `server/src/web/pages/bookmarks.rs`
- Modify: `server/src/web/pages/settings.rs`

**Step 1: Introduce the page data the shared header and settings template need**

Create a real shared page module instead of trying to reuse the private `bookmarks.rs` struct in place. `templates/components/header.html` already renders `user.image`, `user.email_initial`, `user.display_name`, and `user.email`, so the shared view model must live in a module both page handlers can import and its fields must be visible to Askama code generated from both `bookmarks.rs` and `settings.rs`.

Create `server/src/web/pages/shared.rs`:

```rust
pub(crate) struct UserView {
    pub(crate) email: String,
    pub(crate) display_name: String,
    pub(crate) email_initial: String,
    pub(crate) image: Option<String>,
}

impl From<crate::domain::user::User> for UserView {
    fn from(u: crate::domain::user::User) -> Self {
        let email_initial = u.email.chars().next().unwrap_or('?').to_string();
        let display_name = u.name.clone().unwrap_or_default();
        Self {
            email: u.email,
            display_name,
            email_initial,
            image: u.image,
        }
    }
}
```

Register that module from `server/src/web/pages/mod.rs`:

```rust
pub(crate) mod shared;
```

Then update both page templates that include the shared header so the new branch is fully specified before `templates/components/header.html` changes:

```rust
#[derive(Template)]
#[template(path = "bookmarks/grid.html")]
struct GridPage {
    user: Option<UserView>,
    header_shows_bookmark_actions: bool,
    bookmarks: Vec<BookmarkView>,
    ...
}
```

```rust
render(&GridPage {
    user: Some(user.into()),
    header_shows_bookmark_actions: true,
    bookmarks: bookmark_views,
    ...
})
```

Update `SettingsPage` in `server/src/web/pages/settings.rs` to include the same header flag with the opposite value:

```rust
struct ModelOptionView {
    label: String,
    value: String,
    selected: bool,
}

#[derive(Template)]
#[template(path = "settings/index.html")]
struct SettingsPage {
    user: Option<UserView>,
    header_shows_bookmark_actions: bool,
    email: String,
    llm_enabled: bool,
    has_anthropic_api_key: bool,
    anthropic_model: String,
    anthropic_model_options: Vec<ModelOptionView>,
    success_message: Option<String>,
}
```

Update the form parsing in `server/src/web/pages/settings.rs` so API key intent is one explicit state instead of two conflicting controls:

```rust
#[derive(Deserialize)]
struct SettingsForm {
    llm_enabled: Option<String>,
    anthropic_api_key_action: Option<String>, // keep | replace | clear
    anthropic_api_key: Option<String>,
    anthropic_model: Option<String>,
}
```

Map that single form field in the POST handler:

```rust
let api_key_action = form
    .anthropic_api_key_action
    .as_deref()
    .unwrap_or("keep");

let (anthropic_api_key, clear_anthropic_api_key) = match api_key_action {
    "replace" => (form.anthropic_api_key, false),
    "clear" => (None, true),
    _ => (None, false),
};
```

Populate `user: Some(user.clone().into())`, `header_shows_bookmark_actions: false`, and build `anthropic_model_options` from a small helper in `server/src/web/pages/settings.rs`, prepending one selected preservation option only when the current saved value is not one of the three official values:

```rust
ModelOptionView {
    label: format!("Keep current saved model ({current_model})"),
    value: current_model.clone(),
    selected: true,
}
```

That preservation option is not a new recommendation and should appear only when the database already contains a non-official model value. Its job is to keep existing persistence intact until the user explicitly chooses one of the official options.

Add unit tests for that helper in `server/src/web/pages/settings.rs`:

```rust
#[test]
fn official_models_render_only_the_three_official_options() { ... }

#[test]
fn legacy_saved_model_gets_one_preservation_option_plus_the_official_options() { ... }
```

That keeps the legacy-preservation branch verifiable without making the main E2E path contradictory.

**Step 2: Make the shared header safe on `/settings`**

Update `templates/components/header.html` so:

```html
<header class="flex items-center justify-between px-6 py-3 border-b border-gray-800" role="banner">
    <a href="/bookmarks" class="flex items-center gap-2">
        <span class="text-red-500 text-xl">&#128278;</span>
        <span class="font-bold text-lg">BoopMark</span>
    </a>
    {% if header_shows_bookmark_actions %}
    <input ... hx-get="/bookmarks" hx-target="#bookmark-grid" ...>
    <button ... onclick="document.getElementById('add-modal').classList.remove('hidden')">+ Add Bookmark</button>
    {% else %}
    <form action="/bookmarks" method="get" class="flex-1 max-w-xl mx-8">
        <input type="search" name="search" placeholder="Search bookmarks..." class="..." />
    </form>
    <a href="/bookmarks" class="px-4 py-2 bg-blue-600 hover:bg-blue-700 rounded-lg text-sm font-medium">Back to bookmarks</a>
    {% endif %}
```

Do this only after `GridPage` is already supplying `header_shows_bookmark_actions: true`; otherwise `/bookmarks` will stop rendering. The settings branch keeps the header visible and gives a deterministic path back to the rest of the app without targeting a missing `#bookmark-grid` or missing add-bookmark modal.

While you are already in `server/src/web/pages/settings.rs`, update the POST handler to translate `DomainError::InvalidInput` from the new save validator into `400 Bad Request` instead of a generic `500`, so forged unsupported submissions are rejected cleanly.

**Step 3: Replace the isolated settings card behavior with the new form UX**

Update `templates/settings/index.html` to include the shared header and render the new key state:

```html
{% include "components/header.html" %}
<main class="max-w-3xl mx-auto px-6 py-8">
```

Render the saved-key branch like this:

```html
{% if has_anthropic_api_key %}
<div
    data-testid="anthropic-api-key-status"
    class="rounded-lg border border-gray-700 bg-[#1a1d2e] px-4 py-3"
>
    <p class="text-sm font-medium text-gray-200">Anthropic API key saved securely.</p>
    <p class="text-xs text-gray-400 mt-1">Choose one action below: keep using it, replace it, or remove it.</p>
</div>
<fieldset class="space-y-3">
    <legend class="text-sm font-medium text-gray-200">Saved key action</legend>
    <label class="flex items-start gap-2 text-sm text-gray-300">
        <input type="radio" name="anthropic_api_key_action" value="keep" aria-label="Keep current saved key" checked>
        <span>Keep current saved key</span>
    </label>
    <label class="flex items-start gap-2 text-sm text-gray-300">
        <input type="radio" name="anthropic_api_key_action" value="replace" aria-label="Replace saved key">
        <span>Replace saved key</span>
    </label>
    <div data-testid="anthropic-api-key-replacement" class="space-y-2">
        <label for="anthropic_api_key_replacement" class="block text-sm font-medium text-gray-200">Replacement Anthropic API key</label>
        <input id="anthropic_api_key_replacement" name="anthropic_api_key" type="password" autocomplete="off" class="w-full ..." />
    </div>
    <label class="flex items-start gap-2 text-sm text-gray-300">
        <input type="radio" name="anthropic_api_key_action" value="clear" aria-label="Clear saved key">
        <span>Clear saved key</span>
    </label>
    <p class="text-xs text-gray-400">Saving will remove the stored Anthropic API key.</p>
</fieldset>
{% else %}
<input id="anthropic_api_key" name="anthropic_api_key" type="password" autocomplete="off" class="w-full ..." />
<p class="text-xs text-gray-400">No Anthropic API key saved yet.</p>
{% endif %}
```

Replace the free-text model input with a select rendered from `anthropic_model_options`:

```html
<select id="anthropic_model" name="anthropic_model" class="w-full ...">
    {% for option in anthropic_model_options %}
    <option value="{{ option.value }}" {% if option.selected %}selected{% endif %}>{{ option.label }}</option>
    {% endfor %}
</select>
<p class="text-xs text-gray-400">Default: Claude Haiku 4.5 (`claude-haiku-4-5-20251001`).</p>
```

Use a small progressive-enhancement script or CSS toggle pattern tied to the selected radio so only the active branch is visible: replacement input only when `replace` is selected, clear confirmation only when `clear` is selected, and neither when `keep` is selected. That removes the contradictory replace-plus-clear state instead of relying on backend precedence to decide it.

This keeps the page truthful: saved key state is visible without inventing a pseudo-secret, the API key action is mutually exclusive and explicit, and official model choices use friendly labels with full saved IDs.

**Step 4: Run the targeted browser and backend tests**

Run:

```bash
cargo test -p boopmark-server app::settings -- --nocapture
npx playwright test tests/e2e/settings.spec.js tests/e2e/profile-menu.spec.js tests/e2e/suggest.spec.js
```

Expected:
- PASS for the settings service unit tests.
- PASS for the settings E2E flow, including truthful saved-key status, replace, clear, and the model select.
- PASS for the profile-menu navigation smoke test, confirming the settings page still looks like BoopMark and remains reachable from the normal shell.
- PASS for `suggest.spec.js`, confirming the shared bookmarks-header branch still opens the add modal and keeps the main app shell behavior intact after the header conditional is introduced.
- PASS for `/bookmarks` rendering with the unchanged live-search/add-bookmark header branch because `GridPage` now explicitly supplies `header_shows_bookmark_actions: true`.

**Step 5: Commit the UI and shell change**

```bash
git add server/src/web/pages/shared.rs server/src/web/pages/mod.rs templates/components/header.html templates/settings/index.html server/src/web/pages/bookmarks.rs server/src/web/pages/settings.rs
git commit -m "feat: improve settings page shell and llm ux"
```
