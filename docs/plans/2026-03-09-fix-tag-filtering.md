# Fix Tag Filtering Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Fix tag filtering so clicking a tag filter button correctly shows only bookmarks with that tag, and ensure the tag filter bar retains all tags on full-page loads so users can switch between tags.

**Architecture:** Two bugs combine to break tag filtering. First, HTMX `hx-include` on tag buttons sends the empty search input as `search=`, and the handler passes `Some("")` into `BookmarkFilter.search`, adding a `plainto_tsquery('english', '')` clause that matches zero rows. Second, `collect_all_tags` derives the tag filter bar from the already-filtered results, so on full-page loads (browser refresh, back-navigation) with an active tag filter, tags not present in the filtered set disappear from the filter bar. The first fix normalizes empty search strings to `None` in the handler. The second adds a dedicated `all_tags` query (`SELECT DISTINCT unnest(tags)`) to the `BookmarkRepository` trait — this is more efficient than fetching full bookmark rows, correctly returns all tags regardless of count, and only runs on full-page loads (the HTMX partial path renders only `BookmarkList` with no filter bar).

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

### Task 2: Add `all_tags` query to the repository layer

Add a dedicated `all_tags` method that returns all distinct tags for a user via `SELECT DISTINCT unnest(tags)`. This is more efficient than fetching full bookmark rows and correctly returns all tags regardless of bookmark count (no LIMIT).

**Files:**
- Modify: `server/src/domain/ports/bookmark_repo.rs` (add `all_tags` to trait)
- Modify: `server/src/adapters/postgres/bookmark_repo.rs` (implement `all_tags`)
- Modify: `server/src/app/bookmarks.rs` (expose `all_tags` through service)

**Step 1: Add `all_tags` to the `BookmarkRepository` trait**

In `server/src/domain/ports/bookmark_repo.rs`, add after the `delete` method:

```rust
    async fn all_tags(&self, user_id: Uuid) -> Result<Vec<String>, DomainError>;
```

**Step 2: Implement `all_tags` in the Postgres adapter**

In `server/src/adapters/postgres/bookmark_repo.rs`, add inside the `impl BookmarkRepository for PostgresPool` block, after the `delete` method:

```rust
    async fn all_tags(&self, user_id: Uuid) -> Result<Vec<String>, DomainError> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT DISTINCT unnest(tags) AS tag FROM bookmarks WHERE user_id = $1 ORDER BY tag",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?;

        Ok(rows.into_iter().map(|(t,)| t).collect())
    }
```

**Step 3: Expose `all_tags` through `BookmarkService`**

In `server/src/app/bookmarks.rs`, add after the `extract_metadata` method:

```rust
    pub async fn all_tags(&self, user_id: Uuid) -> Result<Vec<String>, DomainError> {
        self.repo.all_tags(user_id).await
    }
```

**Step 4: Build to verify it compiles**

Run: `cargo build -p boopmark-server`
Expected: Compiles without errors.

**Step 5: Commit**

```bash
git add server/src/domain/ports/bookmark_repo.rs server/src/adapters/postgres/bookmark_repo.rs server/src/app/bookmarks.rs
git commit -m "feat: add all_tags query to BookmarkRepository for efficient tag listing"
```

### Task 3: Use `all_tags` for the tag filter bar on full-page loads

Replace `collect_all_tags(&bookmarks)` with the new `all_tags` query on full-page loads. On the HTMX partial path, the filter bar is not rendered, so no tag query is needed.

**Files:**
- Modify: `server/src/web/pages/bookmarks.rs:148-175` (tag bar + render logic)

**Step 1: Replace the tag collection and render block**

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
        // Full-page load: query all distinct tags for the filter bar.
        // This is a lightweight query (SELECT DISTINCT unnest(tags)) that
        // returns the complete tag set regardless of active filters.
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

**Step 2: Remove the now-unused `collect_all_tags` function**

Delete the `collect_all_tags` function (lines 305-314 of `server/src/web/pages/bookmarks.rs`):

```rust
fn collect_all_tags(bookmarks: &[Bookmark]) -> Vec<String> {
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

**Step 3: Build to verify it compiles**

Run: `cargo build -p boopmark-server`
Expected: Compiles without errors and no warnings about unused functions.

**Step 4: Commit**

```bash
git add server/src/web/pages/bookmarks.rs
git commit -m "fix: use all_tags query for filter bar on full-page loads"
```

### Task 4: Verify with agent-browser

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
