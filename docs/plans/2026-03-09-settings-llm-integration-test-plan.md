# Settings + LLM Integration — Test Plan

## Strategy reconciliation

The agreed testing strategy still mostly holds: the primary confidence for this feature should come from the existing Playwright end-to-end harness, with a small amount of Rust unit coverage for nontrivial normalization and secret-handling logic. Reconciliation against the implementation plan adds two concrete boundaries that need explicit coverage:

1. The E2E harness must copy and read the worktree `.env`, fail fast when `ANTHROPIC_API_KEY` is missing, and still leave the signed-in user's settings empty until they save them explicitly.
2. The implementation plan introduces encrypted-at-rest persistence, so the test plan needs one focused service-level secret-handling test in addition to the browser workflow coverage.

One implementation-plan detail does not match the approved user goal: the plan names `claude-3-5-haiku-latest` as the default model, but the user asked for the latest Haiku 4.5 default and Anthropic's official model docs currently identify the Haiku 4.5 alias as `claude-haiku-4-5` on March 9, 2026. This test plan therefore treats `claude-haiku-4-5` as the required default model value, and treats the dated identifier `claude-haiku-4-5-20251001` as a valid explicit override. This does not expand scope beyond what the user already approved, so no additional approval is required here.

## Harness requirements

### Harness 1: Existing Playwright E2E app harness

- **What it does**: Starts PostgreSQL plus the app through `scripts/e2e/start-server.sh`, enables the built-in E2E sign-in path, and drives a real browser against `http://127.0.0.1:4010`.
- **What it exposes**: Authenticated browser navigation, profile-menu interactions, direct route visits, form submission, page reloads, DOM assertions, URL assertions, and Playwright request-level status assertions. The settings spec can also read the copied worktree `.env` through Node's filesystem APIs.
- **Estimated complexity**: Already exists. This task only needs one new settings spec, one renamed profile-menu selector/update, and one direct-request assertion for unauthenticated access.
- **Tests that depend on it**: Tests 1, 2, 3, 4, 5, 6, 7, 8.

### Harness 2: Existing Rust unit-test harness

- **What it does**: Runs focused `cargo test -p boopmark-server ...` coverage against pure helper logic and service behavior without a browser.
- **What it exposes**: Model normalization, keep/replace/clear key semantics, encryption helper behavior, and repository-call assertions via a fake repository.
- **Estimated complexity**: Already exists. This task only needs small tests colocated with the new settings and secret-handling modules.
- **Tests that depend on it**: Tests 9, 10, 11, 12.

## Test plan

1. **Name**: The profile menu opens `Settings`, not `API Keys`, and the first visit shows empty LLM settings
   **Type**: scenario
   **Harness**: Existing Playwright E2E app harness
   **Preconditions**: The app is running through `scripts/e2e/start-server.sh`; the copied worktree `.env` contains `ANTHROPIC_API_KEY`; the tester signs in through `Sign in for E2E` and lands on `/bookmarks`; no `user_llm_settings` row exists yet for this test user.
   **Actions**:
   1. Open the profile menu from the bookmarks header.
   2. Activate the `Settings` menu item.
   3. Observe the destination page and the initial form state.
   **Expected outcome**:
   - The menu item is labeled `Settings`, not `API Keys`.
   - Navigation lands on `/settings`.
   - A page heading `Settings` and a section heading `LLM Integration` are visible.
   - `Enable LLM integration` is unchecked.
   - The `Anthropic API key` field is empty.
   - The page shows `No Anthropic API key saved yet.`
   - The `Anthropic model` control shows `claude-haiku-4-5`.
   - **Source of truth**: The user request requires the page to be called `Settings`, include an `LLM Integration` section, default to disabled, require the user to enter their own Anthropic key in settings, and default the model to the latest Haiku 4.5 version. The implementation plan, Task 4, defines `/settings` as the page route and the settings form as the primary UI surface.
   **Interactions**: Header template markup, profile-menu selector wiring, Axum route merge, `AuthUser`-protected GET `/settings`, Askama settings template rendering, default-value resolution.

