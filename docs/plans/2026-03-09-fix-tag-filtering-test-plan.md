# Fix Tag Filtering - Test Plan

## Harness requirements

### Harness 1: Playwright MCP / agent-browser (manual)

- **What it does:** Drives a real browser against the running dev server (`http://localhost:4000`) to verify user-visible behavior end-to-end.
- **What it exposes:** Screenshot capture, DOM inspection, URL bar observation, click/type simulation.
- **Estimated complexity:** Zero build effort -- uses existing Playwright MCP tooling against the existing dev stack (`docker compose up`).
- **Tests that depend on it:** Tests 1-5 (all scenario and regression tests).

### Harness 2: `cargo test` / `cargo build`

- **What it does:** Compiles the workspace and runs the existing unit/integration test suite to verify no regressions.
- **What it exposes:** Build success/failure, test pass/fail.
- **Estimated complexity:** Zero -- already exists.
- **Tests that depend on it:** Test 6.

---

## Test plan

### Test 1: Clicking a tag filter shows only bookmarks with that tag

- **Name:** Tag filter click returns only matching bookmarks
- **Type:** scenario
- **Harness:** Playwright MCP / agent-browser
- **Preconditions:** User is logged in. Homepage (`/bookmarks`) displays multiple bookmarks, some tagged "development" and some not. The tag filter bar shows a "development" button.
- **Actions:**
  1. Navigate to `/bookmarks`.
  2. Observe the unfiltered bookmark list and note which bookmarks have the "development" tag.
  3. Click the "development" tag filter button.
- **Expected outcome:**
  - The URL updates to include `?tags=development` (with `hx-push-url`).
  - Only bookmarks tagged "development" are displayed.
  - Bookmarks without the "development" tag are not displayed.
  - Source of truth: User's bug report states "clicking a tag should filter bookmarks to only those with that tag." The HTMX template sends `hx-get="/bookmarks?tags=development"` which should produce a filtered list via `BookmarkFilter.tags`.
- **Interactions:** HTMX partial rendering (the response replaces `#bookmark-grid` only), Postgres `tags && $N` array overlap query.

### Test 2: Tag filter works on full-page load (direct URL navigation)

- **Name:** Navigating directly to a tag-filtered URL shows correct results
- **Type:** scenario
- **Harness:** Playwright MCP / agent-browser
- **Preconditions:** User is logged in. Bookmarks exist with various tags including "development".
- **Actions:**
  1. Navigate directly to `/bookmarks?tags=development&sort=newest` (full-page load, not HTMX partial).
