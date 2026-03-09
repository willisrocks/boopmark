# Tag Filter Active State & Toggle Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Make tag filter chips visually indicate the active/selected tag and support toggle-off (clicking an active tag clears the filter).

**Architecture:** Two changes are needed: (1) the filter bar template needs toggle-aware URLs so clicking an active tag clears the filter, and (2) HTMX partial responses need to include an out-of-band filter bar update so the active state renders after HTMX navigation. Uses HTMX's `hx-swap-oob` to update the filter bar alongside the bookmark list in a single response.

**Tech Stack:** Rust/Axum, Askama templates, HTMX 2, Tailwind CSS 4

---

### Task 1: Add toggle-off URL logic to the filter bar template

**Files:**
- Modify: `templates/components/filters.html`

The template already has the conditional styling for active tags (line 9-10). The issue is that `hx-get` always sets `tags={{ tag.name }}` regardless of active state. Fix: if the tag is active, the URL should omit the `tags` param to clear the filter.

**Step 1: Update the `hx-get` attribute to support toggle**

In `templates/components/filters.html`, change the tag button's `hx-get` attribute to conditionally send an empty `tags` value when the tag is already active:

```html
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
```

This way, clicking an active tag sends `tags=` (empty), which the handler already treats as "no filter".

**Step 2: Verify the change compiles**

Run: `cargo build`
Expected: Build succeeds (template changes are checked at compile time with Askama)

**Step 3: Commit**

```bash
git add templates/components/filters.html
git commit -m "feat: toggle tag filter off when clicking active tag chip"
```

---

### Task 2: Add an ID to the filter bar for OOB swapping

**Files:**
- Modify: `templates/components/filters.html`

Wrap the filter bar's outer `<div>` with an `id` attribute so HTMX can target it for out-of-band swaps.

**Step 1: Add `id="filter-bar"` to the filter bar container**

Change the opening div in `templates/components/filters.html` from:
```html
<div class="flex items-center gap-2 px-6 py-3 flex-wrap">
```
to:
```html
<div id="filter-bar" class="flex items-center gap-2 px-6 py-3 flex-wrap">
```

**Step 2: Verify it compiles**

Run: `cargo build`
Expected: Build succeeds

**Step 3: Commit**

```bash
git add templates/components/filters.html
git commit -m "feat: add id to filter bar div for HTMX OOB targeting"
```

---

### Task 3: Create a new partial template for HTMX responses with OOB filter bar

**Files:**
- Create: `templates/bookmarks/list_with_filters.html`

This template returns the bookmark list (the primary swap content for `#bookmark-grid`) plus an out-of-band swap of the filter bar. This is how HTMX updates multiple page sections from one response.

**Step 1: Create the template file**

Create `templates/bookmarks/list_with_filters.html` with this content:

```html
{% for bookmark in bookmarks %}
{% include "bookmarks/card.html" %}
{% endfor %}
{% if bookmarks.is_empty() %}
<div class="col-span-full text-center py-12 text-gray-500">
    No bookmarks found.
</div>
{% endif %}
<div id="filter-bar" hx-swap-oob="true" class="flex items-center gap-2 px-6 py-3 flex-wrap">
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
</div>
```

Note: This duplicates the filter bar markup. This is intentional -- the OOB swap replaces the full `#filter-bar` element including its attributes. Keeping it in a separate template avoids adding OOB attributes to the shared `filters.html` include (which would break the full-page render where OOB is not wanted).

**Step 2: Verify it compiles**

Run: `cargo build`
Expected: Build will fail because no Rust struct references this template yet (that's Task 4)

**Step 3: Commit**

```bash
git add templates/bookmarks/list_with_filters.html
git commit -m "feat: add HTMX partial template with OOB filter bar update"
```

---

### Task 4: Update the Rust handler to return filter tags in HTMX responses

**Files:**
- Modify: `server/src/web/pages/bookmarks.rs`

Currently the `list` handler returns `BookmarkList` (just cards) for HTMX requests. Change it to return a new `BookmarkListWithFilters` struct that includes filter tags, so the OOB filter bar renders with correct active state.

**Step 1: Add the new Askama template struct**

In `server/src/web/pages/bookmarks.rs`, add a new struct after the existing `BookmarkList`:

```rust
#[derive(Template)]
#[template(path = "bookmarks/list_with_filters.html")]
struct BookmarkListWithFilters {
    bookmarks: Vec<BookmarkView>,
    filter_tags: Vec<TagView>,
    sort: String,
}
```

**Step 2: Update the HTMX branch to query tags and return the new template**

In the `list` function, change the HTMX branch (the `if is_htmx(&headers)` block) from:

```rust
if is_htmx(&headers) {
    render(&BookmarkList {
        bookmarks: bookmark_views,
    })
}
```

to:

```rust
if is_htmx(&headers) {
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

    render(&BookmarkListWithFilters {
        bookmarks: bookmark_views,
        filter_tags,
        sort: sort_str,
    })
}
```

Note: This duplicates the `all_tags` / `filter_tags` logic from the full-page branch. To DRY it up, extract the tag query before the `if is_htmx` branch so both paths share it. Here is the refactored `list` function body from `let bookmark_views` onward:

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

**Step 3: Verify it compiles**

Run: `cargo build`
Expected: Build succeeds

**Step 4: Commit**

```bash
git add server/src/web/pages/bookmarks.rs
git commit -m "feat: return filter bar with active state in HTMX responses via OOB swap"
```

---

### Task 5: Rebuild Tailwind CSS

**Files:**
- Modify: `static/css/tailwind-output.css` (generated)

The new template uses Tailwind classes that may already be in the output. Rebuild to be safe.

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

**Step 4: Save screenshots with descriptive names**

Save screenshots as:
- `screenshot-tag-filter-no-active.png`
- `screenshot-tag-filter-active.png`
- `screenshot-tag-filter-toggled-off.png`
