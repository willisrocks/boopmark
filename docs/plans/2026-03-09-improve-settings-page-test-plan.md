# Settings Page UX Test Plan

## Harness requirements

### Harness 1: Existing Playwright E2E app harness

- What it does: Starts the app with the worktree `.env`, signs in through the built-in E2E path, and drives a real browser against the authenticated settings and bookmarks routes.
- What it exposes: Browser navigation, DOM assertions, form submission, page reloads, profile-menu interactions, and authenticated in-page `fetch` calls for HTTP-contract checks.
- Estimated complexity to build: Already exists. This task only extends `tests/e2e/settings.spec.js`; no new harness code is required.
- Tests that depend on it: 1, 2, 3, 4, 5, 6.

### Harness 2: Existing Rust service/unit-test harness

- What it does: Exercises settings-domain constants and `SettingsService` behavior with fake repositories and a real `SecretBox`, without needing a browser.
- What it exposes: Model-option metadata, load/save view shaping, invalid-input errors, preservation of stored legacy values, encryption before persistence, and repository-call assertions.
- Estimated complexity to build: Already exists. This task adds focused tests in `server/src/domain/llm_settings.rs` and `server/src/app/settings.rs`.
- Tests that depend on it: 7, 8, 9, 10, 11, 12.

## Strategy reconciliation

The earlier testing strategy still holds at a high level: browser scenarios should drive confidence, with service-level tests covering the nontrivial save-state machine and encrypted persistence. The revised implementation plan changes the strategy in these ways without increasing scope or requiring user approval:

- The browser contract must now cover the shared app shell on `/settings`, not just the settings card.
- The default-model source of truth is now the official full identifier `claude-haiku-4-5-20251001`, with only three official UI options for new selections.
- The service-layer tests must explicitly cover preserved legacy/custom model values on load, explicit re-submit, omitted save, blank save, and forged unsupported submission.

## Test plan

1. **Name**: Settings renders inside the normal BoopMark shell with the official default Anthropic model
   **Type**: scenario
   **Harness**: Existing Playwright E2E app harness
   **Preconditions**: The app is running from the worktree; the worktree `.env` exists and contains `ANTHROPIC_API_KEY`; the user signs in through `Sign in for E2E`; `resetSettings(page)` has left the account on the normal official-model path with no saved key and no custom stored model.
   **Actions**:
   1. Navigate to `/settings`.
   2. Observe the shell, navigation, and initial LLM form state.
   **Expected outcome**:
   - A visible `banner` is rendered on the page.
   - The `BoopMark` brand link points to `/bookmarks`.
   - The page shows `Settings` and `LLM Integration`.
   - `Enable LLM integration` is unchecked.
   - The page shows `No Anthropic API key saved yet.`
   - The editable `Anthropic API key` field is visible.
   - The `Anthropic model` control is a `select` whose value is `claude-haiku-4-5-20251001`.
   - The select exposes exactly these values on the official-only path: `claude-opus-4-6`, `claude-sonnet-4-6`, `claude-haiku-4-5-20251001`.
   - **Source of truth**: User requirements for settings-shell/navigation and model UX; implementation plan architecture and Task 1 Step 2.
   **Interactions**: Shared header template, settings page template, authenticated GET `/settings`, default model resolution, browser reset helper.

2. **Name**: Saving a new Anthropic configuration shows a truthful saved-key state instead of an empty editable password field
   **Type**: scenario
   **Harness**: Existing Playwright E2E app harness
   **Preconditions**: The same authenticated state as Test 1; no saved Anthropic key is present before the test starts; the test reads a real key from the copied worktree `.env`.
   **Actions**:
   1. Visit `/settings`.
   2. Check `Enable LLM integration`.
   3. Fill `Anthropic API key` with the `.env` value.
   4. Select `claude-sonnet-4-6`.
   5. Submit `Save settings`.
   6. Observe the redirected page.
   **Expected outcome**:
   - Saving redirects to `/settings?saved=1`.
   - A success message is visible.
   - The page shows a saved-state indicator such as `Anthropic API key saved securely`.
   - The original editable `Anthropic API key` field is no longer present by default.
   - The default selected key-action control is `Keep current saved key`.
   - The selected model persists as `claude-sonnet-4-6`.
   - **Source of truth**: User requirements for saved-key UX and model UX; implementation plan Task 1 Step 2 and Task 3.
   **Interactions**: Browser form submission, POST `/settings`, settings service save path, encrypted persistence path, redirect and reload rendering.

