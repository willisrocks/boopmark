# Fix Tag Filtering Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Fix tag filtering so clicking a tag filter button correctly shows only bookmarks with that tag, and fix the tag filter bar so it always shows all available tags regardless of current filters.

**Architecture:** Two bugs cause tag filtering to appear broken. First, HTMX `hx-include` sends empty search strings (`search=`) alongside tag filters; the handler passes `Some("")` to the Postgres full-text search clause, which matches nothing — zeroing out results. Second, the tag filter bar is derived from already-filtered results, so it loses tags not in the current result set. Both fixes are in `server/src/web/pages/bookmarks.rs` — one normalizes the search field, the other fetches all tags from an unfiltered query.

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

### Task 2: Fetch all tags from an unfiltered query for the filter bar

**Files:**
- Modify: `server/src/web/pages/bookmarks.rs:145-155` (tag bar construction)
- Modify: `server/src/domain/ports/bookmark_repo.rs` (add `all_tags` method)
- Modify: `server/src/adapters/postgres/bookmark_repo.rs` (implement `all_tags`)
- Modify: `server/src/app/bookmarks.rs` (expose `all_tags` through service)

**Step 1: Add `all_tags` to the BookmarkRepository port**

In `server/src/domain/ports/bookmark_repo.rs`, add a method to the trait:

```rust
async fn all_tags(&self, user_id: Uuid) -> Result<Vec<String>, DomainError>;
```

**Step 2: Implement `all_tags` in the Postgres adapter**

In `server/src/adapters/postgres/bookmark_repo.rs`, add:

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

**Step 3: Expose `all_tags` through BookmarkService**

In `server/src/app/bookmarks.rs`, add:

```rust
    pub async fn all_tags(&self, user_id: Uuid) -> Result<Vec<String>, DomainError> {
        self.repo.all_tags(user_id).await
    }
```

**Step 4: Update the list handler to use `all_tags`**

In `server/src/web/pages/bookmarks.rs`, replace the tag bar construction block (lines 148-155):

```rust
    let all_tags = collect_all_tags(&bookmarks);
    let filter_tags: Vec<TagView> = all_tags
        .into_iter()
        .map(|name| {
            let active = active_tags.contains(&name);
            TagView { name, active }
        })
        .collect();
```

With:

```rust
    let all_tags = with_bookmarks!(&state.bookmarks, svc => svc.all_tags(user.id).await)
        .unwrap_or_default();
    let filter_tags: Vec<TagView> = all_tags
        .into_iter()
        .map(|name| {
            let active = active_tags.contains(&name);
            TagView { name, active }
        })
        .collect();
```

**Step 5: Remove the now-unused `collect_all_tags` function**

Delete the `collect_all_tags` function (lines 305-314 of `server/src/web/pages/bookmarks.rs`).

**Step 6: Build to verify it compiles**

Run: `cargo build -p boopmark-server`
Expected: Compiles without errors. If there is a compiler warning about unused `collect_all_tags`, ensure it was removed.

**Step 7: Commit**

```bash
git add server/src/web/pages/bookmarks.rs server/src/domain/ports/bookmark_repo.rs server/src/adapters/postgres/bookmark_repo.rs server/src/app/bookmarks.rs
git commit -m "fix: fetch all tags from DB for filter bar instead of deriving from filtered results"
```

### Task 3: Verify with agent-browser

**Step 1: Start the dev server if not running**

Run: `docker compose up` (if not already running)

**Step 2: Navigate to the bookmarks page and take a screenshot showing bookmarks with tags**

Navigate to the bookmarks page. Take a screenshot showing bookmarks are visible with tag filter buttons.

**Step 3: Click a tag filter button and take a screenshot showing filtered results**

Click a tag filter button (e.g., "development"). Take a screenshot showing:
- Only bookmarks with that tag are displayed
- The tag filter bar still shows all available tags
- The clicked tag button appears active/highlighted

**Step 4: Test search still works**

Clear the tag filter, type a search term in the search bar (e.g., "trycycle"). Take a screenshot showing search results are returned correctly.

**Step 5: Commit screenshots as evidence (optional — per user request)**
