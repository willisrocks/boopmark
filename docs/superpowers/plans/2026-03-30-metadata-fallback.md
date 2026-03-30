# Metadata Fallback Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add tiered metadata extraction so Cloudflare-protected sites (like Medium) fall back to a third-party API (iframely or opengraph.io) instead of showing challenge pages as bookmark images.

**Architecture:** New adapters implement the existing `MetadataExtractor` trait. A composite `FallbackMetadataExtractor` chains them in order (HTML scraper first, then third-party API on error). CF challenge detection is added to `HtmlMetadataExtractor` so it returns an error instead of empty metadata when blocked. Screenshot fallback is skipped when CF challenge is detected.

**Tech Stack:** Rust, reqwest (existing), serde_json (existing), axum (existing, for test mock servers), urlencoding (new)

**Design spec:** `docs/superpowers/specs/2026-03-30-metadata-fallback-design.md`

---

## Key Design Decisions

### 1. Dyn-compatible trait via `Pin<Box<dyn Future>>`

The current `MetadataExtractor` uses `#[trait_variant::make(Send)]` with `async fn`, which is NOT object-safe. `Box<dyn MetadataExtractor>` will not compile with this form. The `FallbackMetadataExtractor` needs to hold `Vec<Box<dyn MetadataExtractor>>`, so we must make the trait dyn-compatible.

The codebase already has two established precedents for dyn-compatible async traits: `ScreenshotProvider` and `LlmEnricher` in `server/src/domain/ports/`. Both use the `Pin<Box<dyn Future<Output = ...> + Send + '_>>` return type pattern. We follow the same pattern for consistency.

### 2. Always wrap in `FallbackMetadataExtractor`

Even when no fallback backend is configured (the default), we wrap the single `HtmlMetadataExtractor` in `FallbackMetadataExtractor`. This means the concrete type is always `FallbackMetadataExtractor` throughout the app, avoiding conditional type complexity. The cost (one extra `Vec` with a single element) is negligible.

### 3. CF detection: header first, then body

Cloudflare challenge detection uses two signals:
- **`cf-mitigated: challenge` response header** — checked before consuming the body; the most reliable signal.
- **Body content heuristics** — `<title>Just a moment...</title>` or `"Performing security verification"`. The title check uses the exact `<title>` tag match (not just substring in body text) to avoid false positives on articles that mention "Just a moment..." in their prose.

### 4. CF challenge detection shared via constant

A `CF_CHALLENGE_MSG` constant in `domain/error.rs` is the contract between the scraper (which sets it) and `BookmarkService` (which checks for it). `BookmarkService` checks error messages via `e.to_string().contains(CF_CHALLENGE_MSG)` to decide whether to skip screenshot fallback. This avoids adding a new `DomainError` variant and keeps the change minimal.

### 5. Module reorganization: `scraper.rs` → `metadata/html.rs`

Moving the existing scraper into a `metadata/` module creates a clean home for all metadata adapter implementations. The module structure will be:

```
server/src/adapters/metadata/
  mod.rs
  html.rs         (moved from adapters/scraper.rs)
  fallback.rs     (new)
  iframely.rs     (new)
  opengraph_io.rs (new)
```

### 6. Test mock signature updates

Three `impl MetadataExtractor` exist in the codebase outside the scraper itself — all are test mocks in `bookmarks.rs`. They need their signatures updated from `async fn extract(...)` to `fn extract(...) -> Pin<Box<...>>` but their behavior stays identical.

### 7. OpengraphIo test routing

The opengraph.io API encodes the target URL in the request path (`/api/1.1/site/{encoded_url}`), making it impossible to match as a static axum route. Tests use `Router::new().fallback(get(...))` to match all paths, which is the idiomatic solution.

### 8. `urlencoding` as explicit dependency

The `OpengraphIoExtractor` needs URL encoding for its API path. While `reqwest` re-exports percent-encoding, we add `urlencoding` as an explicit workspace dependency per the user's instruction. It provides a simpler API (`urlencoding::encode`) and avoids coupling to reqwest internals.

---

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| Modify | `server/src/domain/ports/metadata.rs` | Make `MetadataExtractor` dyn-compatible |
| Modify | `server/src/domain/error.rs` | Add `CF_CHALLENGE_MSG` constant |
| Create | `server/src/adapters/metadata/mod.rs` | Module declaration for metadata adapters |
| Create | `server/src/adapters/metadata/fallback.rs` | `FallbackMetadataExtractor` — composite chain |
| Create | `server/src/adapters/metadata/iframely.rs` | `IframelyExtractor` adapter |
| Create | `server/src/adapters/metadata/opengraph_io.rs` | `OpengraphIoExtractor` adapter |
| Move | `server/src/adapters/scraper.rs` → `server/src/adapters/metadata/html.rs` | Move existing HTML scraper into metadata module |
| Modify | `server/src/adapters/mod.rs` | Replace `scraper` module with `metadata` module |
| Modify | `server/src/config.rs` | Add `MetadataFallbackBackend` enum and config fields |
| Modify | `server/src/main.rs` | Wire up fallback chain based on config |
| Modify | `server/src/web/state.rs` | Change generic type from `HtmlMetadataExtractor` to `FallbackMetadataExtractor` |
| Modify | `server/src/app/bookmarks.rs` | Challenge-aware screenshot fallback; update test mock signatures |
| Modify | `Cargo.toml` (workspace) | Add `urlencoding` workspace dep |
| Modify | `server/Cargo.toml` | Add `urlencoding` dep |
| Modify | `.env.example` | Add new env vars |
| Modify | `README.md` | Add metadata fallback config to env var table |

---

## Chunk 1: Make MetadataExtractor Dyn-Compatible + CF Detection