- **Expected outcome:**
  - Only bookmarks tagged "development" are displayed.
  - The tag filter bar shows ALL available tags (not just tags from the filtered results).
  - The "development" tag button appears active/highlighted (blue background per the template's conditional class).
  - Source of truth: Implementation plan states the second bug is that `collect_all_tags` derives tags from filtered results, so on full-page loads tags disappear. The fix uses `all_tags` query. This test verifies the fix works.
- **Interactions:** Full-page Askama template rendering (GridPage), the new `all_tags` Postgres query (`SELECT DISTINCT unnest(tags)`).

### Test 3: Tag filter with empty search input does not break results

- **Name:** Tag filter combined with empty search field returns matching bookmarks
- **Type:** regression
- **Harness:** Playwright MCP / agent-browser
- **Preconditions:** User is logged in. Bookmarks exist with "development" tag. The search input field is empty.
- **Actions:**
  1. Navigate to `/bookmarks`.
  2. Ensure the search input is empty (do not type anything).
  3. Click the "development" tag filter button.
- **Expected outcome:**
  - Bookmarks tagged "development" are displayed (not "No bookmarks found").
  - The URL may include `search=` (empty) due to `hx-include`; the server must treat this as no search.
  - Source of truth: User's bug report states the URL becomes `?tags=development&search=&sort=newest` and returns empty results. The implementation plan identifies that `search: Some("")` causes `plainto_tsquery('english', '')` to match zero rows. After the fix, empty search strings are normalized to `None`.
- **Interactions:** HTMX `hx-include="[name='search'],[name='sort']"` on tag buttons includes the empty search input. The handler's `query.search.and_then(|s| ...)` normalization is the critical code path.

### Test 4: Search by keyword returns matching bookmarks

- **Name:** Searching by keyword returns correct results
- **Type:** scenario
- **Harness:** Playwright MCP / agent-browser
- **Preconditions:** User is logged in. At least one bookmark contains "trycycle" in its title, description, or URL.
- **Actions:**
  1. Navigate to `/bookmarks`.
  2. Type "trycycle" into the search bar.
  3. Wait for HTMX debounce (300ms delay per `hx-trigger="keyup changed delay:300ms"`).
- **Expected outcome:**
  - The bookmark grid updates to show only bookmarks matching "trycycle".
  - At least one result is visible.
  - Source of truth: User confirmed search works ("Searching 'trycycle' via the search bar ... correctly returns matching bookmarks"). This test ensures the fix does not regress search.
- **Interactions:** HTMX partial rendering, Postgres full-text search via `plainto_tsquery('english', $N)`.

### Test 5: Search combined with tag filter narrows results correctly

- **Name:** Applying both search and tag filter returns only bookmarks matching both
- **Type:** integration
- **Harness:** Playwright MCP / agent-browser
- **Preconditions:** User is logged in. Bookmarks exist with various tags and searchable content.
- **Actions:**
  1. Navigate to `/bookmarks`.
  2. Click a tag filter button (e.g., "development").
  3. Type a search term in the search bar that matches some but not all "development" bookmarks (or use a term that matches no "development" bookmarks to verify empty results are legitimate).
- **Expected outcome:**
  - Results are the intersection of the tag filter and the search query.
  - If no bookmarks match both criteria, "No bookmarks found" is a correct result.
  - Source of truth: The `BookmarkFilter` struct applies both `search` and `tags` as AND conditions in the SQL query (`AND to_tsvector(...) @@ plainto_tsquery(...)` plus `AND tags && $N`). Both filters should compose correctly.
- **Interactions:** Both HTMX `hx-include` mechanisms (tag button includes search+sort, search input includes tags+sort), both Postgres filter clauses applied simultaneously.

### Test 6: Build and existing tests pass without regressions

- **Name:** Cargo build and test suite pass
- **Type:** regression
- **Harness:** `cargo build` / `cargo test`
- **Preconditions:** All implementation tasks (1-3) are complete.
- **Actions:**
  1. Run `cargo build -p boopmark-server`.
  2. Run `cargo test`.
- **Expected outcome:**
  - Build succeeds with no errors.
  - All existing tests pass.
  - No warnings about unused functions (specifically `collect_all_tags` should be removed).
  - Source of truth: The implementation plan specifies build verification after each task. The existing test suite in `server/src/app/bookmarks.rs` (unit tests for `needs_metadata` and `merge_metadata`) must continue to pass.
- **Interactions:** The new `all_tags` method on the `BookmarkRepository` trait means any other `impl BookmarkRepository` would fail to compile if it lacks the method. Currently only `PostgresPool` implements the trait, so this is safe.

---

## Coverage summary

### Covered

| Area | Tests |
|------|-------|
| Tag filter click (HTMX partial path) | 1, 3 |
| Tag filter on full-page load (Askama GridPage path) | 2 |
| Empty search normalization (the primary bug) | 3 |
| All-tags query for filter bar (the secondary bug) | 2 |
| Search functionality (non-regression) | 4 |
| Combined search + tag filter | 5 |
| Build and existing test suite | 6 |

### Explicitly excluded (per agreed strategy)

| Area | Reason | Risk |
|------|--------|------|
| Automated E2E in CI (committed Playwright spec) | The agreed strategy is manual browser testing via Playwright MCP with screenshots. The existing `suggest.spec.js` covers the add-bookmark flow, not filtering. | Low -- tag filtering is verifiable manually and the fix is small (two code changes). If filtering breaks again in the future, a committed E2E spec would catch it, but that was not in scope. |
| Sort order verification | Sort is orthogonal to the tag/search fix and was not reported broken. | Low -- sort logic is unchanged by this fix. |
| Pagination / limit-offset behavior | The default limit of 50 is unchanged and the user did not report pagination issues. | Low -- no pagination code is modified. |
| Multi-tag filtering (e.g., `?tags=development,rust`) | The template generates single-tag URLs (`?tags=development`). Multi-tag filtering is not exposed in the UI. | Low -- the code path supports it via comma splitting, but it is not user-reachable. |
| Performance | The new `SELECT DISTINCT unnest(tags)` query runs only on full-page loads, not on every HTMX partial. The bookmark table is small for a personal app. | Negligible. |
