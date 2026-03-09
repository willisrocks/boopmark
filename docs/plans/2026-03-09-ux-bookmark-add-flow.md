# UX Improvements for Bookmark Add Flow and Homepage

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Four UX improvements: (1) loading spinner during suggest requests, (2) clickable bookmark images on homepage, (3) clear the add modal after saving, (4) remove preview image from the add modal.

**Architecture:** All four changes are frontend-only (templates + HTMX attributes). No Rust handler logic changes are needed. The suggest response template stops including the og:image preview. The card template wraps images in an anchor. HTMX event handlers on the form reset it after successful submission. An HTMX indicator provides spinner feedback during suggest requests.

**Tech Stack:** Askama templates, HTMX 2 (htmx:beforeRequest/afterRequest events, hx-indicator, hx-on), Tailwind CSS.

**Prerequisite:** Copy `.env` from main repo into the worktree (already confirmed present).

---

### Task 1: Add loading spinner to suggest fields area

**Goal:** Show a visible spinner/loading animation inside `#bookmark-suggest-target` while the HTMX suggest request is in flight.

**Files:**
- Modify: `templates/bookmarks/add_modal.html` — add `hx-indicator` attribute to the URL input pointing at a spinner element
- Modify: `templates/bookmarks/add_modal_suggest_fields.html` — add a spinner element inside `#bookmark-suggest-target` that HTMX shows/hides via the `htmx-request` class
- Modify: `templates/base.html` — add CSS for the htmx-indicator pattern

**Step 1: Add spinner element to `add_modal_suggest_fields.html`**

Add a spinner div as the first child inside the `#bookmark-suggest-target` div. It uses the HTMX indicator CSS pattern (hidden by default, shown during requests):

```html
<div id="bookmark-suggest-target">
    <div id="suggest-spinner" class="htmx-indicator flex items-center justify-center py-8">
        <svg class="animate-spin h-8 w-8 text-blue-500" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
            <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
            <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z"></path>
        </svg>
        <span class="ml-2 text-gray-400 text-sm">Fetching metadata...</span>
    </div>
    <div class="space-y-4">
        ...existing fields...
    </div>
</div>
```

**Step 2: Add `hx-indicator` to the URL input in `add_modal.html`**

On the `<input id="bookmark-url-input">` element, add the attribute `hx-indicator="#suggest-spinner"` so HTMX targets the spinner during the suggest request.

**Step 3: Add CSS for htmx-indicator pattern in `base.html`**

Add a `<style>` block in `templates/base.html` inside `<head>`, after the CSS link:

```html
<style>
    .htmx-indicator { display: none; }
    .htmx-indicator.htmx-request { display: flex; }
</style>
```

**Important:** Since `hx-swap="outerHTML"` replaces `#bookmark-suggest-target` entirely when the response comes back, the spinner is naturally replaced by the response content. The spinner shows while the request is in flight and vanishes when the response HTML replaces the entire div.

**Verification:** Paste a URL, tab out. The spinner should appear immediately and vanish when the suggest response renders.

---

### Task 2: Make bookmark card images clickable links on the homepage

**Goal:** Wrap the bookmark's preview image (and the placeholder icon) in an anchor tag linking to `bookmark.url`.

**Files:**
- Modify: `templates/bookmarks/card.html` — wrap the image div in an `<a>` tag

**Step 1: Wrap the image section in a link**

In `card.html`, wrap both the image branch and the placeholder branch in an anchor tag:

```html
<a href="{{ bookmark.url }}" target="_blank" rel="noopener" class="block" data-testid="bookmark-card-image-link">
    {% if let Some(img) = bookmark.image_url %}
    <div class="h-40 bg-[#151827] flex items-center justify-center overflow-hidden">
        <img src="{{ img }}" alt="" class="w-full h-full object-cover" loading="lazy" data-testid="bookmark-card-image">
    </div>
    {% else %}
    <div class="h-40 bg-[#151827] flex items-center justify-center">
        <span class="text-4xl text-gray-600">&#128278;</span>
    </div>
    {% endif %}
</a>
```

**Verification:** On the homepage, hovering over a bookmark image should show it's a link; clicking navigates to the bookmark URL.

---

### Task 3: Clear the Add Bookmark modal form after successful save

**Goal:** After a bookmark is created, reset all form fields and the suggest-target content so the modal is fresh next time.

**Files:**
- Modify: `templates/bookmarks/add_modal.html` — update the `hx-on::after-request` handler to also clear form fields

**Step 1: Update the after-request handler**

Currently the form has:
```
hx-on::after-request="if(event.detail.successful && event.detail.elt === this) document.getElementById('add-modal').classList.add('hidden')"
```

Change this to also clear each input field by setting its value to empty string:

```
hx-on::after-request="if(event.detail.successful && event.detail.elt === this) { document.getElementById('bookmark-url-input').value = ''; document.getElementById('bookmark-title-input').value = ''; document.getElementById('bookmark-description-input').value = ''; var ti = document.querySelector('[name=tags_input]'); if(ti) ti.value = ''; document.getElementById('add-modal').classList.add('hidden'); }"
```