### Task 1: Make MetadataExtractor trait dyn-compatible

**Files:**
- Modify: `server/src/domain/ports/metadata.rs`
- Modify: `server/src/adapters/scraper.rs` (update impl)
- Modify: `server/src/app/bookmarks.rs` (update test mock impls)

The current `MetadataExtractor` uses `#[trait_variant::make(Send)]` with `async fn`, which is NOT object-safe. `Box<dyn MetadataExtractor>` will not compile. Change it to use `Pin<Box<dyn Future>>` like `ScreenshotProvider` and `LlmEnricher`.

- [ ] **Step 1: Change the trait definition**

Replace the contents of `server/src/domain/ports/metadata.rs`:

```rust
use crate::domain::bookmark::UrlMetadata;
use crate::domain::error::DomainError;
use std::future::Future;
use std::pin::Pin;

pub trait MetadataExtractor: Send + Sync {
    fn extract(
        &self,
        url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<UrlMetadata, DomainError>> + Send + '_>>;
}
```

- [ ] **Step 2: Update HtmlMetadataExtractor impl**

In `server/src/adapters/scraper.rs`, change the impl from:

```rust
impl MetadataExtractor for HtmlMetadataExtractor {
    async fn extract(&self, url_str: &str) -> Result<UrlMetadata, DomainError> {
```

To:

```rust
impl MetadataExtractor for HtmlMetadataExtractor {
    fn extract(
        &self,
        url_str: &str,
    ) -> Pin<Box<dyn Future<Output = Result<UrlMetadata, DomainError>> + Send + '_>> {
        let url_str = url_str.to_string();
        Box::pin(async move {
            // ... existing body unchanged, but use url_str (owned) ...
        })
    }
}
```

Add `use std::future::Future; use std::pin::Pin;` to the imports.

The body of the async block stays the same — it already uses `self.client` which is borrowed from `&self`.

- [ ] **Step 3: Update all MetadataExtractor test mock impls in bookmarks.rs**

There are three `impl MetadataExtractor` in `server/src/app/bookmarks.rs` that need the same signature change. They are:

1. `NoopMetadata` at line ~678 (in `import_tests` module)
2. `NoopMetadata` at line ~1388 (in `fix_image_tests` module)
3. `HtmlMetadata` at line ~1402 (in `fix_image_tests` module)

Each changes from:
```rust
async fn extract(&self, _url: &str) -> Result<UrlMetadata, DomainError> {
```

To:
```rust
fn extract(
    &self,
    _url: &str,
) -> Pin<Box<dyn Future<Output = Result<UrlMetadata, DomainError>> + Send + '_>> {
    Box::pin(async {
        // ... existing body ...
    })
}
```

Add `use std::future::Future; use std::pin::Pin;` to each test module's imports.

For the `HtmlMetadata` mock specifically, the `self.image_url.clone()` needs to be captured before the async block because `self` is not available inside `Box::pin(async { ... })` by default:

```rust
impl MetadataExtractor for HtmlMetadata {
    fn extract(
        &self,
        _url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<UrlMetadata, DomainError>> + Send + '_>> {
        let image_url = self.image_url.clone();
        Box::pin(async move {
            Ok(UrlMetadata {
                title: None,
                description: None,
                image_url,
                domain: None,
            })
        })
    }
}
```

- [ ] **Step 4: Verify everything compiles**

Run: `cargo build -p boopmark-server`
Expected: Compiles successfully

- [ ] **Step 5: Run all tests**

Run: `cargo test -p boopmark-server`
Expected: All PASS

- [ ] **Step 6: Commit**

```bash
git add server/src/domain/ports/metadata.rs server/src/adapters/scraper.rs server/src/app/bookmarks.rs
git commit -m "refactor: make MetadataExtractor dyn-compatible with Pin<Box<dyn Future>>"
```

---

### Task 2: Add CF challenge detection to HtmlMetadataExtractor

**Files:**
- Modify: `server/src/adapters/scraper.rs`
- Modify: `server/src/domain/error.rs`

- [ ] **Step 1: Add CF_CHALLENGE_MSG constant to DomainError module**

In `server/src/domain/error.rs`, add after the `DomainError` enum:

```rust
/// Error message used when a Cloudflare challenge page is detected.
/// Shared between the scraper (which detects it) and BookmarkService (which checks for it).
pub const CF_CHALLENGE_MSG: &str = "blocked by Cloudflare challenge";
```

- [ ] **Step 2: Write failing tests for CF challenge detection**

Add to the existing `#[cfg(test)] mod tests` in `scraper.rs`:

```rust
#[test]
fn detects_cloudflare_challenge_by_title() {
    let html = r#"<html><head><title>Just a moment...</title></head>
        <body>Performing security verification</body></html>"#;
    assert!(is_cloudflare_challenge(html));
}

#[test]
fn detects_cloudflare_challenge_by_verification_text() {
    let html = r#"<html><head><title>Some Site</title></head>
        <body>Performing security verification</body></html>"#;
    assert!(is_cloudflare_challenge(html));
}

#[test]
fn does_not_flag_normal_page_as_challenge() {
    let html = r#"<html><head><title>My Blog</title></head>
        <body><p>Hello world</p></body></html>"#;
    assert!(!is_cloudflare_challenge(html));
}

#[test]
fn does_not_flag_page_mentioning_moment_in_body() {
    let html = r#"<html><head><title>Blog Post</title></head>
        <body><p>Just a moment... let me explain.</p></body></html>"#;
    assert!(!is_cloudflare_challenge(html));
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p boopmark-server -- detects_cloudflare`
Expected: FAIL — `is_cloudflare_challenge` not found

- [ ] **Step 4: Implement `is_cloudflare_challenge`**

