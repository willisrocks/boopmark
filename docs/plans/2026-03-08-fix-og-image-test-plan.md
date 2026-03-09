# Fix OG Image Display — Test Plan

## Strategy reconciliation

The agreed testing strategy calls for (1) E2E agent-browser tests using https://github.com/danshapiro/trycycle as the bookmark URL, and (2) unit tests for scraper extraction logic.

After reviewing the implementation plan against the codebase:

- **E2E tests**: The plan's Task 6 describes a Playwright-based E2E flow. However, the app uses Google OAuth exclusively (no username/password login). There is no test account bypass, no API key auth, and no way to programmatically log in via Playwright without real Google credentials. The E2E browser test **cannot be fully automated** in a local test run. The strategy adjustment is: replace the Playwright login step with a **semi-automated verification protocol** — the E2E test plan documents exact manual steps and observable assertions so the developer can execute them, and we add an **integration-level HTTP test** that exercises the scraper-to-storage pipeline without requiring OAuth.
- **Unit tests**: The planned interfaces (`select_meta`, `select_meta_name`, `resolve_url`) are free functions in `scraper.rs`, directly testable. The strategy holds as-is.
- **Storage path mismatch**: Verifiable via a unit-style test of `LocalStorage::public_url` and an integration test hitting `/uploads/` route.
- **No new external dependencies** beyond what the strategy anticipated.

These adjustments do not change cost or scope — they make the E2E portion executable without requiring Google OAuth credentials.

---

## Harness requirements

### Harness 1: Scraper unit test harness

- **What it does**: Provides HTML string fixtures to `select_meta`, `select_meta_name`, and `resolve_url` functions.
- **What it exposes**: Direct function calls on the free functions in `scraper.rs`.
- **Estimated complexity**: Trivial — inline `#[cfg(test)]` module, no external setup.
- **Tests that depend on it**: Tests 4, 5, 6, 7, 8, 9, 10.

### Harness 2: E2E browser verification protocol

- **What it does**: Documents exact Playwright MCP steps for manual/semi-automated verification with a running local app.
- **What it exposes**: Browser snapshot assertions, network request inspection, DOM assertions.
- **Estimated complexity**: Low — uses existing Playwright MCP tools. Requires a logged-in session (manual Google OAuth).
- **Tests that depend on it**: Tests 1, 2, 3.

---

## Test plan

### Test 1: Adding a bookmark for a URL with og:image displays the social preview image in the card

- **Name**: Bookmark card shows og:image from bookmarked page
- **Type**: scenario
- **Harness**: E2E browser verification protocol (Playwright MCP)
- **Preconditions**: App running locally (`docker compose up -d && cargo run -p boopmark-server`). User logged in via Google OAuth. No existing bookmark for `https://github.com/danshapiro/trycycle`.
- **Actions**:
  1. Navigate to `http://localhost:4000/bookmarks`.
  2. Click the "Add Bookmark" button (opens `#add-modal`).
  3. Fill the URL field with `https://github.com/danshapiro/trycycle`.
  4. Leave Title empty (triggers auto-extraction via `MetadataExtractor`).
  5. Click "Add Bookmark" submit button.
  6. Wait for the HTMX response to prepend the new card to `#bookmark-grid`.
- **Expected outcome**:
  - The new bookmark card appears at the top of the grid.
  - The card contains an `<img>` element inside the `.h-40` image container.
  - The `<img>` `src` attribute starts with `/uploads/images/` (not the placeholder emoji `&#128278;`).
  - The card title contains "trycycle" (auto-extracted from og:title or `<title>`).
  - **Source of truth**: The GitHub page at `https://github.com/danshapiro/trycycle` contains `<meta property="og:image" content="...">` with a valid image URL. The implementation plan specifies that the image should be downloaded, stored to `./uploads/images/`, and served via `/uploads/`.