This explicitly clears each field. The URL input is outside the suggest target so it persists across HTMX swaps. The title/description/tags inputs are inside `#bookmark-suggest-target` and may have been replaced by HTMX with pre-filled `value` attributes, so `form.reset()` alone would not clear them (it resets to the current DOM `value` attribute defaults). Explicitly setting `.value = ''` is the correct approach.

**Verification:** Add a bookmark, close the modal, reopen it — all fields should be empty.

---

### Task 4: Remove preview image from the Add Bookmark modal suggest fields

**Goal:** Stop showing the og:image preview inside the add modal's suggest fields. The image should only appear on the homepage bookmark cards.

**Files:**
- Modify: `templates/bookmarks/add_modal_suggest_fields.html` — remove the entire preview image panel

**Step 1: Remove preview panel from suggest fields template**

In `templates/bookmarks/add_modal_suggest_fields.html`, remove the entire preview panel block (the `<div>` with `data-testid="bookmark-preview-panel"` and everything inside it, including the `{% if let Some(img) %}` image and the `{% else %}` placeholder).

The template should go directly from `<div class="space-y-4">` to the Title label.

**Step 2: Keep `suggest_preview_image_url` in Rust structs**

The `SuggestFields` struct and `GridPage` struct both have `suggest_preview_image_url`. Since Askama does not error on unused struct fields, the Rust code can remain as-is. This avoids unnecessary Rust changes and keeps the image URL available in the handler in case it's needed in the future.

**Verification:** Paste a URL, tab out — the suggest response should show title/description/tags fields but no image preview.

---

### Task 5: Rebuild Tailwind CSS

**Goal:** Recompile `static/css/output.css` so that newly-introduced utility classes (`animate-spin`, `text-blue-500`, `py-8`, `ml-2`) are included in the production stylesheet. Tailwind uses JIT/purge and only emits classes found in template source files at build time. Without this step the spinner will have no animation, no color, no padding, and no margin on its label.

**Files:**
- Regenerated: `static/css/output.css`

**Step 1: Run the Tailwind build**

From the worktree root, run:

```bash
npx tailwindcss -i static/css/input.css -o static/css/output.css --minify
```

(Equivalent to `just css-build`.)

**Step 2: Verify the new classes exist**

Spot-check that `output.css` now contains `animate-spin` and `text-blue-500`. A quick grep is sufficient.

**Step 3: Commit the regenerated CSS**

Stage and commit `static/css/output.css` alongside the template changes (or in the same final commit).

---

### Task 6: Update E2E tests

**Goal:** Update the existing `suggest.spec.js` test and add assertions for the new behaviors.

**Files:**
- Modify: `tests/e2e/suggest.spec.js` — update expectations to match new behavior

**Step 1: Remove preview image assertion from suggest test**

The current test asserts `await expect(page.getByTestId("bookmark-preview-image")).toBeVisible();` — this assertion must be removed since we're removing the preview image from the modal (Task 4).

**Step 2: Add assertion that modal fields are cleared after save**

After clicking "Add Bookmark" and the modal closing, reopen the modal and assert all fields are empty:

```javascript
// Reopen modal and verify fields are cleared
await page.getByTestId("open-add-bookmark-modal").click();
await expect(modal).toBeVisible();
await expect(urlInput).toHaveValue("");
await expect(titleInput).toHaveValue("");
await expect(descriptionInput).toHaveValue("");
```

**Step 3: Add assertion that bookmark card image is a clickable link**

After the card is created, verify the image is wrapped in an anchor:

```javascript
const imageLink = firstCard.getByTestId("bookmark-card-image-link");
await expect(imageLink).toHaveAttribute("href", /github\.com/);
```

**Step 4: Keep the card image assertion**

The existing assertion `firstCard.getByTestId("bookmark-card-image").toHaveAttribute("src", /\/uploads\/images\//)` should still pass since we're only removing the preview from the modal, not from the card.

---

### Task 7: Run `cargo test` and verify build

**Goal:** Ensure no Rust compilation errors or test failures.

**Steps:**
1. Run `cargo build` in the worktree to verify template compilation
2. Run `cargo test` to verify all existing tests pass
3. Fix any compilation errors from template changes

---

### Task 8: Agent-browser E2E verification with screenshots

**Goal:** Use Playwright MCP to take screenshots proving all 4 changes work.

**Prerequisites:** Start the dev server with `docker compose up` and `cargo run -p boopmark-server` (or use the E2E harness).

**Steps:**
1. Navigate to `/bookmarks`, sign in
2. Open the Add Bookmark modal, paste `https://github.com/danshapiro/trycycle`, tab out
3. **Screenshot A:** Capture the modal showing the loading spinner while the suggest request is in flight
4. **Screenshot B:** After response, capture the modal showing populated title/description/tags with NO preview image
5. Click "Add Bookmark" to save
6. Reopen the modal — **Screenshot C:** Capture the modal showing all fields are empty/cleared
7. On the homepage grid — **Screenshot D:** Capture the bookmark card showing the image is wrapped in a link (hover state or inspect)
