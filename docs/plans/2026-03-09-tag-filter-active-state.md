# Tag Filter Active State & Toggle Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Make tag filter chips visually indicate the active/selected tag and support toggle-off (clicking an active tag clears the filter).

**Architecture:** Three changes: (1) toggle-aware URLs on tag chips so clicking an active tag clears the filter, (2) HTMX partial responses include an out-of-band filter bar update so the active state renders after HTMX navigation, and (3) a hidden input carries the active tag value so the sort dropdown preserves the tag filter. The filter bar markup is extracted into a shared inner template (`filters_inner.html`) to avoid duplication between the full-page and OOB templates.

**Tech Stack:** Rust/Axum, Askama templates, HTMX 2, Tailwind CSS 4

---

### Task 1: Extract filter bar inner content into a shared template

**Files:**
- Create: `templates/components/filters_inner.html`
- Modify: `templates/components/filters.html`

Extract everything inside the outer `<div>` of `filters.html` into `filters_inner.html`, then have `filters.html` include it. This sets up a single source of truth for filter bar content that both the full-page and OOB templates can share.

**Step 1: Create `templates/components/filters_inner.html`**

Create the file with the inner content of the current `filters.html` (everything between the outer `<div>` tags). Also apply the toggle-off logic and hidden tags input in this step to avoid touching the file multiple times:

```html
    <button class="text-sm text-gray-400 flex items-center gap-1">
        <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 4a1 1 0 011-1h16a1 1 0 011 1v2.586a1 1 0 01-.293.707l-6.414 6.414a1 1 0 00-.293.707V17l-4 4v-6.586a1 1 0 00-.293-.707L3.293 7.293A1 1 0 013 6.586V4z"/>
        </svg>
        Filters
    </button>
    {% for tag in filter_tags %}
    <button class="px-3 py-1 text-xs rounded-full border
                    {% if tag.active %}bg-blue-600 border-blue-500 text-white{% else %}bg-[#1a1d2e] border-gray-700 text-gray-400 hover:border-gray-500{% endif %}"
            hx-get="/bookmarks?tags={% if tag.active %}{% else %}{{ tag.name }}{% endif %}"
            hx-target="#bookmark-grid"
            hx-push-url="true"
            hx-include="[name='search'],[name='sort']">
        {{ tag.name }}
    </button>
    {% endfor %}
    <input type="hidden" name="tags" value="{% for tag in filter_tags %}{% if tag.active %}{{ tag.name }}{% endif %}{% endfor %}">
    <div class="ml-auto">
        <select name="sort"
                class="bg-[#1a1d2e] border border-gray-700 rounded-lg px-3 py-1 text-sm text-gray-300"
                hx-get="/bookmarks"
                hx-trigger="change"
                hx-target="#bookmark-grid"
                hx-include="[name='search'],[name='tags']">
            <option value="newest" {% if sort == "newest" %}selected{% endif %}>Newest First</option>
            <option value="oldest" {% if sort == "oldest" %}selected{% endif %}>Oldest First</option>
            <option value="title" {% if sort == "title" %}selected{% endif %}>Title</option>
            <option value="domain" {% if sort == "domain" %}selected{% endif %}>Domain</option>
        </select>
    </div>
```

Key changes from the original `filters.html` content:
- **Toggle-off:** `hx-get` URL conditionally sends empty `tags=` when `tag.active` is true, so clicking an active tag clears the filter.
- **Hidden tags input:** A `<input type="hidden" name="tags">` carries the active tag name. This fixes the sort dropdown's `hx-include="[name='tags']"` which previously could not find any element with `name="tags"` in the DOM, causing the sort dropdown to silently drop the active tag filter when changing sort order.

**Step 2: Replace `filters.html` content with an include wrapper**

Replace the entire content of `templates/components/filters.html` with:

```html
<div id="filter-bar" class="flex items-center gap-2 px-6 py-3 flex-wrap">
    {% include "components/filters_inner.html" %}
</div>
```

This adds the `id="filter-bar"` (needed for OOB targeting in Task 2) and delegates all content to the shared inner template.

**Step 3: Verify it compiles**

Run: `cargo build`
Expected: Build succeeds. Askama includes share the parent template's variable scope, so `filter_tags` and `sort` resolve correctly.

**Step 4: Commit**

```bash
git add templates/components/filters_inner.html templates/components/filters.html
git commit -m "refactor: extract filter bar inner content into shared template with toggle and hidden tags input"
```

---

### Task 2: Create an OOB filter bar template for HTMX responses

**Files:**
- Create: `templates/components/filters_oob.html`

This template renders the same filter bar content but with `hx-swap-oob="true"` on the outer div. HTMX will use this to replace the `#filter-bar` element out-of-band when it appears alongside the primary swap content in a response.

**Step 1: Create `templates/components/filters_oob.html`**

```html
<div id="filter-bar" hx-swap-oob="true" class="flex items-center gap-2 px-6 py-3 flex-wrap">
    {% include "components/filters_inner.html" %}
</div>
```

The only difference from `filters.html` is the `hx-swap-oob="true"` attribute. Both files include `filters_inner.html`, so there is a single source of truth for the filter bar content.

**Step 2: Commit**

```bash
git add templates/components/filters_oob.html
git commit -m "feat: add OOB filter bar template for HTMX partial responses"
```

---

### Task 3: Create the HTMX partial response template