3. **Name**: Replace and clear are explicit mutually exclusive saved-key actions
   **Type**: scenario
   **Harness**: Existing Playwright E2E app harness
   **Preconditions**: The user already has a saved Anthropic key and an official saved model from Test 2 or equivalent setup.
   **Actions**:
   1. Visit `/settings`.
   2. Select `Replace saved key`.
   3. Observe the replacement-key input area.
   4. Fill the replacement key input.
   5. Select `Clear saved key`.
   6. Observe the resulting state before save.
   7. Submit `Save settings`.
   **Expected outcome**:
   - Choosing `Replace saved key` reveals the replacement-key input and keeps the original secret hidden.
   - Choosing `Clear saved key` hides the replacement-key input.
   - The page shows explicit text that saving will remove the stored Anthropic API key.
   - After save, the page shows `No Anthropic API key saved yet.`
   - The editable `Anthropic API key` field is available again.
   - **Source of truth**: User requirements for disabling direct editing until explicit clear/replace and making clear removal obvious; implementation plan Task 1 Step 2 and Task 3 save-intent architecture.
   **Interactions**: Saved-key radio/intent controls, form serialization, POST `/settings`, API-key keep/replace/clear resolution, template conditional rendering.

4. **Name**: A fresh official-model settings path exposes only the current official Anthropic options with user-friendly labels
   **Type**: scenario
   **Harness**: Existing Playwright E2E app harness
   **Preconditions**: `resetSettings(page)` has already saved the official default path and removed any preserved legacy value from the account state.
   **Actions**:
   1. Visit `/settings`.
   2. Inspect the `Anthropic model` select and its option labels and values.
   **Expected outcome**:
   - The option labeled `Claude Opus 4.6` has value `claude-opus-4-6`.
   - The option labeled `Claude Sonnet 4.6` has value `claude-sonnet-4-6`.
   - The option labeled `Claude Haiku 4.5` has value `claude-haiku-4-5-20251001`.
   - No additional option is present on this normal official-only path.
   - **Source of truth**: User requirements for replacing free text with a select using current official Anthropic identifiers; implementation plan architecture and Task 1 Step 2.
   **Interactions**: Server-owned allow-list metadata, Askama option rendering, reset helper behavior.

5. **Name**: Forged unsupported model submissions are rejected with `400 Bad Request`
   **Type**: integration
   **Harness**: Existing Playwright E2E app harness
   **Preconditions**: The user is signed in and has a valid authenticated browser session.
   **Actions**:
   1. Issue an authenticated in-page `fetch("/settings", { method: "POST" ... })` with `anthropic_model=claude-3-7-sonnet-latest`.
   2. Read the returned HTTP status.
   **Expected outcome**:
   - The response status is `400`.
   - **Source of truth**: User requirement to save only full official model identifiers; implementation plan Task 1 Step 2 and Task 2 Step 1/3 requiring unsupported forged submissions to fail with `400 Bad Request`.
   **Interactions**: Browser request layer, Axum form handler, service validation, `DomainError::InvalidInput` mapping.

6. **Name**: Shared-shell changes do not break bookmark-page navigation and profile-menu behavior
   **Type**: regression
   **Harness**: Existing Playwright E2E app harness
   **Preconditions**: The app is running through the normal E2E harness.
   **Actions**:
   1. Run the settings spec together with `tests/e2e/profile-menu.spec.js` and `tests/e2e/suggest.spec.js`.
   2. Exercise the existing bookmarks-page profile-menu and bookmark-suggest flows.
   **Expected outcome**:
   - The profile menu still stays visible across pointer and keyboard transitions.
   - The `Settings` menu item still navigates to `/settings`.
   - The add-bookmark modal still autofills metadata on blur and renders the stored preview image after submit.
   - **Source of truth**: User requirement to preserve existing app-shell/navigation patterns; implementation plan Task 3 shared-header reuse and Task 5 verification suite.
   **Interactions**: Shared header component, bookmarks page templates, profile-menu interactions, suggest flow, authenticated bookmarks shell.

7. **Name**: The shared model metadata exposes the official allow-list and full default identifier
   **Type**: unit
   **Harness**: Existing Rust service/unit-test harness
   **Preconditions**: The domain module exports `DEFAULT_ANTHROPIC_MODEL` and `ANTHROPIC_MODEL_OPTIONS`.
   **Actions**:
   1. Read the default-model constant.
   2. Read the model-option collection.
   **Expected outcome**:
   - `DEFAULT_ANTHROPIC_MODEL` is `claude-haiku-4-5-20251001`.
   - The allow-list contains exactly three official options.
   - The allow-list values are `claude-opus-4-6`, `claude-sonnet-4-6`, and `claude-haiku-4-5-20251001`.
   - **Source of truth**: User requirement to verify and use the latest official Anthropic model identifiers; implementation plan architecture and Task 2 Step 1/3.
   **Interactions**: Domain metadata only.

8. **Name**: Loading settings preserves a stored legacy/custom model value for display instead of normalizing it away
   **Type**: integration
   **Harness**: Existing Rust service/unit-test harness
   **Preconditions**: The fake repository contains an existing `LlmSettings` row with an encrypted key present and `anthropic_model="claude-3-7-sonnet-latest"`.
   **Actions**:
   1. Call `SettingsService::load(user_id)`.
   2. Inspect the returned `SettingsView`.
   **Expected outcome**:
   - `SettingsView.has_anthropic_api_key` is `true`.
   - `SettingsView.anthropic_model` is `claude-3-7-sonnet-latest`.
   - No plaintext API key is exposed in the view.
   - **Source of truth**: User requirement to keep encrypted persistence intact; implementation plan architecture requiring legacy/custom saved models to render alongside official choices until explicitly changed.
   **Interactions**: Service load path, fake repository, page-facing view shaping.

