# Simplify Settings Key UX - Test Plan

## Strategy reconciliation

The implementation plan touches three layers: Askama template, Axum form handler, and Playwright E2E tests. The domain service layer (`SettingsService`, `SaveLlmSettingsInput`) is unchanged. The existing E2E harness (`scripts/e2e/start-server.sh` on port 4010, Playwright config) remains the correct test runner. The `.env` file in the worktree supplies `ANTHROPIC_API_KEY` and `LLM_SETTINGS_ENCRYPTION_KEY` to the E2E server.

The strategy holds without changes:
- **Harness:** Committed Playwright E2E suite (`tests/e2e/settings.spec.js`) against the dedicated E2E server on `http://127.0.0.1:4010`. No new harness needed.
- **Unit tests:** Existing `cargo test -p boopmark-server` covers `build_model_option_views` and `SettingsService` domain logic. No new unit tests needed since the domain layer is unchanged.
- **Agent-browser verification:** Playwright MCP screenshots for visual proof against the dev/E2E server.

No paid/external API dependencies are exercised by the tests themselves (the `ANTHROPIC_API_KEY` in `.env` is only stored/encrypted, never called).

---

## Test plan

### 1. Full add-then-delete key lifecycle

- **Name:** Adding a key shows saved state; deleting it returns to editable add-key state
- **Type:** scenario
- **Harness:** Playwright E2E (`tests/e2e/settings.spec.js`)
- **Preconditions:** Signed-in user with no saved key (achieved via `resetSettings` helper).
- **Actions:**
  1. Navigate to `/settings`.
  2. Check "Enable LLM integration".
  3. Fill the "Anthropic API key" password input with the key from `.env`.
  4. Select model "claude-sonnet-4-6".
  5. Click "Save settings".
  6. Observe the redirected page.
  7. Check the "Delete saved key" checkbox (`data-testid="delete-anthropic-api-key"`).
  8. Click "Save settings".
  9. Observe the redirected page.
- **Expected outcome:**
  - After step 6: URL matches `/settings?saved=1`. "Settings saved" banner visible. `data-testid="anthropic-api-key-status"` visible with text "Anthropic API key saved securely." No password input present (`getByLabel("Anthropic API key")` count = 0). Delete checkbox visible. Model select has value "claude-sonnet-4-6".
  - After step 9: URL matches `/settings?saved=1`. "No Anthropic API key saved yet." visible. Password input is editable. `data-testid="anthropic-api-key-status"` count = 0. `data-testid="delete-anthropic-api-key"` count = 0.
  - Source of truth: user description ("key exists -> user can delete it; key does not exist -> user can add one; deleting returns to editable add-key state").
- **Interactions:** Exercises encrypted persistence round-trip (encrypt on save, presence check on reload), Axum form handler, Askama template conditional rendering.

### 2. Settings page renders in app shell with default model when no key saved

- **Name:** Settings page shows app shell, editable key input, and default model when no key exists
- **Type:** scenario
- **Harness:** Playwright E2E (`tests/e2e/settings.spec.js`)
- **Preconditions:** Signed-in user with no saved key (via `resetSettings`).
- **Actions:**
  1. Navigate to `/settings`.
  2. Inspect the page structure.
- **Expected outcome:**
  - Banner (header/navbar) is visible. "BoopMark" link points to `/bookmarks`.
  - "Settings" and "LLM Integration" headings visible.
  - "Enable LLM integration" checkbox is unchecked.
  - "No Anthropic API key saved yet." text visible.
  - "Anthropic API key" password input is editable.
  - Model select defaults to "claude-haiku-4-5-20251001".
  - Exactly 3 model options: claude-opus-4-6, claude-sonnet-4-6, claude-haiku-4-5-20251001.
  - Source of truth: user description ("Keep the select-based model UI", "Keep the Settings page inside the normal BoopMark app shell/navigation", "key does not exist -> user can add one").
- **Interactions:** Askama template rendering, model options from `ANTHROPIC_MODEL_OPTIONS`.

### 3. Legacy API keys route redirects to settings

- **Name:** /settings/api-keys redirects authenticated users to /settings
- **Type:** regression
- **Harness:** Playwright E2E (`tests/e2e/settings.spec.js`)
- **Preconditions:** Signed-in user.
- **Actions:**
  1. Navigate to `/settings/api-keys`.
- **Expected outcome:**
  - URL resolves to `/settings`. "Settings" and "LLM Integration" headings visible.
  - Source of truth: existing behavior (legacy redirect preserved per implementation plan).
- **Interactions:** Axum routing.

### 4. Unauthenticated access is rejected

- **Name:** Unauthenticated requests to settings endpoints return 401
- **Type:** boundary
- **Harness:** Playwright E2E (`tests/e2e/settings.spec.js`)
- **Preconditions:** No authentication.
- **Actions:**
  1. GET `/settings` via API request (no cookies).
  2. GET `/settings/api-keys` via API request (no cookies, no redirects).
  3. POST `/settings` with form data via API request (no cookies).
  4. Navigate to `/settings` in browser (no sign-in).
  5. Navigate to `/settings/api-keys` in browser (no sign-in).