- **Interactions**: MetadataExtractor (HTTP fetch to GitHub), image download (HTTP fetch to GitHub's CDN), LocalStorage (filesystem write to `./uploads/images/`), ServeDir (HTTP serving from `/uploads/`), PostgreSQL (bookmark row insert).

### Test 2: Stored image is accessible via the /uploads HTTP route

- **Name**: Image stored in uploads directory is served by the /uploads route
- **Type**: integration
- **Harness**: E2E browser verification protocol (Playwright MCP)
- **Preconditions**: Test 1 has been executed. A bookmark with an image exists.
- **Actions**:
  1. From the bookmark card created in Test 1, extract the `<img>` `src` attribute value.
  2. Navigate directly to `http://localhost:4000{src_value}` in the browser.
- **Expected outcome**:
  - The browser displays the image (HTTP 200 response with an image content-type).
  - The image is not a 404 or error page.
  - **Source of truth**: Implementation plan Task 1 specifies that `/uploads` is served by `ServeDir::new("uploads")`, and `LocalStorage` public URL prefix uses `/uploads` (not `/static/uploads`).
- **Interactions**: tower-http ServeDir, filesystem.

### Test 3: Bookmark card without og:image shows placeholder

- **Name**: Bookmark card for a URL without og:image shows the placeholder emoji
- **Type**: scenario
- **Harness**: E2E browser verification protocol (Playwright MCP)
- **Preconditions**: App running locally. User logged in.
- **Actions**:
  1. Add a bookmark for a URL known to lack og:image (e.g., `https://example.com`).
  2. Observe the newly created bookmark card.
- **Expected outcome**:
  - The card's image container shows the link emoji placeholder (`&#128278;`), not a broken `<img>` tag.
  - The card does NOT contain an `<img>` element in the image area.
  - **Source of truth**: The `card.html` template renders the emoji placeholder when `bookmark.image_url` is `None`. `https://example.com` has no og:image meta tag.
- **Interactions**: MetadataExtractor (HTTP fetch to example.com returns no og:image), template rendering.

### Test 4: Scraper extracts og:image from property attribute

- **Name**: og:image is extracted from meta property="og:image"
- **Type**: unit
- **Harness**: Scraper unit test harness
- **Preconditions**: None.
- **Actions**:
  1. Parse an HTML string containing `<meta property="og:image" content="https://example.com/image.png">`.
  2. Call `select_meta(&document, "og:image")`.
- **Expected outcome**:
  - Returns `Some("https://example.com/image.png")`.
  - **Source of truth**: The HTML meta tag specification — `property="og:image"` with `content` attribute is the standard Open Graph image tag format.
- **Interactions**: None (pure function).

### Test 5: Scraper falls back to name="og:image" when property is absent

- **Name**: og:image extracted via name attribute when property attribute is missing
- **Type**: unit
- **Harness**: Scraper unit test harness
- **Preconditions**: None.
- **Actions**:
  1. Parse HTML with only `<meta name="og:image" content="https://example.com/name.png">` (no `property` attribute version).
  2. Call `select_meta(&document, "og:image")` — should return `None`.
  3. Call `select_meta_name(&document, "og:image")` — should return the value.
- **Expected outcome**:
  - `select_meta` returns `None`.
  - `select_meta_name` returns `Some("https://example.com/name.png")`.
  - **Source of truth**: Implementation plan Task 3 specifies fallback to `name` attribute. Some sites incorrectly use `name` instead of `property` for og tags.
- **Interactions**: None (pure function).

### Test 6: Scraper falls back to twitter:image when og:image is absent

- **Name**: twitter:image is used when og:image is not present
- **Type**: unit
- **Harness**: Scraper unit test harness
- **Preconditions**: None.
- **Actions**:
  1. Parse HTML with only `<meta name="twitter:image" content="https://example.com/tw.png">`.
  2. Execute the full fallback chain: `select_meta("og:image").or_else(|| select_meta_name("og:image")).or_else(|| select_meta("twitter:image")).or_else(|| select_meta_name("twitter:image"))`.
- **Expected outcome**:
  - Returns `Some("https://example.com/tw.png")`.
  - **Source of truth**: Implementation plan Task 3 specifies the twitter:image fallback chain.
- **Interactions**: None (pure function).

### Test 7: Scraper returns None when no image meta tags exist

- **Name**: No image URL extracted when page has no image meta tags
- **Type**: boundary
- **Harness**: Scraper unit test harness
- **Preconditions**: None.
- **Actions**:
  1. Parse HTML with `<head><title>No images</title></head>`.
  2. Execute the full fallback chain.
- **Expected outcome**:
  - Returns `None`.
  - **Source of truth**: When no meta tags match, the Option chain should exhaust all alternatives and return None.
- **Interactions**: None (pure function).

### Test 8: resolve_url handles absolute URLs

- **Name**: Absolute image URLs are returned unchanged
- **Type**: unit
- **Harness**: Scraper unit test harness
- **Preconditions**: None.
- **Actions**:
  1. Call `resolve_url("https://example.com/page", "https://cdn.example.com/img.jpg")`.
- **Expected outcome**:
  - Returns `"https://cdn.example.com/img.jpg"`.
  - **Source of truth**: Absolute URLs (starting with `http`) should be used as-is per standard URL resolution behavior.
- **Interactions**: None (pure function).

### Test 9: resolve_url resolves relative URLs against base

- **Name**: Relative image URLs are resolved against the page URL
- **Type**: unit
- **Harness**: Scraper unit test harness
- **Preconditions**: None.
- **Actions**:
  1. Call `resolve_url("https://example.com/page", "/img.jpg")`.
- **Expected outcome**:
  - Returns `"https://example.com/img.jpg"`.
  - **Source of truth**: Relative URLs should be resolved against the base URL per RFC 3986.
- **Interactions**: None (pure function).

### Test 10: og:image takes priority over twitter:image

- **Name**: og:image is preferred over twitter:image when both are present
- **Type**: invariant
- **Harness**: Scraper unit test harness
- **Preconditions**: None.
- **Actions**:
  1. Parse HTML containing both `<meta property="og:image" content="https://example.com/og.png">` and `<meta name="twitter:image" content="https://example.com/tw.png">`.
  2. Execute the full fallback chain.
- **Expected outcome**:
  - Returns `Some("https://example.com/og.png")` — the og:image, not the twitter:image.
  - **Source of truth**: The implementation plan's fallback chain checks og:image first: `select_meta("og:image")` is the first call, so it takes precedence.
- **Interactions**: None (pure function).

### Test 11: LocalStorage public_url uses /uploads prefix after fix

- **Name**: LocalStorage generates URLs with /uploads prefix, not /static/uploads
- **Type**: regression
- **Harness**: Scraper unit test harness (inline Rust test)
- **Preconditions**: None.
- **Actions**:
  1. Create a `LocalStorage` with `base_dir = "./uploads"` and `public_url_prefix = "http://localhost:4000/uploads"`.
  2. Call `public_url("images/abc.jpg")`.
- **Expected outcome**:
  - Returns `"http://localhost:4000/uploads/images/abc.jpg"`.
  - Does NOT return `"http://localhost:4000/static/uploads/images/abc.jpg"`.
  - **Source of truth**: Implementation plan Task 1 — the root cause of the bug is that `LocalStorage` was initialized with `/static/uploads` prefix, but files are stored in `./uploads` which is not under `./static`. The fix changes the prefix to `/uploads`.
- **Interactions**: None (pure function on `LocalStorage`).

---

## Coverage summary

### Covered areas

| Area | Tests |
|------|-------|
| Full user flow: add bookmark, see og:image in card | Test 1 |
| Image serving via /uploads route (storage path fix) | Test 2, Test 11 |
| Graceful degradation: no og:image shows placeholder | Test 3 |
| Scraper og:image extraction (property attribute) | Test 4 |
| Scraper fallback: name attribute | Test 5 |
| Scraper fallback: twitter:image | Test 6 |
| Scraper: no image meta tags | Test 7 |
| URL resolution (absolute and relative) | Tests 8, 9 |
| og:image priority over twitter:image | Test 10 |
| LocalStorage URL prefix correctness | Test 11 |

### Explicitly excluded per agreed strategy

| Area | Reason | Risk |
|------|--------|------|
| S3 storage backend | Strategy focuses on local storage backend; S3 path is unchanged by this fix | Low — the S3 path does not have the `/static/uploads` mismatch bug. |
| Image download failures (network errors, timeouts, invalid content-type) | Error handling paths exist but are not the focus of this bug fix | Medium — if a CDN blocks the request, the fallback is no image (placeholder shown). User-Agent header addition mitigates the most common case. |
| Google OAuth login flow | Cannot be automated without credentials; manual step in E2E tests | Low — OAuth is orthogonal to the og:image feature. |
| Concurrent bookmark creation | Not affected by this change | Low. |
| Image content validation (is the downloaded file actually an image?) | Not in scope of the bug fix | Low — the existing code stores whatever bytes are returned. |