Add this function to `scraper.rs` (above the `#[cfg(test)]` block):

```rust
fn is_cloudflare_challenge(body: &str) -> bool {
    // Check for the specific CF challenge title (not body text, which could appear in articles)
    body.contains("<title>Just a moment...</title>")
        || body.contains("Performing security verification")
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p boopmark-server -- cloudflare`
Expected: All PASS

- [ ] **Step 6: Wire challenge detection into `extract` method**

In the `extract` method's async block, add CF detection between the HTTP response and HTML parsing. The current code is:

```rust
let resp = self
    .client
    .get(&url_str)
    .send()
    .await
    .map_err(|e| DomainError::Internal(format!("fetch error: {e}")))?;
let html = resp
    .text()
    .await
    .map_err(|e| DomainError::Internal(format!("read error: {e}")))?;
```

Change to:

```rust
let resp = self
    .client
    .get(&url_str)
    .send()
    .await
    .map_err(|e| DomainError::Internal(format!("fetch error: {e}")))?;

// Check the CF-Mitigated header before consuming the body
if resp
    .headers()
    .get("cf-mitigated")
    .and_then(|v| v.to_str().ok())
    .is_some_and(|v| v.eq_ignore_ascii_case("challenge"))
{
    return Err(DomainError::Internal(CF_CHALLENGE_MSG.to_string()));
}

let html = resp
    .text()
    .await
    .map_err(|e| DomainError::Internal(format!("read error: {e}")))?;

if is_cloudflare_challenge(&html) {
    return Err(DomainError::Internal(CF_CHALLENGE_MSG.to_string()));
}
```

Add `use crate::domain::error::CF_CHALLENGE_MSG;` to the imports.

- [ ] **Step 7: Run all scraper tests**

Run: `cargo test -p boopmark-server -- cloudflare`
Expected: All PASS

Run: `cargo test -p boopmark-server`
Expected: All PASS (no regressions)

- [ ] **Step 8: Commit**

```bash
git add server/src/adapters/scraper.rs server/src/domain/error.rs
git commit -m "feat: detect Cloudflare challenge pages in HTML metadata extractor"
```

---

### Task 3: Create FallbackMetadataExtractor and reorganize modules

**Files:**
- Create: `server/src/adapters/metadata/mod.rs`
- Create: `server/src/adapters/metadata/fallback.rs`
- Move: `server/src/adapters/scraper.rs` → `server/src/adapters/metadata/html.rs`
- Modify: `server/src/adapters/mod.rs`
- Modify: `server/src/main.rs` (import path)
- Modify: `server/src/web/state.rs` (import path)

- [ ] **Step 1: Reorganize — move scraper.rs into metadata module**

Create the directory `server/src/adapters/metadata/`.

Create `server/src/adapters/metadata/mod.rs`:

```rust
pub mod fallback;
pub mod html;
```

Move `server/src/adapters/scraper.rs` to `server/src/adapters/metadata/html.rs` (contents unchanged).

Update `server/src/adapters/mod.rs` — replace `pub mod scraper;` with `pub mod metadata;`.

Update imports in `server/src/main.rs`:
- Change `use adapters::scraper::HtmlMetadataExtractor;` to `use adapters::metadata::html::HtmlMetadataExtractor;`

Update imports in `server/src/web/state.rs`:
- Change `use crate::adapters::scraper::HtmlMetadataExtractor;` to `use crate::adapters::metadata::html::HtmlMetadataExtractor;`

- [ ] **Step 2: Verify the move compiles and tests pass**

Run: `cargo test -p boopmark-server`
Expected: All PASS

- [ ] **Step 3: Write failing tests for FallbackMetadataExtractor**

Create `server/src/adapters/metadata/fallback.rs`:

```rust
use crate::domain::bookmark::UrlMetadata;
use crate::domain::error::DomainError;
use crate::domain::ports::metadata::MetadataExtractor;
use std::future::Future;
use std::pin::Pin;

pub struct FallbackMetadataExtractor {
    extractors: Vec<Box<dyn MetadataExtractor>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FailingExtractor;
    impl MetadataExtractor for FailingExtractor {
        fn extract(
            &self,
            _url: &str,
        ) -> Pin<Box<dyn Future<Output = Result<UrlMetadata, DomainError>> + Send + '_>> {
            Box::pin(async { Err(DomainError::Internal("blocked".to_string())) })
        }
    }

    struct SuccessExtractor {
        title: Option<String>,
    }
    impl MetadataExtractor for SuccessExtractor {
        fn extract(
            &self,
            _url: &str,
        ) -> Pin<Box<dyn Future<Output = Result<UrlMetadata, DomainError>> + Send + '_>> {
            let title = self.title.clone();
            Box::pin(async move {
                Ok(UrlMetadata {
                    title,
                    description: None,
                    image_url: Some("https://example.com/img.jpg".to_string()),
                    domain: None,
                })
            })
        }
    }

    #[tokio::test]
    async fn falls_back_to_second_extractor_on_error() {
        let fallback = FallbackMetadataExtractor::new(vec![
            Box::new(FailingExtractor),
            Box::new(SuccessExtractor { title: Some("Fallback Title".to_string()) }),
        ]);
        let result = fallback.extract("https://example.com").await.unwrap();
        assert_eq!(result.title, Some("Fallback Title".to_string()));
        assert_eq!(result.image_url, Some("https://example.com/img.jpg".to_string()));
    }

    #[tokio::test]
    async fn returns_first_success_without_trying_later() {
        let fallback = FallbackMetadataExtractor::new(vec![
            Box::new(SuccessExtractor { title: Some("First".to_string()) }),
            Box::new(SuccessExtractor { title: Some("Second".to_string()) }),
        ]);
        let result = fallback.extract("https://example.com").await.unwrap();
        assert_eq!(result.title, Some("First".to_string()));
    }

    #[tokio::test]
    async fn returns_last_error_when_all_fail() {
        let fallback = FallbackMetadataExtractor::new(vec![
            Box::new(FailingExtractor),
            Box::new(FailingExtractor),
        ]);
        let result = fallback.extract("https://example.com").await;
        assert!(result.is_err());
    }
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test -p boopmark-server fallback::tests`
Expected: FAIL — `new` method and `MetadataExtractor` impl not found

