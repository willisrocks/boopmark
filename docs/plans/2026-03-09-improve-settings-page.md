# Settings Page UX Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Make `/settings` clearly reflect saved Anthropic configuration, limit model selection to the current official Anthropic model IDs, and render the page inside BoopMark's normal app shell without breaking encrypted settings persistence.

**Architecture:** Keep the existing encrypted `user_llm_settings` storage and route structure, but move Anthropic model handling to a server-owned allow-list so the service, template, and tests all use the same labels and full API values. Reuse the existing header component on Settings by passing the same user view data plus a simple page-context flag so BoopMark branding and navigation stay visible while header controls remain safe off the bookmarks grid. Preserve the current keep/replace/clear key semantics: the UI never rehydrates the secret, it only shows saved state, replace state, or cleared state.

**Tech Stack:** Rust, Axum, Askama templates, Tailwind CSS, Playwright

---

Anthropic model source already confirmed from the official live docs on March 9, 2026: `claude-opus-4-6`, `claude-sonnet-4-6`, and `claude-haiku-4-5-20251001`. Keep Haiku as the default family, but update the stored default to the full current identifier `claude-haiku-4-5-20251001`.

### Task 1: Refresh the worktree `.env` and capture failing browser expectations

Start with the browser contract so the implementation is driven by the user-visible behavior: saved-key masking, explicit replace/clear flow, current model select options, and the restored app shell on `/settings`.

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

await expect(page.getByText("Saved Anthropic API key is active")).toBeVisible();
await expect(page.getByLabel("Anthropic API key")).toBeDisabled();
await page.getByText("Replace saved key").click();
await expect(page.getByTestId("anthropic-api-key-replacement")).toBeVisible();
await page.getByLabel("Replacement Anthropic API key").fill(anthropicApiKey);
await page.getByRole("button", { name: "Save settings" }).click();
```

Finish that test by asserting the clear flow is explicit:

```js
await expect(page.getByLabel("Clear saved Anthropic API key on save")).toBeVisible();
await page.getByLabel("Clear saved Anthropic API key on save").check();
await page.getByRole("button", { name: "Save settings" }).click();

await expect(page.getByText("No Anthropic API key saved yet.")).toBeVisible();
await expect(page.getByLabel("Anthropic API key")).toBeEditable();
```

Keep the helper that reads `ANTHROPIC_API_KEY` from the copied worktree `.env`; do not add a fake fallback.

**Step 3: Run the browser spec and verify it fails on current code**

Run:

```bash
npx playwright test tests/e2e/settings.spec.js
```

Expected:
- FAIL because the current page does not include the shared header.
- FAIL because the current page still uses a free-text model input instead of a `select`.
- FAIL because the current page still shows an empty editable password input even when a key is saved.

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
fn normalize_model_maps_the_previous_haiku_alias_to_the_current_full_id() {
    assert_eq!(
        normalize_model(Some("claude-haiku-4-5".into())),
        "claude-haiku-4-5-20251001"
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

**Step 2: Run the unit tests and verify they fail**

Run:

```bash
cargo test -p boopmark-server app::settings -- --nocapture
```

Expected:
- FAIL because the default model constant is still `claude-haiku-4-5`.
- FAIL because there is no shared allow-list for the three official model IDs.

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

Update `normalize_model` in `server/src/app/settings.rs`:

```rust
fn normalize_model(model: Option<String>) -> String {
    match model.as_deref().map(str::trim) {
        Some("claude-haiku-4-5") => DEFAULT_ANTHROPIC_MODEL.to_string(),
        Some(value) if ANTHROPIC_MODEL_OPTIONS.iter().any(|option| option.value == value) => {
            value.to_string()
        }
        Some(value) if !value.is_empty() => value.to_string(),
        _ => DEFAULT_ANTHROPIC_MODEL.to_string(),
    }
}
```

Keep the last branch so older custom values already stored from the free-text era are not silently destroyed before the user intentionally changes them.

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
    label: &'static str,
    value: &'static str,
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

Populate `user: Some(user.clone().into())`, `header_shows_bookmark_actions: false`, and build `anthropic_model_options` from `ANTHROPIC_MODEL_OPTIONS`, prepending one extra selected option only when the saved value is a legacy custom value that is not in the official list:

```rust
ModelOptionView {
    label: "Saved custom model (change to replace)",
    value: current_model.clone(),
    selected: true,
}
```

That avoids silently overwriting old free-text values while still making the new official choices primary.

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

**Step 3: Replace the isolated settings card behavior with the new form UX**

Update `templates/settings/index.html` to include the shared header and render the new key state:

```html
{% include "components/header.html" %}
<main class="max-w-3xl mx-auto px-6 py-8">
```

Render the saved-key branch like this:

```html
{% if has_anthropic_api_key %}
<div class="rounded-lg border border-gray-700 bg-[#1a1d2e] px-4 py-3">
    <p class="text-sm font-medium text-gray-200">Saved Anthropic API key is active.</p>
    <p class="text-xs text-gray-400 mt-1">Use Replace to enter a new key, or clear it on save to make the field editable again.</p>
</div>
<input
    id="anthropic_api_key"
    type="password"
    value="sk-ant-••••••••••••"
    disabled
    class="w-full ..."
>
<details class="space-y-2">
    <summary class="cursor-pointer text-sm text-blue-300">Replace saved key</summary>
    <div data-testid="anthropic-api-key-replacement" class="space-y-2">
        <label for="anthropic_api_key_replacement" class="block text-sm font-medium text-gray-200">Replacement Anthropic API key</label>
        <input id="anthropic_api_key_replacement" name="anthropic_api_key" type="password" autocomplete="off" class="w-full ..." />
    </div>
</details>
<label class="flex items-start gap-2 text-xs text-gray-300">
    <input type="checkbox" name="clear_anthropic_api_key" aria-label="Clear saved Anthropic API key on save">
    <span>Clear saved key on save. After saving, the API key field will be editable again.</span>
</label>
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

This keeps the page truthful: saved key state is visible, direct editing is opt-in, and official model choices use friendly labels with full saved IDs.

**Step 4: Run the targeted browser and backend tests**

Run:

```bash
cargo test -p boopmark-server app::settings -- --nocapture
npx playwright test tests/e2e/settings.spec.js tests/e2e/profile-menu.spec.js
```

Expected:
- PASS for the settings service unit tests.
- PASS for the settings E2E flow, including saved-key masking, replace, clear, and the model select.
- PASS for the profile-menu navigation smoke test, confirming the settings page still looks like BoopMark and remains reachable from the normal shell.
- PASS for `/bookmarks` rendering with the unchanged live-search/add-bookmark header branch because `GridPage` now explicitly supplies `header_shows_bookmark_actions: true`.

**Step 5: Commit the UI and shell change**

```bash
git add server/src/web/pages/shared.rs server/src/web/pages/mod.rs templates/components/header.html templates/settings/index.html server/src/web/pages/bookmarks.rs server/src/web/pages/settings.rs
git commit -m "feat: improve settings page shell and llm ux"
```
