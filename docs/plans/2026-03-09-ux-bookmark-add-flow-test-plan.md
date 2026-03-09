# Test Plan: UX Improvements for Bookmark Add Flow and Homepage

**Date:** 2026-03-09
**Implementation plan:** `docs/plans/2026-03-09-ux-bookmark-add-flow.md`

---

## Testing strategy

Two testing layers cover this work:

1. **Cargo test** — backend regression. No Rust handler logic is changing, but Askama template compilation is checked at build time. `cargo test` confirms nothing is broken.
2. **Agent-browser E2E with screenshots** — the primary verification method. Playwright MCP against the running dev server proves all four UX changes visually. Screenshots are the deliverables.
3. **Committed Playwright regression** (`tests/e2e/suggest.spec.js`) — updated assertions keep the automated E2E suite green after the template changes.

No new Rust unit tests are needed because all four changes are template/frontend-only. The existing `suggest.spec.js` already exercises the suggest-then-submit-then-verify-card flow and will be updated to cover the changed behavior.

---

## Pre-implementation gate

| # | Check | How |
|---|-------|-----|
| P1 | `.env` copied into worktree | `ls -la /Users/chrisfenton/Code/personal/boopmark/.worktrees/ux-bookmark-add-flow/.env` confirms presence |

---

## Automated test updates (suggest.spec.js)

These are the concrete changes to `tests/e2e/suggest.spec.js` required by the implementation plan (Task 6).

| # | Test assertion | What it covers | Current state | Action |
|---|----------------|----------------|---------------|--------|
| T1 | Remove `expect(page.getByTestId("bookmark-preview-image")).toBeVisible()` | Task 4: preview image removed from modal | Currently asserts image is visible | **Remove** this assertion |
| T2 | After submit + modal hidden, reopen modal and assert `urlInput` has value `""` | Task 3: modal fields cleared after save | Not tested | **Add** |
| T3 | After submit + modal hidden, reopen modal and assert `titleInput` has value `""` | Task 3: modal fields cleared after save | Not tested | **Add** |
| T4 | After submit + modal hidden, reopen modal and assert `descriptionInput` has value `""` | Task 3: modal fields cleared after save | Not tested | **Add** |
| T5 | `firstCard.getByTestId("bookmark-card-image-link")` exists and `toHaveAttribute("href", /github\.com/)` | Task 2: card image is wrapped in a link | Not tested | **Add** |
| T6 | Existing `firstCard.getByTestId("bookmark-card-image").toHaveAttribute("src", /\/uploads\/images\//)` | Card image still shows on homepage | Currently passes | **Keep** unchanged |
| T7 | Existing `firstCard.locator("text=...").toHaveCount(0)` | Placeholder icon not shown when image exists | Currently passes | **Keep** unchanged |

### Expected updated test flow (single test)

```
1. Sign in via E2E auth
2. Open Add Bookmark modal
3. Fill URL input with https://github.com/danshapiro/trycycle
4. Tab out (triggers suggest)
5. Wait for title input to be non-empty (suggest response arrived)
6. Assert bookmark-preview-image is NOT visible (Task 4 — removed from modal)
7. Click submit
8. Assert modal is hidden
9. Reopen modal
10. Assert URL input is empty (Task 3)
11. Assert title input is empty (Task 3)
12. Assert description input is empty (Task 3)
13. Close modal
14. Assert first card has bookmark-card-image-link with href containing github.com (Task 2)
15. Assert first card has bookmark-card-image with src matching /uploads/images/ (existing)
16. Assert placeholder icon count is 0 (existing)
```

---

## Agent-browser E2E verification (screenshots)

These are the manual Playwright MCP verifications the implementing agent must perform. Each produces a screenshot as evidence.

| # | Scenario | Steps | Expected result | Screenshot label |
|---|----------|-------|-----------------|-----------------|
| S1 | Loading spinner during suggest | Navigate to `/bookmarks`, sign in, open Add Bookmark modal, paste `https://github.com/danshapiro/trycycle`, tab out, **immediately** take screenshot | Spinner visible inside `#bookmark-suggest-target` with "Fetching metadata..." text | Screenshot A |
| S2 | Populated fields, no preview image | Wait for suggest response to complete, take screenshot of the modal | Title, description, tags fields populated; **no** `bookmark-preview-image` or `bookmark-preview-panel` visible | Screenshot B |
| S3 | Modal cleared after save | Click "Add Bookmark" submit button, wait for modal to hide, reopen modal via `open-add-bookmark-modal` button, take screenshot | All fields (URL, title, description, tags) are empty | Screenshot C |
| S4 | Card image is clickable link | On the `/bookmarks` grid, hover over the first bookmark card's image area, take screenshot or inspect DOM | Image is wrapped in `<a>` tag with `href` pointing to the bookmark URL, `data-testid="bookmark-card-image-link"` present | Screenshot D |

### Spinner screenshot timing note (S1)

The spinner is only visible while the HTMX request is in flight. The suggest endpoint involves network scraping, so there is a real delay (typically 1-3 seconds). The screenshot must be taken **immediately** after tabbing out, before the response arrives. If the network is too fast, the implementing agent may need to:
- Take the screenshot within a `page.waitForSelector('#suggest-spinner:not(.htmx-request)')` race, or
- Use `page.route()` to delay the `/bookmarks/suggest` response to ensure the spinner is visible long enough to capture.

---

## Cargo test verification

| # | Command | Expected result |
|---|---------|-----------------|
| C1 | `cargo build` in worktree | Compiles without errors (validates Askama template changes) |
| C2 | `cargo test` in worktree | All existing tests pass |

---

## Tailwind CSS rebuild verification

| # | Check | How |
|---|-------|-----|
| W1 | `output.css` contains `animate-spin` | `grep animate-spin static/css/output.css` returns matches |
| W2 | `output.css` contains `text-blue-500` | `grep text-blue-500 static/css/output.css` returns matches |

---

## Committed Playwright E2E run

| # | Command | Expected result |
|---|---------|-----------------|
| E1 | `npx playwright test tests/e2e/suggest.spec.js` | Passes with the updated assertions from T1-T7 |

---

## Acceptance criteria summary

All four UX changes are verified if and only if:

1. **Spinner (Task 1):** Screenshot A shows spinner in flight; automated test does not regress.
2. **Clickable image (Task 2):** Screenshot D shows anchor wrapping image; T5 assertion passes in suggest.spec.js.
3. **Modal clear (Task 3):** Screenshot C shows empty fields; T2-T4 assertions pass in suggest.spec.js.
4. **No preview image in modal (Task 4):** Screenshot B shows no image; T1 removal prevents false failure in suggest.spec.js.
5. **Cargo test (regression):** C1 and C2 pass.
6. **Playwright E2E (regression):** E1 passes.