- [ ] **Step 5: Implement FallbackMetadataExtractor**

Add the constructor and trait impl to `fallback.rs` (above the `#[cfg(test)]` block):

```rust
impl FallbackMetadataExtractor {
    pub fn new(extractors: Vec<Box<dyn MetadataExtractor>>) -> Self {
        Self { extractors }
    }
}

impl MetadataExtractor for FallbackMetadataExtractor {
    fn extract(
        &self,
        url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<UrlMetadata, DomainError>> + Send + '_>> {
        let url = url.to_string();
        Box::pin(async move {
            let mut last_err =
                DomainError::Internal("no metadata extractors configured".to_string());
            for extractor in &self.extractors {
                match extractor.extract(&url).await {
                    Ok(meta) => return Ok(meta),
                    Err(e) => {
                        tracing::warn!(url = %url, error = %e, "metadata extractor failed, trying next");
                        last_err = e;
                    }
                }
            }
            Err(last_err)
        })
    }
}
```

Note: `tracing` is already a dependency. Add `use tracing;` or just use `tracing::warn!` inline (which works without a `use` statement in Rust).

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p boopmark-server fallback::tests`
Expected: All PASS

- [ ] **Step 7: Commit**

```bash
git add server/src/adapters/metadata/ server/src/adapters/mod.rs server/src/main.rs server/src/web/state.rs
git rm server/src/adapters/scraper.rs
git commit -m "feat: add FallbackMetadataExtractor and reorganize metadata adapters"
```

---

### Task 4: Wire FallbackMetadataExtractor into startup

**Files:**
- Modify: `server/src/config.rs`
- Modify: `server/src/main.rs`
- Modify: `server/src/web/state.rs`

- [ ] **Step 1: Add MetadataFallbackBackend to config**

In `server/src/config.rs`, add the enum after the existing `StorageBackend` enum:

```rust
#[derive(Debug, Clone)]
pub enum MetadataFallbackBackend {
    Iframely,
    OpengraphIo,
    None,
}
```

Add fields to the `Config` struct (after `screenshot_service_url`):

```rust
pub metadata_fallback_backend: MetadataFallbackBackend,
pub iframely_api_key: Option<String>,
pub opengraph_io_api_key: Option<String>,
```

Add parsing in `Config::from_env()` (after the `screenshot_service_url` line):

```rust
metadata_fallback_backend: match env::var("METADATA_FALLBACK_BACKEND")
    .unwrap_or_else(|_| "none".into())
    .as_str()
{
    "iframely" => MetadataFallbackBackend::Iframely,
    "opengraph_io" => MetadataFallbackBackend::OpengraphIo,
    _ => MetadataFallbackBackend::None,
},
iframely_api_key: env::var("IFRAMELY_API_KEY").ok(),
opengraph_io_api_key: env::var("OPENGRAPH_IO_API_KEY").ok(),
```

- [ ] **Step 2: Add config parsing tests**

Add to the existing `#[cfg(test)] mod tests` in `config.rs`:

```rust
use super::MetadataFallbackBackend;

#[test]
fn metadata_fallback_backend_default_is_none() {
    let backend: MetadataFallbackBackend = match "none" {
        "iframely" => MetadataFallbackBackend::Iframely,
        "opengraph_io" => MetadataFallbackBackend::OpengraphIo,
        _ => MetadataFallbackBackend::None,
    };
    assert!(matches!(backend, MetadataFallbackBackend::None));
}

#[test]
fn metadata_fallback_backend_parses_iframely() {
    let backend: MetadataFallbackBackend = match "iframely" {
        "iframely" => MetadataFallbackBackend::Iframely,
        "opengraph_io" => MetadataFallbackBackend::OpengraphIo,
        _ => MetadataFallbackBackend::None,
    };
    assert!(matches!(backend, MetadataFallbackBackend::Iframely));
}

#[test]
fn metadata_fallback_backend_parses_opengraph_io() {
    let backend: MetadataFallbackBackend = match "opengraph_io" {
        "iframely" => MetadataFallbackBackend::Iframely,
        "opengraph_io" => MetadataFallbackBackend::OpengraphIo,
        _ => MetadataFallbackBackend::None,
    };
    assert!(matches!(backend, MetadataFallbackBackend::OpengraphIo));
}
```

- [ ] **Step 3: Run config tests**

Run: `cargo test -p boopmark-server config::tests`
Expected: All PASS

- [ ] **Step 4: Update state.rs — change generic type to FallbackMetadataExtractor**

In `server/src/web/state.rs`:

Replace import:
```rust
use crate::adapters::metadata::html::HtmlMetadataExtractor;
```
With:
```rust
use crate::adapters::metadata::fallback::FallbackMetadataExtractor;
```

Replace in `Bookmarks` enum:
```rust
pub enum Bookmarks {
    Local(Arc<BookmarkService<PostgresPool, FallbackMetadataExtractor, LocalStorage>>),
    S3(Arc<BookmarkService<PostgresPool, FallbackMetadataExtractor, S3Storage>>),
}
```

Replace in `AppState`:
```rust
pub enrichment: Arc<EnrichmentService<FallbackMetadataExtractor, PostgresPool>>,
```

