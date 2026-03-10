# Tag Filter Active State & Toggle — Test Plan

## Strategy reconciliation

The agreed testing strategy is: manual browser testing via Playwright MCP with screenshots, plus `cargo test` and `cargo build`. After reviewing the implementation plan, the strategy holds without changes:

- The plan involves template refactoring (Tasks 1-3), a Rust handler change (Task 4), a Tailwind rebuild (Task 5), and browser verification (Task 6). All changes are in templates and a single Rust handler — no new external dependencies, no new APIs, no infrastructure changes.
- `cargo build` verifies Askama template compilation (templates are compiled into Rust at build time, so a successful build proves templates parse and variable bindings resolve).
- `cargo test` verifies existing tests still pass (regression).
- Playwright MCP browser screenshots verify the user-facing behavior: active chip styling, toggle-off, and sort preserving the tag filter.
- No additional harnesses are needed. The Playwright MCP connects to the running local dev server at `http://localhost:4000`.

## Test plan

### 1. Full user journey: filter by tag, verify active state, toggle off

- **Name:** Clicking a tag chip filters bookmarks and shows active styling; clicking again clears the filter
- **Type:** scenario
- **Harness:** Playwright MCP against local dev server (`http://localhost:4000/bookmarks`)
- **Preconditions:** User is logged in. At least two bookmarks exist with different tags (e.g., one tagged "ai", one tagged "rust"). The bookmarks page is loaded (full-page, not HTMX).
- **Actions:**
  1. Navigate to `http://localhost:4000/bookmarks`.
  2. Take a screenshot. Observe all tag chips in the filter bar.
  3. Click the tag chip for "ai" (or whichever tag exists).
  4. Wait for HTMX response to complete.
  5. Take a screenshot.
  6. Click the same tag chip again (the one that is now active).
  7. Wait for HTMX response to complete.
  8. Take a screenshot.
- **Expected outcome:**
  - Screenshot 1 (step 2): All tag chips have inactive styling — `bg-[#1a1d2e]`, `border-gray-700`, `text-gray-400`. No chip has `bg-blue-600`. Source of truth: implementation plan Task 1 template markup and user description ("no visual indication" is the bug being fixed).
  - Screenshot 2 (step 5): The clicked tag chip has active styling — `bg-blue-600`, `border-blue-500`, `text-white`. Other chips remain inactive. Only bookmarks with the selected tag are displayed. The filter bar is present (OOB swap worked). Source of truth: implementation plan Task 1 (active class branch), Task 3 (OOB swap), Task 4 (handler returns filter_tags in HTMX response).
  - Screenshot 3 (step 8): All tag chips return to inactive styling. All bookmarks are displayed (filter cleared). Source of truth: implementation plan Task 1 toggle-off logic (`hx-get` sends empty `tags=` when `tag.active` is true), user description ("clicking again doesn't clear filter" is the bug being fixed).
- **Interactions:** HTMX OOB swap mechanism (filter bar replacement); Axum handler querying tags from storage backend.

### 2. Sort dropdown preserves active tag filter

- **Name:** Changing sort order while a tag filter is active preserves the tag filter
- **Type:** scenario
- **Harness:** Playwright MCP against local dev server
- **Preconditions:** User is logged in. Multiple bookmarks exist with at least one shared tag. The bookmarks page is loaded.
- **Actions:**
  1. Navigate to `http://localhost:4000/bookmarks`.
  2. Click a tag chip to activate a filter.
  3. Wait for HTMX response.
  4. Change the sort dropdown to a different value (e.g., "Oldest First").
  5. Wait for HTMX response.
  6. Take a screenshot.
- **Expected outcome:**
  - After changing sort order, the tag chip remains visually active (blue styling). The bookmarks shown are still filtered to the selected tag, now in the new sort order. Source of truth: implementation plan Task 1 — the hidden `<input type="hidden" name="tags">` carries the active tag value, and the sort dropdown's `hx-include="[name='tags']"` picks it up.
- **Interactions:** Sort dropdown's `hx-include` finding the hidden `tags` input; HTMX OOB swap updating the filter bar with the tag still active.

### 3. Rust code compiles with new template struct and refactored handler

- **Name:** cargo build succeeds after adding BookmarkListWithFilters and refactoring the list handler
- **Type:** integration
- **Harness:** `cargo build` in the worktree
- **Preconditions:** All code changes from Tasks 1-4 are applied.
- **Actions:**
  1. Run `cargo build` in the worktree root.
- **Expected outcome:**
  - Build succeeds with exit code 0. This validates: (a) the new `BookmarkListWithFilters` struct's template path resolves, (b) the template variables (`bookmarks`, `filter_tags`, `sort`) match the struct fields, (c) `filters_inner.html` include resolves from both `filters.html` and `filters_oob.html`, (d) `list_with_filters.html` includes resolve. Source of truth: Askama compile-time template checking.
- **Interactions:** Askama template compilation; Rust type checking of struct fields against template variables.

### 4. Existing tests still pass (regression)

