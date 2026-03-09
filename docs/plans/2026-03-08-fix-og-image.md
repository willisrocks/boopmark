# Fix OG Image Display Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Fix bookmark cards so they fetch, store, and display the og:image from bookmarked pages instead of showing a placeholder.

**Architecture:** Two bugs prevent og:image display: (1) the local storage file path (`./uploads`) does not match the static file serving path (`./static`), so stored images are unreachable via HTTP; (2) the scraper sends requests without a User-Agent header, causing many sites (including GitHub) to return stripped HTML or block the request entirely. The fix aligns storage paths with the static file server and adds a proper User-Agent to the scraper's HTTP client.

**Tech Stack:** Rust, Axum, reqwest, scraper crate, tower-http ServeDir

---

### Task 1: Fix local storage path to align with static file serving

The `LocalStorage` is initialized with base dir `./uploads` and public URL prefix `{APP_URL}/static/uploads`, but `ServeDir` serves `./static` at `/static`. Files saved to `./uploads/images/xxx.jpg` are expected at URL `/static/uploads/images/xxx.jpg`, which maps to filesystem `./static/uploads/images/xxx.jpg` -- but the file is actually at `./uploads/images/xxx.jpg`.

**Files:**
- Modify: `server/src/main.rs:43-46`

**Step 1: Fix the LocalStorage initialization**

Change the base directory from `"./uploads"` to `"./static/uploads"` so that files saved by LocalStorage land inside the `./static` tree where `ServeDir` can find them.

```rust
let storage = Arc::new(LocalStorage::new(
    "./static/uploads".into(),
    format!("{}/static/uploads", config.app_url),
));
```

**Step 2: Verify the change compiles**

Run: `cargo build -p boopmark-server`
Expected: Compiles successfully.

**Step 3: Commit**

```bash
git add server/src/main.rs
git commit -m "fix: align local storage path with static file server"
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

### Task 5: Add unit test for scraper og:image extraction

**Files:**
- Modify: `server/src/adapters/scraper.rs` (add `#[cfg(test)]` module at bottom)

**Step 1: Write the test**

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
