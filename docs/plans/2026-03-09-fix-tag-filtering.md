# Fix Tag Filtering Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Fix tag filtering so clicking a tag filter button correctly shows only bookmarks with that tag, and ensure the tag filter bar retains all tags on full-page loads so users can switch between tags.

**Architecture:** Two bugs combine to break tag filtering. First, HTMX `hx-include` on tag buttons sends the empty search input as `search=`, and the handler passes `Some("")` into `BookmarkFilter.search`, adding a `plainto_tsquery('english', '')` clause that matches zero rows. Second, `collect_all_tags` derives the tag filter bar from the already-filtered results, so on full-page loads (browser refresh, back-navigation) with an active tag filter, tags not present in the filtered set disappear from the filter bar. Both fixes are in `server/src/web/pages/bookmarks.rs`. The HTMX partial path renders only `BookmarkList` (no filter bar), so the tag bar fix only matters on full-page loads where we issue an additional unfiltered query to get the complete tag set.

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

### Task 2: Populate tag filter bar from unfiltered results on full-page loads

The HTMX partial path returns only `BookmarkList` (no filter bar), so `filter_tags` only matters on the `GridPage` full-page path. When there are active filters, we need an additional unfiltered query to populate the complete tag bar. This uses the existing `list` method with `BookmarkFilter::default()` — no new trait methods needed.

**Files:**
- Modify: `server/src/web/pages/bookmarks.rs:145-175` (tag bar + render logic)

**Step 1: Capture `has_active_filter` before filter is consumed**

Add this line immediately before `let bookmarks = with_bookmarks!(...)` (before line 145):

```rust
    let has_active_filter = filter.search.is_some() || filter.tags.is_some();
```

**Step 2: Replace the tag collection and render block**

Replace lines 148-175 (from `let all_tags = collect_all_tags` through the end of the `else` render block):

```rust
    let all_tags = collect_all_tags(&bookmarks);
    let filter_tags: Vec<TagView> = all_tags
        .into_iter()
        .map(|name| {
            let active = active_tags.contains(&name);
            TagView { name, active }
        })
        .collect();

    let bookmark_views: Vec<BookmarkView> = bookmarks.into_iter().map(Into::into).collect();

    if is_htmx(&headers) {
        render(&BookmarkList {
            bookmarks: bookmark_views,
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

With:

```rust
    let bookmark_views: Vec<BookmarkView> = bookmarks.into_iter().map(Into::into).collect();

    if is_htmx(&headers) {
        render(&BookmarkList {
            bookmarks: bookmark_views,
        })
    } else {
        // On full-page loads with active filters, fetch unfiltered bookmarks
        // to populate the complete tag bar so users can switch between tags.
        let tag_names = if has_active_filter {
            let all_bookmarks = with_bookmarks!(&state.bookmarks, svc =>
                svc.list(user.id, BookmarkFilter::default()).await
            )
            .unwrap_or_default();
            collect_all_tags(&all_bookmarks)
        } else {
            collect_all_tag_names(&bookmark_views)
        };
        let filter_tags: Vec<TagView> = tag_names
            .into_iter()
            .map(|name| {
                let active = active_tags.contains(&name);
                TagView { name, active }
            })
            .collect();

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

**Step 3: Add `collect_all_tag_names` helper for `BookmarkView`**

Add this function alongside the existing `collect_all_tags` (near line 305):

```rust
fn collect_all_tag_names(bookmarks: &[BookmarkView]) -> Vec<String> {
    let mut tags: Vec<String> = bookmarks
        .iter()
        .flat_map(|b| b.tags.iter().cloned())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    tags.sort();
    tags
}
```

This is needed because `bookmarks` (the `Vec<Bookmark>`) has been moved into `bookmark_views` (the `Vec<BookmarkView>`) by the time we need to collect tags on the unfiltered path.

**Step 4: Build to verify it compiles**

Run: `cargo build -p boopmark-server`
Expected: Compiles without errors.

**Step 5: Commit**

```bash
git add server/src/web/pages/bookmarks.rs
git commit -m "fix: populate tag filter bar from unfiltered results on full-page loads"
```

### Task 3: Verify with agent-browser

**Step 1: Start the dev server if not running**

Run: `docker compose up` (if not already running)

**Step 2: Navigate to the bookmarks page and take a screenshot showing bookmarks with tags**

Navigate to the bookmarks page. Take a screenshot showing bookmarks are visible with tag filter buttons.

**Step 3: Click a tag filter button and take a screenshot showing filtered results**

Click a tag filter button (e.g., "development"). Take a screenshot showing:
- Only bookmarks with that tag are displayed
- The clicked tag button appears active/highlighted

**Step 4: Refresh the page with the tag filter still active and take a screenshot**

Refresh the browser (full-page load with `?tags=development` in the URL). Take a screenshot showing:
- Only bookmarks with that tag are displayed
- The tag filter bar still shows ALL available tags (not just tags from filtered results)

**Step 5: Test search still works**

Clear the tag filter, type a search term in the search bar (e.g., "trycycle"). Take a screenshot showing search results are returned correctly.
