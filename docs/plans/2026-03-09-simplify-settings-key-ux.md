# Simplify Settings Key UX Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Replace the keep/replace/clear radio-button workflow for Anthropic API keys with a simple two-state UX: key exists (show status + delete button) or key absent (show input field).

**Architecture:** The change is purely in the web/presentation layer (Askama template + page handler) and its tests. The domain model, encrypted persistence layer, and `SettingsService` remain unchanged. The backend form handler is simplified: `anthropic_api_key_action` is removed; a new `delete_anthropic_api_key` checkbox replaces the `clear` action. Adding a key is the normal path when no key exists (just submit the form with a value in the input field).

**Tech Stack:** Rust/Axum handler, Askama HTML template, Playwright E2E tests, Tailwind CSS.

---

### Task 1: Simplify the Askama settings template

**Files:**
- Modify: `templates/settings/index.html`

**Step 1: Rewrite the API key section of the template**

Replace the entire `has_anthropic_api_key` conditional block (the fieldset with keep/replace/clear radios, the `<template>` elements, and the `<script>` block) with a simpler two-state design:

When `has_anthropic_api_key` is true:
- Show the existing status banner (`data-testid="anthropic-api-key-status"`) with text "Anthropic API key saved securely."
- Below it, show a labeled checkbox: `name="delete_anthropic_api_key"` with label text "Delete saved key". This is the only action available.
- No hidden input field, no radio buttons, no JS-driven template swapping.

When `has_anthropic_api_key` is false:
- Show the password input field (`name="anthropic_api_key"`, `id="anthropic_api_key"`) exactly as today.
- Show the "No Anthropic API key saved yet." helper text exactly as today.

Remove all three `<template>` elements and the entire `<script>` block at the bottom of the file. They are no longer needed since there are no dynamic radio-driven UI states.

The full replacement for the API key `<div class="space-y-2">` block:

```html
<div class="space-y-2">
    <label for="anthropic_api_key" class="block text-sm font-medium text-gray-200">Anthropic API key</label>
    {% if has_anthropic_api_key %}
    <div
        data-testid="anthropic-api-key-status"
        class="rounded-lg border border-gray-700 bg-[#1a1d2e] px-4 py-3"
    >
        <p class="text-sm font-medium text-gray-200">Anthropic API key saved securely.</p>
    </div>
    <label class="flex items-center gap-2 text-sm text-gray-300">
        <input type="checkbox" name="delete_anthropic_api_key" data-testid="delete-anthropic-api-key">
        <span>Delete saved key</span>
    </label>
    {% else %}
    <input
        id="anthropic_api_key"
        name="anthropic_api_key"
        type="password"
        autocomplete="off"
        class="w-full px-4 py-2 rounded-lg bg-[#1a1d2e] border border-gray-700 text-gray-200 focus:outline-none focus:border-blue-500"
    >
    <p class="text-xs text-gray-400">No Anthropic API key saved yet.</p>
    {% endif %}
</div>
```

Also remove everything after `</main>` up to `{% endblock %}` (the three `<template>` elements and the `<script>` block), replacing them with nothing so the file ends:

```html
</main>
{% endblock %}
```

**Step 2: Verify the template renders**

Run: `cargo build -p boopmark-server`
Expected: compiles successfully.

**Step 3: Commit**

```bash
git add templates/settings/index.html
git commit -m "feat: simplify settings key UX to delete-or-add model"
```

---

### Task 2: Simplify the backend form handler

**Files:**
- Modify: `server/src/web/pages/settings.rs`

**Step 1: Update the `SettingsForm` struct**

Replace the `anthropic_api_key_action` field with a `delete_anthropic_api_key` field:

```rust
#[derive(Deserialize)]
struct SettingsForm {
    llm_enabled: Option<String>,
    delete_anthropic_api_key: Option<String>,
    anthropic_api_key: Option<String>,
    anthropic_model: Option<String>,
}
```

**Step 2: Simplify the `save_settings` handler**

Replace the complex `api_key_action` match block with straightforward logic:

```rust
async fn save_settings(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Form(form): Form<SettingsForm>,
) -> axum::response::Response {
    let enabled = form.llm_enabled.is_some();
    let delete_key = form.delete_anthropic_api_key.is_some();
    let submitted_api_key = form
        .anthropic_api_key
        .filter(|value| !value.trim().is_empty());

    let (anthropic_api_key, clear_anthropic_api_key) = if delete_key {
        (None, true)
    } else {
        (submitted_api_key, false)
    };

    match state
        .settings
        .save(
            user.id,
            crate::app::settings::SaveLlmSettingsInput {
                enabled,
                anthropic_api_key,
                clear_anthropic_api_key,
                anthropic_model: form.anthropic_model,
            },
        )
        .await
    {
        Ok(_) => Redirect::to("/settings?saved=1").into_response(),
        Err(DomainError::InvalidInput(_)) => axum::http::StatusCode::BAD_REQUEST.into_response(),
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
```

**Step 3: Run existing unit tests to check nothing in the service layer broke**