2. **Name**: Enabling LLM integration saves the user's Anthropic key and explicit model choice without echoing the secret back into the form
   **Type**: scenario
   **Harness**: Existing Playwright E2E app harness
   **Preconditions**: The same authenticated state as Test 1; the copied worktree `.env` contains a non-empty `ANTHROPIC_API_KEY`; the user has no saved LLM settings before starting the test.
   **Actions**:
   1. Visit `/settings`.
   2. Check `Enable LLM integration`.
   3. Fill `Anthropic API key` with the value read from the copied worktree `.env`.
   4. Change `Anthropic model` from the default alias to `claude-haiku-4-5-20251001`.
   5. Submit `Save settings`.
   6. Reload the page.
   **Expected outcome**:
   - Saving redirects to `/settings?saved=1`.
   - A success message is visible after save.
   - The page shows `Anthropic API key saved`.
   - The `Anthropic API key` field is blank after save and remains blank after reload.
   - `Enable LLM integration` remains checked after reload.
   - `Anthropic model` persists as `claude-haiku-4-5-20251001` after reload.
   - **Source of truth**: The user request requires enable/disable, Anthropic key entry, and Anthropic model choice. The implementation plan, Task 3 and Task 4, defines save/replace behavior, success redirect, and a settings page that exposes only saved-key presence rather than replaying the stored secret.
   **Interactions**: Browser form submission, POST `/settings`, settings service normalization and save path, secret encryption helper, repository upsert behavior, redirect handling, Askama reload rendering.

3. **Name**: Disabling LLM integration does not silently delete a previously saved Anthropic key
   **Type**: scenario
   **Harness**: Existing Playwright E2E app harness
   **Preconditions**: The user already has saved settings with `enabled=true`, a stored Anthropic key, and a non-default saved model from Test 2 or equivalent setup.
   **Actions**:
   1. Visit `/settings`.
   2. Uncheck `Enable LLM integration`.
   3. Leave `Anthropic API key` blank and do not check `Clear saved Anthropic API key`.
   4. Submit `Save settings`.
   5. Reload the page.
   **Expected outcome**:
   - The save succeeds and returns to `/settings?saved=1`.
   - `Enable LLM integration` is unchecked after reload.
   - The saved-key indicator is still present after reload.
   - The `Anthropic model` value remains unchanged after reload.
   - **Source of truth**: The user request requires enable/disable as a separate control. The implementation plan, Task 2 and Task 3, explicitly defines keep/replace/clear semantics so blank submission keeps the saved key unless the user uses the clear control.
   **Interactions**: Settings form serialization, POST `/settings`, `resolve_api_key_change` keep-existing path, repository upsert logic, page reload rendering.

4. **Name**: The explicit clear control removes the saved Anthropic key and returns the page to the empty-key state
   **Type**: scenario
   **Harness**: Existing Playwright E2E app harness
   **Preconditions**: The user already has a saved Anthropic key in `user_llm_settings`.
   **Actions**:
   1. Visit `/settings`.
   2. Check `Clear saved Anthropic API key`.
   3. Submit `Save settings`.
   4. Reload the page.
   **Expected outcome**:
   - The save succeeds and returns to `/settings?saved=1`.
   - The page no longer shows `Anthropic API key saved`.
   - The page shows `No Anthropic API key saved yet.`
   - The `Anthropic API key` field is blank.
   - The clear checkbox is not pre-checked after reload.
   - **Source of truth**: The implementation plan, Task 1 Step 4 and Task 3, explicitly defines a clear-key flow and a user-visible empty-key state distinct from save/replace.
   **Interactions**: Form checkbox serialization, POST `/settings`, clear-key branch in the settings service, repository upsert behavior, Askama conditional rendering.

5. **Name**: The legacy `/settings/api-keys` URL redirects authenticated users to the renamed `/settings` page
   **Type**: boundary
   **Harness**: Existing Playwright E2E app harness
   **Preconditions**: The user is signed in through the E2E flow.
   **Actions**:
   1. Navigate directly to `/settings/api-keys`.
   2. Observe the final URL and rendered page content.
   **Expected outcome**:
   - The browser is redirected to `/settings`.
   - The destination renders the `Settings` page with the `LLM Integration` section.
   - **Source of truth**: The user request renames the surface from `API Keys` to `Settings`. The implementation plan, Task 4 Step 1, explicitly adds a redirect from the old `/settings/api-keys` path.
   **Interactions**: Legacy route handler, redirect response, authenticated route protection, settings-page rendering.