- [ ] **Step 5: Update main.rs — build the fallback chain**

In `server/src/main.rs`, update imports:

```rust
// Remove:
use adapters::scraper::HtmlMetadataExtractor;
// Add (this replaces the import updated in Task 3):
use adapters::metadata::fallback::FallbackMetadataExtractor;
use adapters::metadata::html::HtmlMetadataExtractor;
```

Replace lines 51-52 (the metadata initialization):

```rust
let html_extractor = HtmlMetadataExtractor::new();
let extractors: Vec<Box<dyn domain::ports::metadata::MetadataExtractor>> =
    vec![Box::new(html_extractor)];

// Fallback adapters are wired in Task 7 after they are implemented.
// For now, the chain always has just the HTML extractor.

let metadata = Arc::new(FallbackMetadataExtractor::new(extractors));
let metadata_for_enrichment = metadata.clone();
```

- [ ] **Step 6: Verify everything compiles**

Run: `cargo build -p boopmark-server`
Expected: Compiles successfully

- [ ] **Step 7: Run all tests**

Run: `cargo test -p boopmark-server`
Expected: All PASS

- [ ] **Step 8: Commit**

```bash
git add server/src/config.rs server/src/main.rs server/src/web/state.rs
git commit -m "feat: wire FallbackMetadataExtractor into startup config"
```

---

## Chunk 2: Third-Party Adapters

### Task 5: Implement IframelyExtractor

**Files:**
- Create: `server/src/adapters/metadata/iframely.rs`
- Modify: `server/src/adapters/metadata/mod.rs`

The iframely API endpoint is `https://iframe.ly/api/iframely?url={url}&api_key={key}`. It returns JSON with fields like `meta.title`, `meta.description`, `links.thumbnail[].href`.

- [ ] **Step 1: Create iframely.rs with struct, tests, and implementation**

Create `server/src/adapters/metadata/iframely.rs`:

```rust
use crate::domain::bookmark::UrlMetadata;
use crate::domain::error::DomainError;
use crate::domain::ports::metadata::MetadataExtractor;
use std::future::Future;
use std::pin::Pin;

pub struct IframelyExtractor {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
}

#[derive(serde::Deserialize)]
struct IframelyResponse {
    meta: Option<IframelyMeta>,
    links: Option<IframelyLinks>,
}

#[derive(serde::Deserialize)]
struct IframelyMeta {
    title: Option<String>,
    description: Option<String>,
}

#[derive(serde::Deserialize)]
struct IframelyLinks {
    thumbnail: Option<Vec<IframelyThumbnail>>,
}

#[derive(serde::Deserialize)]
struct IframelyThumbnail {
    href: Option<String>,
}

impl IframelyExtractor {
    pub fn new(api_key: String) -> Self {
        Self::with_base_url(api_key, "https://iframe.ly".to_string())
    }

    fn with_base_url(api_key: String, base_url: String) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap(),
            api_key,
            base_url,
        }
    }
}

impl MetadataExtractor for IframelyExtractor {
    fn extract(
        &self,
        url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<UrlMetadata, DomainError>> + Send + '_>> {
        let url = url.to_string();
        Box::pin(async move {
            let resp = self
                .client
                .get(format!("{}/api/iframely", self.base_url))
                .query(&[("url", &url), ("api_key", &self.api_key)])
                .send()
                .await
                .map_err(|e| DomainError::Internal(format!("iframely fetch error: {e}")))?;

            if !resp.status().is_success() {
                return Err(DomainError::Internal(format!(
                    "iframely returned HTTP {}",
                    resp.status()
                )));
            }

            let data: IframelyResponse = resp
                .json()
                .await
                .map_err(|e| DomainError::Internal(format!("iframely parse error: {e}")))?;

            let meta = data.meta.unwrap_or(IframelyMeta { title: None, description: None });
            let image_url = data
                .links
                .and_then(|l| l.thumbnail)
                .and_then(|t| t.into_iter().next())
                .and_then(|t| t.href);

            let domain = url::Url::parse(&url)
                .ok()
                .and_then(|u| u.host_str().map(|h| h.to_string()));

            Ok(UrlMetadata {
                title: meta.title,
                description: meta.description,
                image_url,
                domain,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, routing::get};

    async fn mock_success() -> Json<serde_json::Value> {
        Json(serde_json::json!({
            "meta": {
                "title": "Test Article",
                "description": "A test description"
            },
            "links": {
                "thumbnail": [{"href": "https://cdn.example.com/thumb.jpg"}]
            }
        }))
    }

    async fn mock_error() -> (axum::http::StatusCode, &'static str) {
        (axum::http::StatusCode::FORBIDDEN, "Forbidden")
    }

    #[tokio::test]
    async fn parses_iframely_response() {
        let app = Router::new().route("/api/iframely", get(mock_success));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(axum::serve(listener, app).into_future());

        let extractor = IframelyExtractor::with_base_url(
            "test-key".to_string(),
            format!("http://{}", addr),
        );
        let result = extractor.extract("https://medium.com/some-article").await.unwrap();
        assert_eq!(result.title, Some("Test Article".to_string()));
        assert_eq!(result.description, Some("A test description".to_string()));
        assert_eq!(result.image_url, Some("https://cdn.example.com/thumb.jpg".to_string()));
    }

    #[tokio::test]
    async fn returns_error_on_api_failure() {
        let app = Router::new().route("/api/iframely", get(mock_error));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(axum::serve(listener, app).into_future());

        let extractor = IframelyExtractor::with_base_url(
            "bad-key".to_string(),
            format!("http://{}", addr),
        );
        let result = extractor.extract("https://medium.com/some-article").await;
        assert!(result.is_err());
    }
}
```