**Files:**
- Create: `templates/bookmarks/list_with_filters.html`

This template is used for HTMX responses. It returns the bookmark list as the primary swap content (targeting `#bookmark-grid`) plus the OOB filter bar.

**Step 1: Create `templates/bookmarks/list_with_filters.html`**

```html
{% include "bookmarks/list.html" %}
{% include "components/filters_oob.html" %}
```

This reuses both existing templates:
- `bookmarks/list.html` renders the bookmark cards (the primary content swapped into `#bookmark-grid`)
- `components/filters_oob.html` renders the filter bar with `hx-swap-oob="true"` (replaces `#filter-bar` out-of-band)

**Step 2: Commit**

```bash
git add templates/bookmarks/list_with_filters.html
git commit -m "feat: add HTMX partial template combining bookmark list with OOB filter bar"
```

---

### Task 4: Update the Rust handler to return filter tags in HTMX responses

**Files:**
- Modify: `server/src/web/pages/bookmarks.rs`

Currently the `list` handler returns `BookmarkList` (just cards) for HTMX requests, and only queries tags for full-page loads. Change it to always query tags and return a new `BookmarkListWithFilters` struct for HTMX responses.

**Step 1: Add the new Askama template struct**

In `server/src/web/pages/bookmarks.rs`, add a new struct after the existing `BookmarkList` (after line 89):

```rust
#[derive(Template)]
#[template(path = "bookmarks/list_with_filters.html")]
struct BookmarkListWithFilters {
    bookmarks: Vec<BookmarkView>,
    filter_tags: Vec<TagView>,
    sort: String,
}
```

**Step 2: Refactor the `list` function to share the tag query**

Replace the code from `let bookmark_views` (line 151) through the end of the function (line 184) with:

```rust
    let bookmark_views: Vec<BookmarkView> = bookmarks.into_iter().map(Into::into).collect();

    // Query all distinct tags for the filter bar (used by both HTMX and full-page paths).
    let all_tag_names = with_bookmarks!(&state.bookmarks, svc =>
        svc.all_tags(user.id).await
    )
    .unwrap_or_default();
    let filter_tags: Vec<TagView> = all_tag_names
        .into_iter()
        .map(|name| {
            let active = active_tags.contains(&name);
            TagView { name, active }
        })
        .collect();

    if is_htmx(&headers) {
        render(&BookmarkListWithFilters {
            bookmarks: bookmark_views,
            filter_tags,
            sort: sort_str,
        })
    } else {
        render(&GridPage {
            user: Some(user.into()),
            header_shows_bookmark_actions: true,
            bookmarks: bookmark_views,
            filter_tags,
            sort: sort_str,
            suggest_title: String::new(),
            suggest_description: String::new(),
            suggest_preview_image_url: None,
            suggest_tags: String::new(),
        })
    }
```

This moves the `all_tags` query and `filter_tags` construction before the `if is_htmx` branch so both paths share it. The full-page branch previously had this code inline (lines 161-171); it is now shared.

**Step 3: Verify it compiles**

Run: `cargo build`
Expected: Build succeeds

**Step 4: Run tests**

Run: `cargo test`
Expected: All tests pass

**Step 5: Commit**

```bash
git add server/src/web/pages/bookmarks.rs
git commit -m "feat: return filter bar with active state in HTMX responses via OOB swap"
```

---

### Task 5: Rebuild Tailwind CSS

**Files:**
- Modify: `static/css/tailwind-output.css` (generated)

The new templates reuse existing Tailwind classes, but rebuild to ensure the scanner picks up classes from the new template files.

**Step 1: Rebuild Tailwind**

Run: `./tailwindcss-macos-arm64 -i static/css/tailwind-input.css -o static/css/tailwind-output.css --minify`
Expected: CSS file regenerated

**Step 2: Check if the output changed**

Run: `git diff --stat`
Expected: If the CSS changed, commit it. If unchanged, skip.

**Step 3: Commit (if changed)**

```bash
git add static/css/tailwind-output.css
git commit -m "build: rebuild Tailwind CSS output"
```

---

### Task 6: Verify via agent-browser screenshots

**Files:** None (manual verification)

Use the Playwright MCP / agent-browser to take screenshots proving both features work against the local dev server at `http://localhost:4000/bookmarks`.

**Step 1: Take a screenshot of the bookmarks page with no filter active**

Navigate to `http://localhost:4000/bookmarks` and take a screenshot.
Expected: All tag chips have the inactive (dark) styling.

**Step 2: Click a tag chip (e.g., "ai") and take a screenshot**

Click the "ai" tag chip and take a screenshot.
Expected: The "ai" chip has the active (blue) styling. Only bookmarks with the "ai" tag are shown.

**Step 3: Click the same tag chip again and take a screenshot**

Click the "ai" chip again (now active) and take a screenshot.
Expected: The filter is cleared. All chips return to inactive styling. All bookmarks are shown.

**Step 4: Verify sort preserves tag filter**

With a tag active, change the sort dropdown and take a screenshot.
Expected: The tag filter remains active after changing sort order (the hidden `tags` input preserves it).

**Step 5: Save screenshots with descriptive names**

Save screenshots as:
- `screenshot-tag-filter-no-active.png`
- `screenshot-tag-filter-active.png`
- `screenshot-tag-filter-toggled-off.png`
- `screenshot-tag-filter-sort-preserves-tag.png`
