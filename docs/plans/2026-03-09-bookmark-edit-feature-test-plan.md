# Bookmark Edit Feature Test Plan

## Strategy reconciliation

The user's testing strategy calls for Playwright MCP / agent-browser E2E testing against the local dev stack, with screenshots proving key behaviors. After reviewing the implementation plan against the codebase:

- **Harness match:** The existing Playwright E2E harness (`scripts/e2e/start-server.sh` on port 4010, `playwright.config.js`) matches what the strategy assumes. The committed spec pattern (`tests/e2e/*.spec.js`) is the established approach. The plan adds standard Axum routes, Askama templates, and HTMX interactions -- all testable with the existing harness.
- **LLM dependency:** The "Suggest Edits" flow requires a configured Anthropic API key. The E2E harness already handles this (see `settings.spec.js` which reads the key from `.env`). Tests requiring LLM will follow the same pattern. Tests that don't need LLM will not configure it.
- **No new external dependencies** beyond what the project already uses.
- **`tags_with_counts` SQL query:** Exercised indirectly through the edit-suggest flow. A unit test for the `build_prompt` function with `existing_tags` is already specified in the implementation plan (Task 2, Step 5).
- **`UpdateBookmark` clearing semantics:** The plan's most subtle behavior -- passing `Some("")` to allow field clearing via `COALESCE` -- needs explicit boundary testing. This is a difference from the create flow (which uses `non_empty` to convert `""` to `None`).

No strategy changes requiring user approval.

## Harness requirements

**Existing Playwright E2E harness** (no new harness needed)
- What it does: Starts a dedicated E2E server on port 4010 with `ENABLE_E2E_AUTH=1`, Postgres, and local storage backend
- What it exposes: Full browser automation via Playwright, E2E auth bypass button, page navigation, DOM assertions
- Estimated complexity: Zero -- already built and operational
- Which tests depend on it: All tests below (1-9)

**Seed bookmark helper** (minor addition to test file)
- What it does: Creates a bookmark via the add-modal flow so edit tests have something to work with
- What it exposes: A `createBookmark(page, url)` helper function
- Estimated complexity: ~10 lines, reusing existing add-modal pattern from `suggest.spec.js`
- Which tests depend on it: Tests 1-7

---

## Test plan

### 1. Clicking Edit on a bookmark card opens a pre-populated edit modal

- **Type:** scenario
- **Harness:** Playwright E2E
- **Preconditions:** User is signed in. At least one bookmark exists with a known title, description, and tags.
- **Actions:**
  1. Sign in via E2E auth.
  2. Create a bookmark with URL `https://example.com`, filling in title "Test Title", description "Test Desc", tags "alpha, beta".
  3. Locate the bookmark card containing "Test Title".
  4. Click the "Edit" button on that card.
- **Expected outcome:**
  - An edit modal appears (element `#edit-modal` is visible).
  - The title input contains "Test Title".
  - The description input contains "Test Desc".
  - The tags input contains "alpha, beta".
  - A "Save" button and "Cancel" button are visible.
  - Source of truth: Implementation plan Task 3 (edit_modal.html template) and Task 5 (edit handler populates from `bookmark.title`, `bookmark.description`, `bookmark.tags.join(", ")`).
- **Interactions:** Exercises `GET /bookmarks/{id}/edit` handler, Askama template rendering, HTMX `hx-get` + `hx-target="body"` + `hx-swap="beforeend"`.

### 2. Editing a bookmark title and saving updates the card in-place

- **Type:** scenario
- **Harness:** Playwright E2E
- **Preconditions:** User is signed in. A bookmark exists with title "Original Title".
- **Actions:**
  1. Click "Edit" on the bookmark card.
  2. Clear the title field and type "Updated Title".
  3. Click "Save".
- **Expected outcome:**
  - The edit modal disappears (`#edit-modal` is no longer in the DOM).
  - The bookmark card that was edited now displays "Updated Title" (not "Original Title").
  - The card remains in the grid (no page reload -- HTMX `outerHTML` swap).
  - Source of truth: Implementation plan Task 5 (`update` handler returns `BookmarkCard` partial, `hx-put` targets `#bookmark-{id}` with `outerHTML` swap), Task 3 (`hx-on::after-request` removes modal on success).
- **Interactions:** Exercises `PUT /bookmarks/{id}`, `UpdateBookmark` domain model, Postgres `COALESCE` update SQL, HTMX swap.

### 3. Cancel closes the edit modal without saving changes

- **Type:** scenario
- **Harness:** Playwright E2E
- **Preconditions:** User is signed in. A bookmark exists with title "Unchanged Title".
- **Actions:**
  1. Click "Edit" on the bookmark card.
  2. Clear the title field and type "Should Not Save".
  3. Click "Cancel" (not Save).