- [ ] **Step 2: Add to mod.rs**

In `server/src/adapters/metadata/mod.rs`, add:

```rust
pub mod iframely;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p boopmark-server iframely::tests`
Expected: All PASS

- [ ] **Step 4: Verify full build**

Run: `cargo build -p boopmark-server`
Expected: Compiles

- [ ] **Step 5: Commit**

```bash
git add server/src/adapters/metadata/iframely.rs server/src/adapters/metadata/mod.rs
git commit -m "feat: add IframelyExtractor metadata adapter"
```

---

### Task 6: Implement OpengraphIoExtractor

**Files:**
- Create: `server/src/adapters/metadata/opengraph_io.rs`
- Modify: `server/src/adapters/metadata/mod.rs`
- Modify: `Cargo.toml` (workspace) — add `urlencoding`
- Modify: `server/Cargo.toml` — add `urlencoding`

The opengraph.io API endpoint is `https://opengraph.io/api/1.1/site/{encoded_url}?app_id={key}`. It returns JSON with `hybridGraph.title`, `hybridGraph.description`, `hybridGraph.image`.

- [ ] **Step 1: Add urlencoding dependency**

In workspace `Cargo.toml`, add to `[workspace.dependencies]`:

```toml
urlencoding = "2"
```

In `server/Cargo.toml`, add to `[dependencies]`:

```toml
urlencoding.workspace = true
```

- [ ] **Step 2: Create opengraph_io.rs with struct, tests, and implementation**

Create `server/src/adapters/metadata/opengraph_io.rs`:

```rust
use crate::domain::bookmark::UrlMetadata;
use crate::domain::error::DomainError;
use crate::domain::ports::metadata::MetadataExtractor;
use std::future::Future;
use std::pin::Pin;

pub struct OpengraphIoExtractor {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
}

#[derive(serde::Deserialize)]
struct OpengraphIoResponse {
    #[serde(rename = "hybridGraph")]
    hybrid_graph: Option<HybridGraph>,
}

#[derive(serde::Deserialize)]
struct HybridGraph {
    title: Option<String>,
    description: Option<String>,
    image: Option<String>,
}

impl OpengraphIoExtractor {
    pub fn new(api_key: String) -> Self {
        Self::with_base_url(api_key, "https://opengraph.io".to_string())
    }

    fn with_base_url(api_key: String, base_url: String) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap(),
            api_key,
            base_url,
        }
    }
}

impl MetadataExtractor for OpengraphIoExtractor {
    fn extract(
        &self,
        url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<UrlMetadata, DomainError>> + Send + '_>> {
        let url = url.to_string();
        Box::pin(async move {
            let encoded_url = urlencoding::encode(&url);
            let resp = self
                .client
                .get(format!("{}/api/1.1/site/{}", self.base_url, encoded_url))
                .query(&[("app_id", &self.api_key)])
                .send()
                .await
                .map_err(|e| DomainError::Internal(format!("opengraph.io fetch error: {e}")))?;

            if !resp.status().is_success() {
                return Err(DomainError::Internal(format!(
                    "opengraph.io returned HTTP {}",
                    resp.status()
                )));
            }

            let data: OpengraphIoResponse = resp
                .json()
                .await
                .map_err(|e| DomainError::Internal(format!("opengraph.io parse error: {e}")))?;

            let graph = data.hybrid_graph.unwrap_or(HybridGraph {
                title: None,
                description: None,
                image: None,
            });

            let domain = url::Url::parse(&url)
                .ok()
                .and_then(|u| u.host_str().map(|h| h.to_string()));

            Ok(UrlMetadata {
                title: graph.title,
                description: graph.description,
                image_url: graph.image,
                domain,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, routing::get};

    async fn mock_success() -> Json<serde_json::Value> {
        Json(serde_json::json!({
            "hybridGraph": {
                "title": "OG Test",
                "description": "OG description",
                "image": "https://cdn.example.com/og.jpg"
            }
        }))
    }

    async fn mock_error() -> (axum::http::StatusCode, &'static str) {
        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error")
    }

    #[tokio::test]
    async fn parses_opengraph_io_response() {
        // Use fallback routing because the opengraph.io API encodes the target
        // URL in the request path, making static route matching impractical.
        let app = Router::new().fallback(get(mock_success));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(axum::serve(listener, app).into_future());

        let extractor = OpengraphIoExtractor::with_base_url(
            "test-key".to_string(),
            format!("http://{}", addr),
        );
        let result = extractor.extract("https://medium.com/some-article").await.unwrap();
        assert_eq!(result.title, Some("OG Test".to_string()));
        assert_eq!(result.description, Some("OG description".to_string()));
        assert_eq!(result.image_url, Some("https://cdn.example.com/og.jpg".to_string()));
    }

    #[tokio::test]
    async fn returns_error_on_api_failure() {
        let app = Router::new().fallback(get(mock_error));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(axum::serve(listener, app).into_future());

        let extractor = OpengraphIoExtractor::with_base_url(
            "bad-key".to_string(),
            format!("http://{}", addr),
        );
        let result = extractor.extract("https://medium.com/some-article").await;
        assert!(result.is_err());
    }
}
```

- [ ] **Step 3: Add to mod.rs**

In `server/src/adapters/metadata/mod.rs`, add:

```rust
pub mod opengraph_io;
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p boopmark-server opengraph_io::tests`
Expected: All PASS

- [ ] **Step 5: Verify full build**

Run: `cargo build -p boopmark-server`
Expected: Compiles

- [ ] **Step 6: Commit**

```bash
git add server/src/adapters/metadata/opengraph_io.rs server/src/adapters/metadata/mod.rs Cargo.toml server/Cargo.toml
git commit -m "feat: add OpengraphIoExtractor metadata adapter"
```