6. **Name**: The settings E2E harness refuses to use a fake Anthropic key when the copied worktree `.env` is missing it
   **Type**: boundary
   **Harness**: Existing Playwright E2E app harness
   **Preconditions**: A dedicated negative-path run uses a worktree `.env` with `ANTHROPIC_API_KEY` removed or blank; no application code is modified for the test beyond the fixture setup.
   **Actions**:
   1. Invoke the settings spec path that reads `ANTHROPIC_API_KEY` from the copied worktree `.env`.
   2. Observe the failure before any settings save step uses a fabricated key.
   **Expected outcome**:
   - The settings test fails immediately with a deterministic error that `ANTHROPIC_API_KEY` must exist in the copied worktree `.env`.
   - The test does not fall back to a placeholder or fake key.
   - **Source of truth**: The user request says to check `ANTHROPIC_API_KEY` in `.env` only for agent-browser and Playwright E2E testing and still require the user to enter the key themselves. The implementation plan, Task 1 Step 4, explicitly requires a fail-fast `.env` read with no fake-key fallback.
   **Interactions**: Node filesystem access in the Playwright spec, copied worktree `.env`, E2E bootstrap discipline.

7. **Name**: Unauthenticated requests cannot read or save settings
   **Type**: integration
   **Harness**: Existing Playwright E2E app harness
   **Preconditions**: The app is running through the standard E2E harness; the test has not signed in and holds no session cookie.
   **Actions**:
   1. Issue an unauthenticated `GET /settings`.
   2. Issue an unauthenticated `POST /settings` with representative form data.
   3. Observe the HTTP responses.
   **Expected outcome**:
   - Both requests are rejected with HTTP 401.
   - No settings page content is rendered for the unauthenticated request.
   - **Source of truth**: The agreed testing strategy requires unauthorized access to remain rejected by `AuthUser`. The current codebase's `AuthUser` extractor returns `401 UNAUTHORIZED` when no valid API key or session cookie is present, and the implementation plan keeps both settings handlers behind `AuthUser`.
   **Interactions**: `AuthUser` extractor, settings GET route, settings POST route, request parsing without a browser session.

8. **Name**: The existing authenticated bookmark suggest flow still works after the settings route, menu label, and shared shell changes
   **Type**: regression
   **Harness**: Existing Playwright E2E app harness
   **Preconditions**: The app is running through the standard E2E harness; the existing suggest flow dependencies remain reachable.
   **Actions**:
   1. Run `tests/e2e/profile-menu.spec.js`, `tests/e2e/settings.spec.js`, and `tests/e2e/suggest.spec.js` together.
   2. Execute the existing bookmark suggest flow from sign-in through bookmark creation.
   **Expected outcome**:
   - The settings and profile-menu suites pass together with the existing suggest suite.
   - The add-bookmark modal still autofills metadata on blur and renders the stored preview image after submit.
   - **Source of truth**: The implementation plan, Task 5 Step 2, explicitly requires this targeted regression suite to pass together because the shared authenticated shell and header are being modified.
   **Interactions**: Shared header rendering, profile-menu changes, bookmarks page shell, suggest HTMX flow, bookmark creation and rendering.

9. **Name**: Blank or missing model input falls back to the current Haiku 4.5 default while explicit model values are preserved
   **Type**: unit
   **Harness**: Existing Rust unit-test harness
   **Preconditions**: The new settings helper module exposes model-normalization logic.
   **Actions**:
   1. Call the normalization helper with `None`.
   2. Call it with whitespace-only input.
   3. Call it with `claude-haiku-4-5-20251001`.
   **Expected outcome**:
   - `None` normalizes to `claude-haiku-4-5`.
   - Whitespace-only input normalizes to `claude-haiku-4-5`.
   - A nonblank explicit identifier is returned unchanged.
   - **Source of truth**: The user request requires the latest Haiku 4.5 model by default. The implementation plan's service layer introduces explicit model normalization for blank input and preserves nonblank user-entered model values.
   **Interactions**: Settings helper logic only.

