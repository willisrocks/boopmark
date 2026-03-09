# Fix OG Image Display Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Fix bookmark cards so they fetch, store, and display the og:image from bookmarked pages instead of showing a placeholder.

**Architecture:** Three issues prevent og:image display: (1) the local storage file path (`./uploads`) does not match the static file serving path (`./static`), so stored images are unreachable via HTTP; (2) the scraper sends requests without a User-Agent header, causing many sites (including GitHub) to return stripped HTML or block the request entirely; (3) there is no fallback to `twitter:image` meta tags. The fix adds a dedicated `/uploads` route to serve user-generated content separately from the git-tracked `./static` directory, adds a proper User-Agent to the scraper's HTTP client, and adds meta tag fallbacks.

**Tech Stack:** Rust, Axum, reqwest, scraper crate, tower-http ServeDir

---

### Task 1: Serve uploads directory on a dedicated route

The `LocalStorage` saves files to `./uploads` with public URL prefix `{APP_URL}/static/uploads`, but `ServeDir` only serves `./static` at `/static`. Instead of moving uploads into the git-tracked `static/` directory (which would pollute the repo with binary files), add a second `ServeDir` at `/uploads` that serves from `./uploads`, and update the `LocalStorage` public URL prefix to match.

**Files:**
- Modify: `server/src/web/router.rs:6-17`
- Modify: `server/src/main.rs:43-46`

**Step 1: Add the `/uploads` route to the router**

In `server/src/web/router.rs`, add a second `nest_service` call for uploads:

```rust
use axum::Router;
use tower_http::services::ServeDir;

use crate::web::state::AppState;

pub fn create_router(state: AppState) -> Router {
    Router::new()
        // API routes
        .nest("/api/v1", super::api::routes())
        // Page routes
        .merge(super::pages::routes())
        // Static files (checked-in assets: CSS, JS, etc.)
        .nest_service("/static", ServeDir::new("static"))
        // User-generated uploads (images, etc.)
        .nest_service("/uploads", ServeDir::new("uploads"))
        // Health check
        .route("/health", axum::routing::get(|| async { "ok" }))
        .with_state(state)
}
```

**Step 2: Update the LocalStorage public URL prefix**

In `server/src/main.rs`, change the `LocalStorage` initialization so the public URL prefix uses `/uploads` instead of `/static/uploads`:

```rust
let storage = Arc::new(LocalStorage::new(
    "./uploads".into(),
    format!("{}/uploads", config.app_url),
));
```

**Step 3: Verify the change compiles**

Run: `cargo build -p boopmark-server`
Expected: Compiles successfully.

**Step 4: Commit**

```bash
git add server/src/web/router.rs server/src/main.rs
git commit -m "fix: serve uploads on dedicated route instead of under static"
```

---

### Task 2: Add User-Agent header to scraper HTTP client

Many sites (including GitHub) return minimal or blocked responses when no User-Agent is provided. The scraper's `reqwest::Client` needs a User-Agent header.

**Files:**
- Modify: `server/src/adapters/scraper.rs:14-18`

**Step 1: Add a User-Agent to the client builder**

```rust
impl HtmlMetadataExtractor {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .user_agent("Boopmark/1.0 (+https://boopmark.app)")
                .build()
                .unwrap(),
        }
    }
}
```

**Step 2: Verify the change compiles**

Run: `cargo build -p boopmark-server`
Expected: Compiles successfully.

**Step 3: Commit**

```bash
git add server/src/adapters/scraper.rs
git commit -m "fix: add User-Agent header to metadata scraper"
```

---

### Task 3: Add twitter:image fallback to scraper

Some sites provide `twitter:image` but not `og:image`. The scraper should fall back to `twitter:image` when `og:image` is absent. Also add `meta[name="og:image"]` as a fallback since some sites use `name` instead of `property`.

**Files:**
- Modify: `server/src/adapters/scraper.rs:54`

**Step 1: Extend the image_url extraction chain**

Replace the single `og:image` lookup:

```rust
let image_url = select_meta(&document, "og:image").map(|img| resolve_url(url_str, &img));
```

With a fallback chain:

```rust
let image_url = select_meta(&document, "og:image")
    .or_else(|| select_meta_name(&document, "og:image"))
    .or_else(|| select_meta(&document, "twitter:image"))
    .or_else(|| select_meta_name(&document, "twitter:image"))
    .map(|img| resolve_url(url_str, &img));
```

**Step 2: Verify the change compiles**

Run: `cargo build -p boopmark-server`
Expected: Compiles successfully.

**Step 3: Commit**

```bash
git add server/src/adapters/scraper.rs
git commit -m "fix: add twitter:image fallback for og:image extraction"
```

---

### Task 4: Add User-Agent to the image download client in BookmarkService

The `download_and_store_image` method in `BookmarkService` creates a bare `reqwest::Client::new()` without a User-Agent. Some CDNs reject requests without one.

