# Fix LLM Bookmark Enrichment — Test Plan

## Strategy reconciliation

The implementation plan adds three missing pieces: (1) `tags` field on `UrlMetadata` / `SuggestFields`, (2) `LlmEnricher` port + `AnthropicEnricher` adapter, (3) wiring in the suggest handler + `get_decrypted_api_key` on `SettingsService`. The testing strategy must cover:

- **Unit tests** for pure logic (prompt building, JSON parsing, API key decryption) — these are cheap and deterministic.
- **Integration/scenario tests** verifying the full suggest flow from the user's perspective. The Anthropic API is a paid external dependency. The committed Playwright E2E suite runs against a real server; for LLM enrichment to work in E2E, a real Anthropic API key must be available (the E2E start script already reads `ANTHROPIC_API_KEY` from `.env`). The implementation plan's Task 6 calls for agent-browser ad-hoc verification with a real key and screenshots, which is the right approach for proving the feature works end-to-end.
- **Regression tests** ensuring the existing suggest flow (scraping-only, no LLM) still works when LLM is disabled or unconfigured.

The plan's architecture matches what the strategy assumes: the `suggest` handler is the integration point, `SuggestFields` is the observation surface in templates, and the `LlmEnricher` trait boundary enables future mock-based testing. No adjustments to scope or cost are needed.

## Harness requirements

### Harness 1: Existing Playwright E2E harness

- **What it does:** Starts a real Boopmark server on port 4010 with E2E auth, Postgres, and local storage. Runs browser-based tests.
- **Exposes:** Page interactions via Playwright selectors (`data-testid` attributes). Network observation via Playwright.
- **Complexity:** Already built. No changes needed to the harness itself.
- **Tests that depend on it:** Tests 1, 2, 7, 8.

### Harness 2: Existing Rust unit test harness

- **What it does:** `cargo test -p boopmark-server` runs in-process unit tests with fake repositories.
- **Exposes:** Direct function calls, struct construction, assertion on return values.
- **Complexity:** Already built. `FakeLlmSettingsRepository` exists in `server/src/app/settings.rs`.
- **Tests that depend on it:** Tests 3, 4, 5, 6, 9, 10, 11.

---

## Test plan

### Test 1 — Scenario: Adding a bookmark with LLM enabled produces enriched title, description, and tags

- **Type:** scenario
- **Harness:** Playwright E2E (agent-browser ad-hoc verification)
- **Preconditions:** User is signed in. LLM integration is enabled in `/settings` with a valid Anthropic API key and model selected (e.g., `claude-sonnet-4-6`). No bookmarks exist.
- **Actions:**
  1. Navigate to `/bookmarks`.
  2. Click "Add Bookmark" button (`data-testid="open-add-bookmark-modal"`).
  3. Fill the URL field with `https://github.com/danshapiro/trycycle`.
  4. Tab out of the URL field to trigger the `blur` → `hx-post="/bookmarks/suggest"` call.
  5. Wait for the suggest fields to populate (title input becomes non-empty).
  6. Take a screenshot.
  7. Submit the bookmark.
  8. Take a screenshot of the resulting bookmark card.
- **Expected outcome:**
  - The title input (`data-testid="bookmark-title-input"`) contains text that is NOT the raw `og:title` "GitHub - danshapiro/trycycle" — it should be an LLM-improved version (e.g., "Trycycle" or similar concise rewrite). **Source of truth:** user's bug report specifying LLM should enhance the title.
  - The description input (`data-testid="bookmark-description-input"`) contains text that is NOT the raw `og:description` "Contribute to danshapiro/trycycle development by creating an account on GitHub." — it should be LLM-enhanced. **Source of truth:** user's bug report.
  - The tags input (`name="tags_input"`) contains comma-separated tag values (not empty, not just placeholder text). **Source of truth:** user's bug report specifying tags should be populated by LLM.
  - The preview image (`data-testid="bookmark-preview-image"`) is visible (scraping still works). **Source of truth:** existing passing E2E behavior.
- **Interactions:** Anthropic Messages API (real call with real key). HTML scraping of github.com. Postgres for user session. HTMX swap replacing `#bookmark-suggest-target`.

### Test 2 — Scenario: Adding a bookmark with LLM disabled still shows scraped metadata and empty tags

