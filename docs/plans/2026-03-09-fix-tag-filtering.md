# Fix Tag Filtering Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Fix tag filtering so clicking a tag filter button correctly shows only bookmarks with that tag.

**Architecture:** A single bug causes tag filtering to return zero results. HTMX `hx-include` on the tag filter buttons sends the empty search input as `search=` alongside the tag parameter. The handler passes `Some("")` into `BookmarkFilter.search`, which adds a `plainto_tsquery('english', '')` clause to the Postgres query. An empty tsquery matches zero rows, zeroing out all results regardless of the tag filter. The fix is a one-line normalization in the list handler to convert empty/whitespace-only search strings to `None`.

**Tech Stack:** Rust, Axum, SQLx, Askama, HTMX, Postgres

---

### Task 1: Normalize empty search strings to None in the list handler

**Files:**
- Modify: `server/src/web/pages/bookmarks.rs:134` (the `filter` construction)

**Step 1: Write the fix**

In the `list` handler, change the `BookmarkFilter` construction to filter out empty/whitespace-only search strings. Replace:

```rust
    let filter = BookmarkFilter {
        search: query.search,
        tags: if active_tags.is_empty() {
            None
        } else {
            Some(active_tags.clone())
        },
        sort: Some(sort),
        ..Default::default()
    };
```

With:

```rust
    let filter = BookmarkFilter {
        search: query.search.and_then(|s| {
            let trimmed = s.trim().to_string();
            if trimmed.is_empty() { None } else { Some(trimmed) }
        }),
        tags: if active_tags.is_empty() {
            None
        } else {
            Some(active_tags.clone())
        },
        sort: Some(sort),
        ..Default::default()
    };
```

**Step 2: Build to verify it compiles**

Run: `cargo build -p boopmark-server`
Expected: Compiles without errors.

**Step 3: Commit**

```bash
git add server/src/web/pages/bookmarks.rs
git commit -m "fix: normalize empty search strings to None so tag filtering works"
```

### Task 2: Verify with agent-browser

**Step 1: Start the dev server if not running**

Run: `docker compose up` (if not already running)

**Step 2: Navigate to the bookmarks page and take a screenshot showing bookmarks with tags**

Navigate to the bookmarks page. Take a screenshot showing bookmarks are visible with tag filter buttons.

**Step 3: Click a tag filter button and take a screenshot showing filtered results**

Click a tag filter button (e.g., "development"). Take a screenshot showing:
- Only bookmarks with that tag are displayed
- The clicked tag button appears active/highlighted

**Step 4: Test search still works**

Clear the tag filter, type a search term in the search bar (e.g., "trycycle"). Take a screenshot showing search results are returned correctly.