**Files:**
- Modify: `server/src/app/bookmarks.rs:73-74`

**Step 1: Add User-Agent to the image download client**

Replace:
```rust
let client = reqwest::Client::new();
```

With:
```rust
let client = reqwest::Client::builder()
    .user_agent("Boopmark/1.0 (+https://boopmark.app)")
    .timeout(std::time::Duration::from_secs(30))
    .build()
    .map_err(|e| DomainError::Internal(format!("client build error: {e}")))?;
```

**Step 2: Verify the change compiles**

Run: `cargo build -p boopmark-server`
Expected: Compiles successfully.

**Step 3: Commit**

```bash
git add server/src/app/bookmarks.rs
git commit -m "fix: add User-Agent and timeout to image download client"
```

---

### Task 5: Add unit tests for scraper og:image extraction

**Files:**
- Modify: `server/src/adapters/scraper.rs` (add `#[cfg(test)]` module at bottom)

**Step 1: Write the tests**

Add at the bottom of `server/src/adapters/scraper.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_og_image_from_property() {
        let html = Html::parse_document(
            r#"<html><head>
                <meta property="og:image" content="https://example.com/image.png">
                <meta property="og:title" content="Test Title">
                <meta property="og:description" content="Test Desc">
            </head><body></body></html>"#,
        );
        let img = select_meta(&html, "og:image");
        assert_eq!(img, Some("https://example.com/image.png".to_string()));
    }

    #[test]
    fn falls_back_to_twitter_image() {
        let html = Html::parse_document(
            r#"<html><head>
                <meta name="twitter:image" content="https://example.com/tw.png">
            </head><body></body></html>"#,
        );
        let img = select_meta(&html, "og:image")
            .or_else(|| select_meta_name(&html, "og:image"))
            .or_else(|| select_meta(&html, "twitter:image"))
            .or_else(|| select_meta_name(&html, "twitter:image"));
        assert_eq!(img, Some("https://example.com/tw.png".to_string()));
    }

    #[test]
    fn falls_back_to_name_attribute() {
        let html = Html::parse_document(
            r#"<html><head>
                <meta name="og:image" content="https://example.com/name.png">
            </head><body></body></html>"#,
        );
        let img = select_meta(&html, "og:image")
            .or_else(|| select_meta_name(&html, "og:image"));
        assert_eq!(img, Some("https://example.com/name.png".to_string()));
    }

    #[test]
    fn resolve_url_handles_absolute() {
        assert_eq!(
            resolve_url("https://example.com/page", "https://cdn.example.com/img.jpg"),
            "https://cdn.example.com/img.jpg"
        );
    }

    #[test]
    fn resolve_url_handles_relative() {
        assert_eq!(
            resolve_url("https://example.com/page", "/img.jpg"),
            "https://example.com/img.jpg"
        );
    }
}
```

**Step 2: Run the tests**

Run: `cargo test -p boopmark-server`
Expected: All 5 tests pass.

**Step 3: Commit**

```bash
git add server/src/adapters/scraper.rs
git commit -m "test: add unit tests for og:image scraper extraction"
```

---

### Task 6: E2E browser verification with Playwright

After all code changes are complete, verify end-to-end that bookmarking https://github.com/danshapiro/trycycle displays the og:image in the bookmark card. This requires the app to be running locally with `docker compose up` and access to a browser via Playwright MCP tools.

**Preconditions:** Tasks 1-5 must be complete. The app must be running locally (`cargo run -p boopmark-server` or `docker compose up`). A test user must be logged in.

**Step 1: Start the app locally**

Run: `docker compose up -d && cargo run -p boopmark-server`
Expected: Server starts on port 4000.

**Step 2: Navigate to the app and log in**

Use Playwright MCP `browser_navigate` to open `http://localhost:4000`. Log in with a test account (Google OAuth -- if no test account is available, this step requires manual intervention).

**Step 3: Add a bookmark for the test URL**

Use Playwright MCP tools to:
1. Click the "Add Bookmark" button to open the modal.
2. Fill the URL field with `https://github.com/danshapiro/trycycle`.
3. Submit the form.

**Step 4: Verify the bookmark card displays an image**

Use Playwright MCP `browser_snapshot` or `browser_take_screenshot` to capture the page. Verify that the newly created bookmark card contains an `<img>` element with a `src` attribute pointing to `/uploads/images/...` (not the placeholder emoji). The image should be the GitHub social preview for the trycycle repo.

**Step 5: Document results**

If the image displays correctly, the fix is verified. If not, inspect the network requests (`browser_network_requests`) and console (`browser_console_messages`) for errors and report findings.

**Note:** If Google OAuth is not available in the test environment, this task can be verified manually by the developer. The key verification is: (a) the scraper successfully fetched the og:image URL from the GitHub page, (b) the image was downloaded and stored to `./uploads/images/`, and (c) the bookmark card renders the stored image via the `/uploads/` route.