Run: `cargo test -p boopmark-server`
Expected: all existing `app::settings` unit tests pass (they don't depend on the web handler). The `web::pages::settings` tests should also pass since they only test `build_model_option_views`.

**Step 4: Commit**

```bash
git add server/src/web/pages/settings.rs
git commit -m "feat: simplify settings form handler to delete-or-add"
```

---

### Task 3: Update E2E tests for the simplified UX

**Files:**
- Modify: `tests/e2e/settings.spec.js`

**Step 1: Update `resetSettings` helper**

The "Clear saved key" radio no longer exists. Replace it with the new delete checkbox:

```javascript
async function resetSettings(page) {
  await page.goto("/settings");

  const deleteKey = page.getByTestId("delete-anthropic-api-key");
  if (await deleteKey.count()) {
    await deleteKey.check();
  }

  const enableLlm = page.getByLabel("Enable LLM integration");
  if (await enableLlm.isChecked()) {
    await enableLlm.uncheck();
  }

  await page
    .getByLabel("Anthropic model")
    .selectOption("claude-haiku-4-5-20251001");
  await page.getByRole("button", { name: "Save settings" }).click();
  await expect(page).toHaveURL(/\/settings\?saved=1$/);
}
```

**Step 2: Update the "renders in the app shell" test**

This test should remain mostly the same. It already checks for "No Anthropic API key saved yet." and editable input. No changes needed since `resetSettings` clears the key first.

**Step 3: Replace the "keep replace and clear flows" test with the new simplified flow test**

Replace the test named `"settings page uses explicit keep replace and clear flows for saved Anthropic keys"` with a new test:

```javascript
test("settings page supports add and delete key flows", async ({ page }) => {
  const anthropicApiKey = readAnthropicApiKeyFromDotEnv();

  await signIn(page);
  await resetSettings(page);
  await page.goto("/settings");

  // Add a key: fill the input and save
  await page.getByLabel("Enable LLM integration").check();
  await page.getByLabel("Anthropic API key").fill(anthropicApiKey);
  await page.getByLabel("Anthropic model").selectOption("claude-sonnet-4-6");
  await page.getByRole("button", { name: "Save settings" }).click();

  // Verify key-saved state
  await expect(page).toHaveURL(/\/settings\?saved=1$/);
  await expect(page.getByText("Settings saved")).toBeVisible();
  await expect(page.getByTestId("anthropic-api-key-status")).toBeVisible();
  await expect(page.getByText("Anthropic API key saved securely")).toBeVisible();
  await expect(page.getByLabel("Anthropic API key")).toHaveCount(0);
  await expect(page.getByTestId("delete-anthropic-api-key")).toBeVisible();
  await expect(page.getByLabel("Anthropic model")).toHaveValue("claude-sonnet-4-6");

  // Delete the key
  await page.getByTestId("delete-anthropic-api-key").check();
  await page.getByRole("button", { name: "Save settings" }).click();

  // Verify back to add-key state
  await expect(page).toHaveURL(/\/settings\?saved=1$/);
  await expect(page.getByText("No Anthropic API key saved yet.")).toBeVisible();
  await expect(page.getByLabel("Anthropic API key")).toBeEditable();
  await expect(page.getByTestId("anthropic-api-key-status")).toHaveCount(0);
  await expect(page.getByTestId("delete-anthropic-api-key")).toHaveCount(0);
});
```

**Step 4: Remove or update validation tests that reference the old radio actions**

- Remove the test `"settings rejects replace-key submissions without a replacement value"` — the replace action no longer exists.
- Remove the test `"settings rejects forged key-action combinations"` — `anthropic_api_key_action` no longer exists.
- Keep the test `"settings rejects forged unsupported anthropic model submissions with 400"` as-is (unchanged).
- Keep the test `"unauthenticated requests cannot read or save settings"` as-is (unchanged).
- Keep the test `"legacy api keys route redirects to settings"` as-is (unchanged).

**Step 5: Run the E2E tests**

Run: `cd /Users/chrisfenton/Code/personal/boopmark/.worktrees/simplify-settings-key-ux && npx playwright test tests/e2e/settings.spec.js`
Expected: all tests pass.

**Step 6: Commit**

```bash
git add tests/e2e/settings.spec.js
git commit -m "test: update E2E tests for simplified settings key UX"
```

---

### Task 4: Verify with agent-browser E2E screenshots

**Files:** none (verification only)

**Step 1: Start the local dev stack if not running**

Run: `cd /Users/chrisfenton/Code/personal/boopmark/.worktrees/simplify-settings-key-ux && docker compose up -d`

**Step 2: Take screenshots proving the new UX works**

Using Playwright MCP (agent-browser), navigate to the running E2E server or dev server and capture screenshots of:

1. Settings page with no key saved (shows editable input field, "No Anthropic API key saved yet.")
2. Settings page after saving a key (shows "Anthropic API key saved securely." status banner, "Delete saved key" checkbox, no input field)
3. Settings page after deleting the saved key (returns to editable input field state)
4. Settings page showing model select still works
5. Settings page showing the app shell header/navbar

These screenshots serve as proof that the implementation works correctly.

**Step 3: Run the full Playwright test suite one final time**

Run: `cd /Users/chrisfenton/Code/personal/boopmark/.worktrees/simplify-settings-key-ux && npx playwright test tests/e2e/settings.spec.js`
Expected: all tests pass.

**Step 4: Commit any remaining changes**

If CSS was regenerated or any other files changed, commit them.

```bash
git add -A
git commit -m "chore: final verification of simplified settings key UX"
```