9. **Name**: Re-submitting the currently stored legacy/custom model preserves it on save
   **Type**: integration
   **Harness**: Existing Rust service/unit-test harness
   **Preconditions**: The fake repository contains an existing row with `anthropic_model="claude-3-7-sonnet-latest"`.
   **Actions**:
   1. Call `SettingsService::save` with `anthropic_model=Some("claude-3-7-sonnet-latest")`, no replacement key, and no clear action.
   2. Inspect the returned view and captured upsert arguments.
   **Expected outcome**:
   - The save succeeds.
   - The persisted model remains `claude-3-7-sonnet-latest`.
   - **Source of truth**: Implementation plan architecture and Task 2 Step 1/3 requiring the already stored legacy value to round-trip unchanged on unrelated saves.
   **Interactions**: Service save path, allow-list validation exception for the current stored value, repository upsert contract.

10. **Name**: Omitting the model field on an unrelated save preserves the existing stored model instead of silently defaulting it
    **Type**: regression
    **Harness**: Existing Rust service/unit-test harness
    **Preconditions**: The fake repository contains an existing row with `anthropic_model="claude-3-7-sonnet-latest"` and a saved encrypted key.
    **Actions**:
    1. Call `SettingsService::save` with `anthropic_model=None`, changing another setting such as `enabled` or clearing the key.
    2. Inspect the returned view and persisted model argument.
    **Expected outcome**:
    - The save succeeds.
    - The persisted model remains `claude-3-7-sonnet-latest`.
    - **Source of truth**: User requirement to keep persistence intact; revised implementation-plan architecture and Task 2 Step 1/3 explicitly preserving the existing stored model when the form omits `anthropic_model`.
    **Interactions**: Save-intent resolution, repository upsert contract, unrelated-settings mutation path.

11. **Name**: Submitting a blank model field on an unrelated save also preserves the existing stored model
    **Type**: boundary
    **Harness**: Existing Rust service/unit-test harness
    **Preconditions**: The fake repository contains an existing row with `anthropic_model="claude-3-7-sonnet-latest"`.
    **Actions**:
    1. Call `SettingsService::save` with `anthropic_model=Some("   ")`, no explicit model change, and another unrelated settings change.
    2. Inspect the returned view and persisted model argument.
    **Expected outcome**:
    - The save succeeds.
    - The persisted model remains `claude-3-7-sonnet-latest`.
    - **Source of truth**: Revised implementation-plan architecture and Task 2 Step 1/3 separating load/display normalization from save-time intent resolution.
    **Interactions**: Save-time trim/blank handling, repository upsert contract.

12. **Name**: Saving a replacement key encrypts it before persistence and rejects newly submitted unsupported model values
    **Type**: integration
    **Harness**: Existing Rust service/unit-test harness
    **Preconditions**: A fake repository can capture persisted bytes; the service uses a real `SecretBox`; no existing row is present for the invalid-model subcase unless noted.
    **Actions**:
    1. Call `SettingsService::save` with a nonblank replacement Anthropic key and an official model.
    2. Inspect the captured bytes and returned `SettingsView`.
    3. Separately call `SettingsService::save` with `anthropic_model=Some("claude-3-7-sonnet-latest")` when that value is not already the stored current model.
    **Expected outcome**:
    - The repository receives encrypted replacement bytes, not the plaintext key string.
    - Decrypting those bytes through the same `SecretBox` yields the original key.
    - The returned view reports only `has_anthropic_api_key=true`, not the secret itself.
    - The forged unsupported model save fails with `DomainError::InvalidInput`.
    - **Source of truth**: User requirement to keep encrypted settings persistence intact and save only supported official model values for new submissions; implementation plan Task 2 Step 1/3.
    **Interactions**: SecretBox encryption, service save path, allow-list validation, fake repository capture.

## Coverage summary

- Covered action space: `/settings` load inside the shared shell; navigation back to `/bookmarks`; first-load empty-key state; saved-key keep/replace/clear UX; official model-option rendering; authenticated invalid-model POST rejection; preserved legacy/custom model display and save semantics; encrypted persistence; profile-menu and bookmarks-shell regression coverage.
- Explicit exclusions: Live Anthropic API calls, screenshot diffing, exhaustive SQL repository tests beyond the service/repository contract exercised by the existing harnesses, and performance benchmarking beyond normal test-command runtime expectations.
- Risks carried by exclusions: A purely visual spacing regression could escape DOM-based assertions; a PostgreSQL-only edge in SQL upsert behavior could first appear in browser verification rather than unit tests; outbound Anthropic compatibility remains dependent on the separately verified official model docs rather than a live API call.