- **Type:** regression / scenario
- **Harness:** Playwright E2E (agent-browser ad-hoc verification)
- **Preconditions:** User is signed in. LLM integration is disabled in `/settings` (unchecked) or no API key is saved.
- **Actions:**
  1. Navigate to `/settings`, disable LLM integration (uncheck the checkbox), save.
  2. Navigate to `/bookmarks`.
  3. Click "Add Bookmark", paste `https://github.com/danshapiro/trycycle`, tab out.
  4. Wait for suggest fields to populate.
- **Expected outcome:**
  - Title input contains the raw scraped `og:title` (e.g., "GitHub - danshapiro/trycycle"). **Source of truth:** existing behavior before this change — scraping works independently of LLM.
  - Description input contains the raw scraped `og:description`. **Source of truth:** existing behavior.
  - Tags input is empty (no LLM to suggest tags, scraper does not produce tags). **Source of truth:** implementation plan — scraper sets `tags: None`.
  - Preview image is still visible. **Source of truth:** existing E2E test assertion.
- **Interactions:** HTML scraping. No Anthropic API call should be made.

### Test 3 — Unit: `get_decrypted_api_key` returns key and model when LLM is enabled with a saved key

- **Type:** unit
- **Harness:** Rust unit tests (`cargo test`)
- **Preconditions:** `FakeLlmSettingsRepository` with a saved `LlmSettings` where `enabled = true` and `anthropic_api_key_encrypted` is set (encrypted via `SecretBox`).
- **Actions:** Call `service.get_decrypted_api_key(user_id)`.
- **Expected outcome:** Returns `Ok(Some((decrypted_key, model)))` where `decrypted_key` matches the original plaintext and `model` matches the saved model. **Source of truth:** implementation plan Task 5 — the method must decrypt the stored key and return it alongside the model.
- **Interactions:** `SecretBox::decrypt`, `FakeLlmSettingsRepository::get`.

### Test 4 — Unit: `get_decrypted_api_key` returns `None` when LLM is disabled

- **Type:** unit
- **Harness:** Rust unit tests (`cargo test`)
- **Preconditions:** `FakeLlmSettingsRepository` with `enabled = false` and a saved encrypted key.
- **Actions:** Call `service.get_decrypted_api_key(user_id)`.
- **Expected outcome:** Returns `Ok(None)`. **Source of truth:** implementation plan Task 3 Step 5 — `try_llm_enrich` checks `enabled` before proceeding; `get_decrypted_api_key` must return `None` when disabled.
- **Interactions:** `FakeLlmSettingsRepository::get`.

### Test 5 — Unit: `get_decrypted_api_key` returns `None` when no settings exist for user

- **Type:** unit
- **Harness:** Rust unit tests (`cargo test`)
- **Preconditions:** `FakeLlmSettingsRepository` with no stored settings.
- **Actions:** Call `service.get_decrypted_api_key(user_id)`.
- **Expected outcome:** Returns `Ok(None)`. **Source of truth:** implementation plan — the method should handle the case where the user has never configured LLM settings.
- **Interactions:** `FakeLlmSettingsRepository::get`.

### Test 6 — Unit: `AnthropicEnricher::build_prompt` includes URL and scraped metadata

- **Type:** unit
- **Harness:** Rust unit tests (`cargo test`)
- **Preconditions:** None.
- **Actions:** Call `AnthropicEnricher::build_prompt` with an `EnrichmentInput` containing a URL, scraped title, and scraped description.
- **Expected outcome:** The returned prompt string contains the URL, the title, and the description. **Source of truth:** implementation plan Task 2 Step 3 — the prompt must include these fields for the LLM to work with.
- **Interactions:** None (pure function).

### Test 7 — Integration: Existing Playwright E2E suggest test still passes

- **Type:** regression
- **Harness:** Playwright E2E (`npx playwright test tests/e2e/suggest.spec.js`)
- **Preconditions:** E2E server running (started by Playwright config). No LLM settings configured for the E2E user (first run).
- **Actions:** Run the existing `suggest.spec.js` test.
- **Expected outcome:** The test passes — title and description are filled (from scraping), preview image is visible, bookmark card is created with stored image. **Source of truth:** the committed E2E test is the regression baseline.
- **Interactions:** HTML scraping, Postgres, local storage, HTMX suggest flow. The LLM enrichment should gracefully no-op (user has no LLM settings).

### Test 8 — Integration: Tags field appears in the suggest response and is submitted with the bookmark