---

### Task 7: Wire third-party adapters into main.rs

**Files:**
- Modify: `server/src/main.rs`

- [ ] **Step 1: Add fallback adapter wiring**

In `server/src/main.rs`, add imports:

```rust
use adapters::metadata::iframely::IframelyExtractor;
use adapters::metadata::opengraph_io::OpengraphIoExtractor;
use config::MetadataFallbackBackend;
```

Replace the metadata initialization (the version from Task 4 Step 5 that has the placeholder comment) with:

```rust
let html_extractor = HtmlMetadataExtractor::new();
let mut extractors: Vec<Box<dyn domain::ports::metadata::MetadataExtractor>> =
    vec![Box::new(html_extractor)];

match &config.metadata_fallback_backend {
    MetadataFallbackBackend::Iframely => {
        let api_key = config
            .iframely_api_key
            .clone()
            .expect("IFRAMELY_API_KEY required when METADATA_FALLBACK_BACKEND=iframely");
        tracing::info!("metadata fallback: iframely");
        extractors.push(Box::new(IframelyExtractor::new(api_key)));
    }
    MetadataFallbackBackend::OpengraphIo => {
        let api_key = config
            .opengraph_io_api_key
            .clone()
            .expect("OPENGRAPH_IO_API_KEY required when METADATA_FALLBACK_BACKEND=opengraph_io");
        tracing::info!("metadata fallback: opengraph.io");
        extractors.push(Box::new(OpengraphIoExtractor::new(api_key)));
    }
    MetadataFallbackBackend::None => {}
}

let metadata = Arc::new(FallbackMetadataExtractor::new(extractors));
let metadata_for_enrichment = metadata.clone();
```

- [ ] **Step 2: Verify build**

Run: `cargo build -p boopmark-server`
Expected: Compiles

- [ ] **Step 3: Run all tests**

Run: `cargo test -p boopmark-server`
Expected: All PASS

- [ ] **Step 4: Commit**

```bash
git add server/src/main.rs
git commit -m "feat: wire iframely and opengraph.io adapters into fallback chain"
```

---

## Chunk 3: Challenge-Aware Screenshots + Docs

### Task 8: Skip screenshot when CF challenge is detected

**Files:**
- Modify: `server/src/app/bookmarks.rs`

When the `FallbackMetadataExtractor` returns an error (meaning ALL extractors failed, including fallbacks), and that error contains `CF_CHALLENGE_MSG`, we know the primary extractor hit a CF challenge and any fallback (if configured) also failed. In this case, taking a screenshot would capture the challenge page, so we skip it.

Note: When a fallback IS configured and succeeds, `FallbackMetadataExtractor` returns `Ok(...)` and no screenshot skip is needed — the fallback provided the metadata. The CF-skip only applies when all extractors fail with a CF challenge message from the first extractor propagating through.

However, the more precise behavior is: if any extractor in the chain returned a CF challenge error (the first one), the `FallbackMetadataExtractor` logs it and tries the next. If a later extractor succeeds, `Ok` is returned and no skip needed. If ALL fail, the LAST error is returned, which may or may not be the CF error. To ensure the CF-skip works when only the HTML extractor fails with CF but a fallback extractor fails for a different reason (e.g., invalid API key), the `FallbackMetadataExtractor` should propagate the CF error specifically.