- **Expected outcome:**
  - The edit modal disappears.
  - The bookmark card still displays "Unchanged Title".
  - No network request to `PUT /bookmarks/{id}` was made after opening the modal (can verify by checking the card text didn't change).
  - Source of truth: Implementation plan Task 3 (Cancel button uses `onclick="document.getElementById('edit-modal').remove()"`).
- **Interactions:** Pure client-side DOM removal, no server interaction.

### 4. Editing description and tags saves correctly

- **Type:** scenario
- **Harness:** Playwright E2E
- **Preconditions:** User is signed in. A bookmark exists with known description and tags.
- **Actions:**
  1. Click "Edit" on the bookmark card.
  2. Change the description to "New description text".
  3. Change the tags to "gamma, delta".
  4. Click "Save".
- **Expected outcome:**
  - The modal closes.
  - The bookmark card shows "New description text" in the description area.
  - The bookmark card shows tag badges for "gamma" and "delta".
  - The old tags are no longer displayed on this card.
  - Source of truth: Implementation plan Task 5 (`update` handler processes `form.description` and `form.tags_input`), card.html template renders `bookmark.description` and `bookmark.tags`.
- **Interactions:** Exercises tag parsing (`split(',')`) in the `update` handler, tag rendering in card template.

### 5. Clearing a field via edit saves an empty value (not the old value)

- **Type:** boundary
- **Harness:** Playwright E2E
- **Preconditions:** User is signed in. A bookmark exists with title "Has Title" and description "Has Desc".
- **Actions:**
  1. Click "Edit" on the bookmark card.
  2. Clear the title field (leave it empty).
  3. Clear the description field (leave it empty).
  4. Clear the tags field (leave it empty).
  5. Click "Save".
- **Expected outcome:**
  - The modal closes.
  - The bookmark card no longer shows "Has Title" -- it falls back to showing the URL (per card.html: `{% if let Some(title) = bookmark.title %}{{ title }}{% else %}{{ bookmark.url }}{% endif %}`).
  - The bookmark card no longer shows "Has Desc".
  - No tag badges are displayed.
  - Source of truth: Implementation plan Task 5, Note on clearing semantics: "We do NOT use non_empty() here (unlike the create flow), because non_empty converts "" to None, and COALESCE(NULL, column) keeps the old value -- preventing the user from clearing a field." The handler passes `form.title` directly (Some("")) so COALESCE sees a non-NULL empty string and uses it.
- **Interactions:** Exercises the critical difference between edit and create flows in how empty strings are handled. Tests the SQL COALESCE behavior with empty-string vs NULL.

### 6. Suggest Edits button populates form with LLM suggestions

- **Type:** scenario
- **Harness:** Playwright E2E
- **Preconditions:** User is signed in. Anthropic API key is configured in settings (read from `.env`). A bookmark exists with URL `https://github.com/danshapiro/trycycle`.
- **Actions:**
  1. Configure LLM settings: navigate to `/settings`, enable LLM, fill API key, select haiku model, save.
  2. Navigate back to `/bookmarks`.
  3. Click "Edit" on the bookmark card for `trycycle`.
  4. Verify the "Suggest Edits" button is visible (conditional on `has_llm`).
  5. Click "Suggest Edits".
  6. Wait for the suggest spinner to disappear and new field values to appear.
- **Expected outcome:**
  - The title, description, and tags fields are populated with LLM-generated suggestions (non-empty values).
  - The suggestions replace the previous form values (edit suggest always replaces, unlike add-flow suggest which uses `fill_if_blank`).
  - Source of truth: Implementation plan Task 5 (`edit_suggest` handler) -- "Unlike the add-flow suggest... the edit suggest always replaces fields with LLM/scraped suggestions."
- **Interactions:** Exercises `POST /bookmarks/{id}/suggest`, `tags_with_counts` query, `try_llm_enrich` with `existing_tags`, Anthropic API call, HTMX swap of `#edit-suggest-target`.

### 7. Suggest Edits button is hidden when no LLM key is configured

- **Type:** boundary
- **Harness:** Playwright E2E
- **Preconditions:** User is signed in. No Anthropic API key is configured (or it has been deleted via settings).
- **Actions:**
  1. Ensure LLM is not configured (reset settings, delete key if present).
  2. Navigate to `/bookmarks`.
  3. Create a bookmark if none exist.
  4. Click "Edit" on a bookmark card.
- **Expected outcome:**
  - The edit modal opens.
  - The "Suggest Edits" button is NOT visible in the modal.
  - Source of truth: Implementation plan Task 3 (edit_modal.html): `{% if has_llm %}` conditional around the Suggest Edits button. Task 5 (`edit` handler): `has_llm` is set by checking `get_decrypted_api_key`.
- **Interactions:** Exercises the `has_llm` conditional in both the handler and the template.

### 8. Close button (x) in modal header removes the modal

- **Type:** boundary
- **Harness:** Playwright E2E
- **Preconditions:** User is signed in. A bookmark exists.
- **Actions:**
  1. Click "Edit" on a bookmark card.
  2. Click the "x" close button in the modal header (not Cancel).
- **Expected outcome:**
  - The edit modal is removed from the DOM.
  - The bookmark card is unchanged.
  - Source of truth: Implementation plan Task 3 (edit_modal.html): close button uses `onclick="document.getElementById('edit-modal').remove()"`.
- **Interactions:** Pure client-side.

### 9. Edit button is visible alongside Delete button on every bookmark card

- **Type:** invariant
- **Harness:** Playwright E2E
- **Preconditions:** User is signed in with at least 2 bookmarks.
- **Actions:**
  1. Navigate to `/bookmarks`.
  2. For each visible bookmark card, check for the presence of both "Edit" and "Delete" buttons.
- **Expected outcome:**
  - Every bookmark card has both an "Edit" button and a "Delete" button visible.
  - The Edit button triggers `hx-get` to `/bookmarks/{id}/edit`.
  - Source of truth: Implementation plan Task 4 (card.html modification adds Edit button next to Delete).
- **Interactions:** Template rendering for all cards.

### 10. `build_prompt` includes existing tags when present

- **Type:** unit
- **Harness:** `cargo test` (already specified in implementation plan Task 2, Step 5)
- **Preconditions:** None (pure function test).
- **Actions:**
  1. Construct an `EnrichmentInput` with `existing_tags: Some(vec![("rust", 5), ("web", 3)])`.
  2. Call `AnthropicEnricher::build_prompt(&input)`.
- **Expected outcome:**
  - The prompt string contains "rust (5)" and "web (3)".
  - The prompt string contains "Prefer reusing these existing tags".
  - Source of truth: Implementation plan Task 2, Step 2 (the `build_prompt` replacement code).
- **Interactions:** None (pure function).

### 11. `build_prompt` omits existing tags instruction when field is None or empty

- **Type:** unit
- **Harness:** `cargo test`
- **Preconditions:** None.
- **Actions:**
  1. Construct an `EnrichmentInput` with `existing_tags: None`.
  2. Call `build_prompt`.
  3. Construct another with `existing_tags: Some(vec![])`.
  4. Call `build_prompt`.
- **Expected outcome:**
  - Neither prompt contains "Prefer reusing these existing tags".
  - Both prompts still contain the URL and standard instructions.
  - Source of truth: Implementation plan Task 2, Step 2 (`match &input.existing_tags` with `Some(tags) if !tags.is_empty()` guard).
- **Interactions:** None (pure function).

---

## Coverage summary

### Covered

- **Edit button visibility** on bookmark cards (tests 1, 9)
- **Edit modal lifecycle**: open with pre-populated values (1), close via Cancel (3), close via X (8), close on successful save (2)
- **Saving edits**: title (2), description (4), tags (4), all fields at once
- **Field clearing semantics**: the critical COALESCE/empty-string behavior (5)
- **LLM Suggest Edits flow**: full round-trip with API key (6), hidden when no key (7)
- **LLM prompt construction**: existing tags included (10), omitted when absent (11)
- **In-place card update**: HTMX outerHTML swap without page reload (2, 4)

### Explicitly excluded (per strategy)

- **`tags_with_counts` SQL query correctness in isolation**: Tested indirectly through the Suggest Edits E2E flow (test 6). A dedicated integration test against Postgres would require a test database fixture setup not currently in the harness. Risk: low -- the SQL is a simple `unnest/GROUP BY/ORDER BY` query and is exercised end-to-end.
- **Concurrent edit conflicts**: The app has no optimistic locking. If two tabs edit the same bookmark, last-write-wins via the existing `UPDATE ... WHERE id = $1 AND user_id = $2`. Not a new risk introduced by this feature.
- **Mobile/responsive layout testing**: Not called for in the strategy.
- **Performance benchmarks**: Low risk -- edit operations are single-row updates. No performance-sensitive code paths introduced.

### Risks from exclusions

- The `tags_with_counts` query is only exercised as part of the LLM suggest flow (test 6), which depends on having an Anthropic API key available. If the key is missing in CI, that query path goes untested. Mitigation: the E2E harness already loads the key from `.env` (see `start-server.sh`).