- **Type:** integration
- **Harness:** Playwright E2E (agent-browser ad-hoc verification)
- **Preconditions:** User is signed in. LLM integration is enabled with a valid key.
- **Actions:**
  1. Add a bookmark via the modal with LLM enabled (same as Test 1).
  2. After the suggest fields populate, inspect the tags input value.
  3. Submit the form.
  4. Verify the created bookmark card displays the tags.
- **Expected outcome:**
  - Tags input has a non-empty value after suggest returns. **Source of truth:** user's bug report.
  - The bookmark card shows the tags. **Source of truth:** the `CreateForm` struct already reads `tags_input` and passes it to `CreateBookmark`, so if the input has values, they should persist.
- **Interactions:** Full stack: Anthropic API, scraper, Postgres create, HTMX swap.

### Test 9 — Unit: `build_prompt` uses "(none)" for missing metadata

- **Type:** boundary
- **Harness:** Rust unit tests (`cargo test`)
- **Preconditions:** None.
- **Actions:** Call `AnthropicEnricher::build_prompt` with `scraped_title: None` and `scraped_description: None`.
- **Expected outcome:** The prompt contains "(none)" for both title and description. **Source of truth:** implementation plan Task 2 Step 3 — `unwrap_or("(none)")`.
- **Interactions:** None (pure function).

### Test 10 — Unit: Enrichment JSON parsing handles clean response

- **Type:** unit
- **Harness:** Rust unit tests (`cargo test`)
- **Preconditions:** None.
- **Actions:** Parse a JSON string `{"title": "Better Title", "description": "Better desc", "tags": ["rust", "web"]}` into `EnrichmentJson`.
- **Expected outcome:** `title` is `Some("Better Title")`, `tags` has 2 elements. **Source of truth:** implementation plan Task 2 — the response format the LLM is asked to produce.
- **Interactions:** `serde_json::from_str`.

### Test 11 — Unit: Enrichment JSON parsing strips markdown fences

- **Type:** boundary
- **Harness:** Rust unit tests (`cargo test`)
- **Preconditions:** None.
- **Actions:** Parse a response wrapped in `` ```json ... ``` `` fences after stripping.
- **Expected outcome:** Parsing succeeds and returns the correct fields. **Source of truth:** implementation plan Task 2 — LLMs sometimes wrap JSON in markdown fences, the adapter must handle this.
- **Interactions:** String manipulation, `serde_json::from_str`.

### Test 12 — Regression: `UrlMetadata` in scraper sets `tags: None`

- **Type:** regression
- **Harness:** Rust unit tests (`cargo test`)
- **Preconditions:** None.
- **Actions:** The existing `merge_metadata_preserves_user_text_but_returns_missing_image` test must compile and pass after adding `tags: None` to `UrlMetadata`.
- **Expected outcome:** Existing test passes with the new field. **Source of truth:** implementation plan Task 1 Step 3.
- **Interactions:** `UrlMetadata` struct construction.

---

## Coverage summary

### Covered

| Area | Tests |
|------|-------|
| Full user scenario: LLM-enriched bookmark creation | 1, 8 |
| Regression: scraping-only flow when LLM is off | 2, 7 |
| API key decryption gate (enabled/disabled/missing) | 3, 4, 5 |
| Prompt construction (with and without metadata) | 6, 9 |
| Response JSON parsing (clean and fenced) | 10, 11 |
| Struct field additions compile correctly | 12 |
| Tags round-trip through the form to the created bookmark | 8 |

### Explicitly excluded

| Area | Reason | Risk |
|------|--------|------|
| Anthropic API error handling (rate limit, invalid key, timeout) | Would require mocking the HTTP layer or using invalid keys; the adapter returns `DomainError::Internal` and `try_llm_enrich` maps errors to `None` (graceful degradation). Low risk — failure mode is "falls back to scrape-only". | Low: worst case is the user sees raw scraped metadata, which is the current behavior. |
| `AppState` construction / `main.rs` wiring | Verified by the E2E tests (the server must start). No unit-testable logic. | Low: compile-time errors catch most wiring issues. |
| Performance of LLM enrichment | The Anthropic API call adds latency (typically 1-5 seconds). The suggest handler already blocks on scraping. No performance regression beyond the expected LLM call time. | Low: the UI already shows a loading state during suggest. |
| Multiple concurrent users / session isolation | LLM settings are per-user-id. No shared mutable state. | Low: the existing auth/session system handles isolation. |