- **Expected outcome:**
  - Steps 1-3: HTTP 401 status.
  - Step 4: "Settings" heading not visible.
  - Step 5: URL does not match `/settings`.
  - Source of truth: existing behavior (auth requirement is unchanged).
- **Interactions:** Auth middleware (`AuthUser` extractor).

### 5. Forged unsupported model submission returns 400

- **Name:** Submitting an unsupported model identifier via forged POST returns 400
- **Type:** boundary
- **Harness:** Playwright E2E (`tests/e2e/settings.spec.js`)
- **Preconditions:** Signed-in user.
- **Actions:**
  1. Submit a forged POST to `/settings` with `anthropic_model=claude-3-7-sonnet-latest` (not in the official allow list, no existing saved model).
- **Expected outcome:**
  - HTTP 400 status.
  - Source of truth: domain validation in `resolve_model_for_save` rejects unknown models.
- **Interactions:** Axum form handler -> `SettingsService.save` -> `resolve_model_for_save`.

### 6. Old radio-based actions are no longer accepted

- **Name:** Submitting the old `anthropic_api_key_action` field has no effect (field is ignored)
- **Type:** invariant
- **Harness:** Playwright E2E (`tests/e2e/settings.spec.js`) - verified implicitly
- **Preconditions:** Implementation removes `anthropic_api_key_action` from `SettingsForm`.
- **Actions:**
  - This is verified structurally: the `SettingsForm` struct no longer deserializes `anthropic_api_key_action`. Axum's `Form` extractor with `#[derive(Deserialize)]` will silently ignore unknown fields by default in `serde`.
  - The old E2E tests that submitted `anthropic_api_key_action` are removed, so no regression path exists.
- **Expected outcome:**
  - No radio buttons, `<template>` elements, or `<script>` block in the rendered settings page.
  - Verified by tests 1 and 2: the page only shows either the delete checkbox or the add-key input.
  - Source of truth: user description ("Remove the current keep/replace/clear saved-key workflow").
- **Interactions:** Serde deserialization, Askama template.

### 7. Agent-browser visual verification screenshots

- **Name:** Screenshots prove the three key states render correctly in the app shell
- **Type:** scenario
- **Harness:** Playwright MCP (agent-browser) against running server
- **Preconditions:** Server running, signed-in user.
- **Actions:**
  1. Navigate to `/settings` with no key saved. Take screenshot.
  2. Add a key and save. Take screenshot of saved state.
  3. Delete the key. Take screenshot of returned add-key state.
  4. Verify model select is visible in all states.
  5. Verify header/navbar is visible in all states.
- **Expected outcome:**
  - Screenshot 1: editable password input, "No Anthropic API key saved yet.", app shell header visible.
  - Screenshot 2: "Anthropic API key saved securely." status, "Delete saved key" checkbox, no password input, app shell header visible.
  - Screenshot 3: same as screenshot 1.
  - Source of truth: user description (all three states described explicitly).
- **Interactions:** Full stack round-trip.

### 8. Existing unit tests pass unchanged

- **Name:** Domain-layer unit tests remain green (service logic and model options untouched)
- **Type:** regression
- **Harness:** `cargo test -p boopmark-server`
- **Preconditions:** No changes to domain layer.
- **Actions:**
  1. Run `cargo test -p boopmark-server`.
- **Expected outcome:**
  - All tests in `app::settings::tests` pass (9 tests).
  - All tests in `web::pages::settings::tests` pass (2 tests for `build_model_option_views`).
  - All tests in `domain::llm_settings::tests` pass (1 test).
  - Source of truth: existing passing test suite (no domain changes in scope).
- **Interactions:** None (isolated unit tests).

---

## Coverage summary

### Covered

| Area | Tests |
|---|---|
| Add key flow (no key -> save key -> key-saved state) | 1, 2 |
| Delete key flow (key-saved -> delete -> add-key state) | 1 |
| App shell presence (header, navbar, navigation) | 2, 7 |
| Model select UI (options, default, selection persistence) | 1, 2 |
| Old radio/template/script removal | 6 (structural), 1, 2 (implicit) |
| Auth protection | 4 |
| Legacy redirect | 3 |
| Model validation | 5 |
| Domain service layer | 8 |
| Visual proof | 7 |

### Explicitly excluded

| Area | Reason | Risk |
|---|---|---|
| Actual LLM API calls | Out of scope; key is stored/encrypted but never called in these tests | Low - persistence layer is tested, API integration is a separate concern |
| Multi-user isolation | Not changed by this task | Low - auth extractor unchanged |
| Database migration | No schema changes | None |
| CSS/Tailwind regression | Visual verification via screenshots covers layout; pixel-perfect comparison not in scope | Low - functional behavior verified |