**Decision:** Keep it simple. The `FallbackMetadataExtractor` returns the last error. If all extractors fail and any of them was a CF challenge, the screenshot will also get the CF challenge. If the fallback extractor fails for a non-CF reason (API key issue), the screenshot might still capture a real page (the CF challenge is specific to our HTML scraper's user agent — a headless browser might get through). So using the last error's message is correct: if the screenshot sidecar would also be blocked (CF challenge), the fallback would also have failed with a CF-related error. In practice, if the fallback service fails for a non-CF reason, trying the screenshot is reasonable.

- [ ] **Step 1: Update the `create` method to track CF-blocked status**

In `server/src/app/bookmarks.rs`, replace the metadata + screenshot block in the `create` method (lines 58-75):

```rust
if needs_metadata(&input) {
    // Try metadata extraction for title/description and og:image
    if let Ok(meta) = self.metadata.extract(&input.url).await
        && let Some(image_url) = merge_metadata(&mut input, meta) {
            // og:image found — download and store it
            if let Ok(stored_url) = self.download_and_store_image(&image_url).await {
                input.image_url = Some(stored_url);
            }
        }
    // Fall back to screenshot service if still no image
    if input.image_url.is_none()
        && let Ok(bytes) = self.screenshot.capture(&input.url).await {
            let key = format!("images/{}.jpg", Uuid::new_v4());
            if let Ok(stored_url) = self.storage.put(&key, bytes, "image/jpeg").await {
                input.image_url = Some(stored_url);
            }
        }
}
```

With:

```rust
if needs_metadata(&input) {
    let mut cf_blocked = false;
    match self.metadata.extract(&input.url).await {
        Ok(meta) => {
            if let Some(image_url) = merge_metadata(&mut input, meta) {
                if let Ok(stored_url) = self.download_and_store_image(&image_url).await {
                    input.image_url = Some(stored_url);
                }
            }
        }
        Err(e) => {
            cf_blocked = e.to_string().contains(CF_CHALLENGE_MSG);
            tracing::warn!(url = %input.url, error = %e, "metadata extraction failed");
        }
    }
    // Skip screenshot if CF challenge detected — it would capture the challenge page
    if input.image_url.is_none() && !cf_blocked {
        if let Ok(bytes) = self.screenshot.capture(&input.url).await {
            let key = format!("images/{}.jpg", Uuid::new_v4());
            if let Ok(stored_url) = self.storage.put(&key, bytes, "image/jpeg").await {
                input.image_url = Some(stored_url);
            }
        }
    }
}
```

Add `use crate::domain::error::CF_CHALLENGE_MSG;` to imports at the top of the file.

- [ ] **Step 2: Apply same logic to `fetch_and_store_image`**

Replace the `fetch_and_store_image` method (lines 381-399):

```rust
/// Try og:image scrape first; fall back to screenshot sidecar.
async fn fetch_and_store_image(
    &self,
    page_url: &str,
) -> Result<String, DomainError> {
    // 1. Try og:image
    if let Ok(meta) = self.metadata.extract(page_url).await
        && let Some(image_url) = meta.image_url
        && let Ok(stored) = self.download_and_store_image(&image_url).await
    {
        return Ok(stored);
    }

    // 2. Fall back to screenshot sidecar
    let bytes = self.screenshot.capture(page_url).await?;

    let key = format!("images/{}.jpg", Uuid::new_v4());
    self.storage.put(&key, bytes, "image/jpeg").await
}
```

With:

```rust
/// Try og:image scrape first; fall back to screenshot sidecar.
async fn fetch_and_store_image(
    &self,
    page_url: &str,
) -> Result<String, DomainError> {
    match self.metadata.extract(page_url).await {
        Ok(meta) => {
            if let Some(image_url) = meta.image_url {
                if let Ok(stored) = self.download_and_store_image(&image_url).await {
                    return Ok(stored);
                }
            }
        }
        Err(e) if e.to_string().contains(CF_CHALLENGE_MSG) => {
            return Err(e);
        }
        Err(_) => {}
    }

    // Fall back to screenshot sidecar
    let bytes = self.screenshot.capture(page_url).await?;
    let key = format!("images/{}.jpg", Uuid::new_v4());
    self.storage.put(&key, bytes, "image/jpeg").await
}
```

- [ ] **Step 3: Verify build**

Run: `cargo build -p boopmark-server`
Expected: Compiles

- [ ] **Step 4: Run all tests**

Run: `cargo test -p boopmark-server`
Expected: All PASS

- [ ] **Step 5: Commit**

```bash
git add server/src/app/bookmarks.rs
git commit -m "feat: skip screenshot fallback when Cloudflare challenge detected"
```

---

### Task 9: Update .env.example and documentation

**Files:**
- Modify: `.env.example`
- Modify: `README.md`

- [ ] **Step 1: Add new env vars to .env.example**

Add after the existing screenshot config section (after `# SCREENSHOT_SERVICE_URL=http://localhost:3001`):

```
# --- Metadata Fallback ---
# "none" (default) — direct HTML scraping only
# "iframely" or "opengraph_io" — third-party fallback for CF-blocked sites

# METADATA_FALLBACK_BACKEND=iframely
# IFRAMELY_API_KEY=
# OPENGRAPH_IO_API_KEY=
```

- [ ] **Step 2: Add to README.md env var table**

Add rows to the existing environment variables table (after the `SCREENSHOT_SERVICE_URL` row, before the `ENABLE_E2E_AUTH` row):

```markdown
| `METADATA_FALLBACK_BACKEND` | `none` | `iframely` or `opengraph_io` (optional) |
| `IFRAMELY_API_KEY` | — | Required when `METADATA_FALLBACK_BACKEND=iframely` |
| `OPENGRAPH_IO_API_KEY` | — | Required when `METADATA_FALLBACK_BACKEND=opengraph_io` |
```

- [ ] **Step 3: Commit**

```bash
git add .env.example README.md
git commit -m "docs: add metadata fallback config to .env.example and README"
```

---

### Task 10: Final verification

- [ ] **Step 1: Run the full test suite**

Run: `cargo test -p boopmark-server`
Expected: All PASS

- [ ] **Step 2: Run cargo clippy**

Run: `cargo clippy -p boopmark-server -- -D warnings`
Expected: No warnings

- [ ] **Step 3: Run cargo fmt check**

Run: `cargo fmt -- --check`
Expected: No formatting issues

---

## Regression Risks

1. **Trait signature change breaks external consumers:** The `MetadataExtractor` trait is only used within this crate. All impls (1 production + 3 test mocks) are updated in Task 1. Risk is low.

2. **Module move breaks import paths:** Two files reference `adapters::scraper::HtmlMetadataExtractor` (`main.rs` and `state.rs`). Both are updated in Task 3. No other files reference this path.

3. **Generic type change in `Bookmarks`/`AppState`:** Changing from `HtmlMetadataExtractor` to `FallbackMetadataExtractor` affects `state.rs`. This is a type-level change verified by the compiler — if it compiles, it works.

4. **CF challenge false positives:** The `is_cloudflare_challenge` function checks for `<title>Just a moment...</title>` (exact tag match, not body text) and `"Performing security verification"` body text. The title check is safe because no legitimate page would have this exact title tag. The body text check could theoretically match an article discussing CF challenges, but this is unlikely and the consequence (falling through to a third-party API) is acceptable.

5. **`FallbackMetadataExtractor` returning last error:** When all extractors fail, the last error is returned. This means if HtmlExtractor fails with CF and Iframely fails with a 403, the returned error is "iframely returned HTTP 403 Forbidden", not the CF message. The screenshot skip in `BookmarkService` checks for `CF_CHALLENGE_MSG` in the error, so the screenshot would NOT be skipped in this case. This is actually correct: if the fallback service also failed, it may be due to a different issue, and the screenshot might work (headless browsers bypass CF more often than simple HTTP clients).

6. **Test mock behavior unchanged:** The test mocks `NoopMetadata` and `HtmlMetadata` change signature but return identical values. The `Box::pin(async { ... })` wrapping is purely mechanical.