10. **Name**: API key change resolution cleanly separates keep-existing, replace, and clear behaviors
    **Type**: unit
    **Harness**: Existing Rust unit-test harness
    **Preconditions**: The new settings helper module exposes API-key change resolution logic.
    **Actions**:
    1. Resolve a blank key submission with `clear=false`.
    2. Resolve a nonblank key submission with `clear=false`.
    3. Resolve `clear=true` with no replacement key.
    **Expected outcome**:
    - Blank input produces the keep-existing branch.
    - Nonblank input produces the replace branch.
    - The explicit clear control produces the clear branch.
    - **Source of truth**: The implementation plan, Task 2 Step 4 and Task 3 Step 1-2, defines keep/replace/clear semantics as the core behavior of the settings service.
    **Interactions**: Settings helper logic only.

11. **Name**: Secret encryption round-trips correctly and the ciphertext blob is not the plaintext key
    **Type**: unit
    **Harness**: Existing Rust unit-test harness
    **Preconditions**: The new `SecretBox` helper accepts a valid application encryption key and exposes encrypt/decrypt operations.
    **Actions**:
    1. Encrypt a representative Anthropic key string.
    2. Compare the resulting blob to the plaintext bytes.
    3. Decrypt the blob.
    **Expected outcome**:
    - Encryption succeeds with the configured application key.
    - The encrypted blob differs from the plaintext key bytes.
    - Decryption returns the original plaintext.
    - **Source of truth**: The user explicitly chose encrypted persistence at rest. The implementation plan, Task 2 Step 2, introduces `SecretBox` as the mechanism that must encrypt before persistence and decrypt for internal use.
    **Interactions**: Application encryption helper only.

12. **Name**: Saving a replacement key encrypts it before persistence and exposes only `has_anthropic_api_key` back to the page model
    **Type**: integration
    **Harness**: Existing Rust unit-test harness
    **Preconditions**: A fake `LlmSettingsRepository` can capture the bytes passed into `upsert`; the settings service is instantiated with that fake repository and a real `SecretBox`.
    **Actions**:
    1. Call `SettingsService::save` with `enabled=true`, a nonblank Anthropic key, `clear=false`, and an explicit model.
    2. Inspect the bytes passed to the fake repository.
    3. Inspect the returned `SettingsView`.
    **Expected outcome**:
    - The repository receives replacement bytes, not the raw plaintext key string.
    - Decrypting those bytes through the same `SecretBox` yields the original key.
    - The returned `SettingsView` reports `has_anthropic_api_key=true`.
    - The returned `SettingsView` never contains the raw Anthropic key.
    - **Source of truth**: The user explicitly chose encrypted persistence. The implementation plan, Task 2 and Task 3, keeps the page limited to save/replace/clear behavior and defines `SettingsView` as the page-facing representation of whether a key exists, not the key itself.
    **Interactions**: Settings service, secret encryption helper, repository contract.

## Coverage summary

- **Covered areas**: Settings entry from the shared profile menu; first-load defaults for disabled state, empty saved-key state, and the current Haiku 4.5 default model; authenticated save, reload, disable, and clear flows; worktree `.env` propagation for E2E without auto-seeding saved settings; legacy route redirect; unauthenticated rejection; shared-shell regression coverage; model normalization; keep/replace/clear branching; encryption helper behavior; encrypted repository writes.
- **Explicitly excluded per agreed strategy**: Broad database-integration tests for every repository branch, screenshot-based visual comparison, performance benchmarking beyond normal Playwright and test-command timeouts, and actual outbound Anthropic API calls.
- **Risk carried by exclusions**: A SQL statement bug that only appears against PostgreSQL could evade the unit-level service tests until the browser suite reaches it; purely visual layout regressions would rely on DOM assertions rather than screenshot diffing; no test in this plan validates live Anthropic credentials against Anthropic's API because the feature scope stops at settings capture and persistence.