- **Name:** cargo test passes — no regressions from handler refactoring
- **Type:** regression
- **Harness:** `cargo test` in the worktree
- **Preconditions:** All code changes from Tasks 1-4 are applied. Build succeeds (Test 3).
- **Actions:**
  1. Run `cargo test` in the worktree root.
- **Expected outcome:**
  - All tests pass with exit code 0. The refactoring (moving the `all_tags` query before the `is_htmx` branch) should not break any existing behavior. Source of truth: existing test suite as characterization tests.
- **Interactions:** Any integration tests that exercise the bookmarks list endpoint.

### 5. Filter bar is present after HTMX navigation (OOB swap works)

- **Name:** After clicking a tag chip, the filter bar is replaced via OOB swap and remains functional
- **Type:** integration
- **Harness:** Playwright MCP against local dev server
- **Preconditions:** User is logged in. Bookmarks page is loaded.
- **Actions:**
  1. Navigate to `http://localhost:4000/bookmarks`.
  2. Click a tag chip.
  3. Wait for HTMX response.
  4. Inspect the DOM: verify `#filter-bar` element exists and contains tag chip buttons.
  5. Click a different tag chip from the (OOB-swapped) filter bar.
  6. Wait for HTMX response.
  7. Take a screenshot.
- **Expected outcome:**
  - After step 3, the `#filter-bar` element is present in the DOM (the OOB swap replaced it). After step 6, the newly clicked tag is active (blue) and the previously active tag is inactive. This proves the OOB-swapped filter bar is fully functional, not just visually present. Source of truth: implementation plan Tasks 2-3 (OOB template), HTMX OOB swap specification.
- **Interactions:** HTMX OOB swap; two sequential HTMX navigations.

### 6. Full-page load with tag query parameter shows active state

- **Name:** Navigating directly to a URL with a tag query parameter shows the correct active chip
- **Type:** boundary
- **Harness:** Playwright MCP against local dev server
- **Preconditions:** User is logged in. Bookmarks with the tag "ai" exist.
- **Actions:**
  1. Navigate directly to `http://localhost:4000/bookmarks?tags=ai` (full-page load, not HTMX).
  2. Take a screenshot.
- **Expected outcome:**
  - The "ai" tag chip has active (blue) styling. Other chips are inactive. Only bookmarks tagged "ai" are shown. Source of truth: the full-page path in the existing handler already passes `filter_tags` with `active` computed from `active_tags.contains(&name)` — this test verifies the existing full-page path still works correctly after the refactor.
- **Interactions:** Full-page render path (GridPage template).

### 7. Empty tags parameter shows all bookmarks with no active chip

- **Name:** Navigating with an empty tags parameter shows all bookmarks, no chip active
- **Type:** boundary
- **Harness:** Playwright MCP against local dev server
- **Preconditions:** User is logged in. Bookmarks exist.
- **Actions:**
  1. Navigate to `http://localhost:4000/bookmarks?tags=`.
  2. Take a screenshot.
- **Expected outcome:**
  - All tag chips are inactive (no blue styling). All bookmarks are displayed. The empty `tags=` parameter is treated the same as no `tags` parameter. Source of truth: Rust handler code — `query.tags.as_deref().filter(|t| !t.is_empty())` filters out empty strings, producing an empty `active_tags` vec.
- **Interactions:** Query parameter parsing in the list handler.

## Coverage summary

### Covered

- **Active state visual indication:** Tests 1, 5, 6 verify the blue styling appears on the correct chip after clicking, OOB swap, and direct URL navigation.
- **Toggle-off behavior:** Test 1 verifies clicking an active chip clears the filter.
- **Sort + tag filter interaction:** Test 2 verifies the hidden input mechanism preserves tags when changing sort order.
- **OOB swap functionality:** Tests 1, 2, 5 verify the filter bar is correctly replaced after HTMX responses.
- **Full-page vs. HTMX paths:** Test 6 covers full-page loads; Tests 1, 2, 5 cover HTMX partial responses.
- **Build and regression:** Tests 3, 4 verify compilation and existing test suite passage.
- **Boundary: empty filter:** Test 7 verifies empty tags parameter handling.

### Explicitly excluded (per agreed strategy)

- **Automated E2E tests (committed Playwright specs):** The strategy calls for manual Playwright MCP verification, not new committed test files. Risk: no automated regression protection for this specific feature. Mitigated by the existing test suite (Test 4) and the fact that the feature is template-level with compile-time checking.
- **Unit tests for tag toggle URL generation:** The toggle logic is in the Askama template (`{% if tag.active %}{% else %}{{ tag.name }}{% endif %}`), which cannot be unit-tested in isolation. The scenario tests cover this behavior end-to-end.
- **Multi-tag filter combinations:** The current implementation supports only single-tag filtering (the `tags` query param takes a single value per chip click). Multi-tag is out of scope for this task.
- **Performance testing:** The only change to the HTMX response path is adding one additional lightweight query (`all_tags`). The risk is negligible — this query was already running on full-page loads.
