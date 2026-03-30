# Metadata Fallback — Test Plan

## Strategy Reconciliation

The agreed testing strategy covers eight verification points. This plan reconciles them with the 10-task implementation plan:

1. **Unit tests for each new adapter (IframelyExtractor, OpengraphIoExtractor, FallbackMetadataExtractor)** -- Covered by Task 3 (FallbackMetadataExtractor), Task 5 (IframelyExtractor), and Task 6 (OpengraphIoExtractor). Each adapter's tests are embedded in its source file.
2. **Unit tests for CF challenge detection (is_cloudflare_challenge)** -- Covered by Task 2. Four new tests in `server/src/adapters/scraper.rs` (later moved to `server/src/adapters/metadata/html.rs` in Task 3).
3. **Axum mock HTTP servers for adapter tests (not raw hyper)** -- Task 5 and Task 6 both use `axum::Router` + `tokio::net::TcpListener::bind("127.0.0.1:0")` for mock API servers, consistent with the existing pattern in `fix_images_tests`.
4. **Existing BookmarkService tests continue passing with mock MetadataExtractor** -- Task 1 Step 3 updates the three `impl MetadataExtractor` mocks in `bookmarks.rs` (two `NoopMetadata` and one `HtmlMetadata`) to use the new `Pin<Box<dyn Future>>` signature. Behavior is unchanged.
5. **CF challenge detection tested with sample challenge HTML** -- Task 2 includes four tests with realistic HTML samples: CF title match, CF verification text match, normal page negative, and body-text false-positive negative.
6. **Config parsing tests following existing pattern** -- Task 4 Step 2 adds three new `#[test]` functions in `config.rs::tests` matching the established pattern (inline `match` expression, no env var mutation).
7. **TDD approach** -- Plan specifies write-test-first, verify-fail, implement, verify-pass for Tasks 2, 3, 5, and 6. This test plan includes verification steps at each stage.
8. **No external API calls in tests** -- All third-party APIs are mocked with local axum servers bound to `127.0.0.1:0`.

### Scope refinements

- The implementation plan's Task 1 Step 3 lists three mock impls to update in `bookmarks.rs`. These are at: `import_tests::NoopMetadata` (line ~678), `fix_images_tests::NoopMetadata` (line ~1388), and `fix_images_tests::HtmlMetadata` (line ~1402). The `HtmlMetadata` mock specifically needs `self.image_url.clone()` captured before the async block because `Box::pin(async { ... })` does not capture `&self` by default -- the plan addresses this correctly.
- The existing `fix_images_tests` module already uses axum mock servers (`start_fake_site`, `start_fake_screenshot_svc`). This validates that the axum-based mock pattern is already proven in the codebase.
- The `is_cloudflare_challenge` function is a pure function taking `&str`, making it testable without any async machinery.
- The `FallbackMetadataExtractor` tests use inline `FailingExtractor` and `SuccessExtractor` structs that implement `MetadataExtractor` directly -- no HTTP server needed for the composite logic.

### Gaps identified in implementation plan

- **No explicit test for CF header detection**: Task 2 adds header-based CF detection (`cf-mitigated: challenge` response header) in Step 6 but only tests body-based detection with the `is_cloudflare_challenge` function. Header detection requires a mock HTTP server (since reqwest must parse the header). This is acceptable because: (a) the header check is a simple `resp.headers().get()` comparison, and (b) the `HtmlMetadataExtractor::extract` method as a whole is tested via `FallbackMetadataExtractor` integration. Adding a header-specific integration test would require spawning a mock server that returns the `cf-mitigated` header, which is valuable but not specified in the plan. This test plan adds one as an optional extension (Test 8).
- **No test for `BookmarkService::create` CF-blocked screenshot skip**: Task 8 modifies the `create` method to skip screenshots when CF is detected, but adds no test for this behavior. The `create` method is tested in `import_tests` (which use `NoopMetadata` returning `Ok`), but none exercise the CF-blocked path. This test plan adds one as a new test (Test 19).

---

## Test Cases

### 1. CF challenge detected by `<title>Just a moment...</title>`

- **Name**: `detects_cloudflare_challenge_by_title`
- **Type**: unit
- **Disposition**: new (Task 2 Step 2)
- **Harness**: `#[test]`
- **File**: `server/src/adapters/metadata/html.rs` (moved from `scraper.rs` in Task 3)
- **Preconditions**: None.
- **Action**: Call `is_cloudflare_challenge(html)` with HTML containing `<title>Just a moment...</title>`.
- **Assertion**: Returns `true`.

### 2. CF challenge detected by "Performing security verification" body text

- **Name**: `detects_cloudflare_challenge_by_verification_text`
- **Type**: unit
- **Disposition**: new (Task 2 Step 2)
- **Harness**: `#[test]`
- **File**: `server/src/adapters/metadata/html.rs`
- **Preconditions**: None.
- **Action**: Call `is_cloudflare_challenge(html)` with HTML containing `"Performing security verification"` in body but a different `<title>`.
- **Assertion**: Returns `true`.

### 3. Normal page is not flagged as CF challenge

- **Name**: `does_not_flag_normal_page_as_challenge`
- **Type**: boundary
- **Disposition**: new (Task 2 Step 2)
- **Harness**: `#[test]`
- **File**: `server/src/adapters/metadata/html.rs`
- **Preconditions**: None.
- **Action**: Call `is_cloudflare_challenge(html)` with normal HTML `<title>My Blog</title>`.
- **Assertion**: Returns `false`.

### 4. Page mentioning "moment" in body text is not a false positive

- **Name**: `does_not_flag_page_mentioning_moment_in_body`
- **Type**: boundary
- **Disposition**: new (Task 2 Step 2)
- **Harness**: `#[test]`
- **File**: `server/src/adapters/metadata/html.rs`
- **Preconditions**: None.
- **Action**: Call `is_cloudflare_challenge(html)` with body text `"Just a moment... let me explain."` but a normal `<title>Blog Post</title>`.
- **Assertion**: Returns `false`. The detection checks for `<title>Just a moment...</title>` as a tag, not substring in body.

### 5. FallbackMetadataExtractor falls back to second extractor on error

- **Name**: `falls_back_to_second_extractor_on_error`
- **Type**: unit
- **Disposition**: new (Task 3 Step 3)
- **Harness**: `#[tokio::test]`
- **File**: `server/src/adapters/metadata/fallback.rs`
- **Preconditions**: `FallbackMetadataExtractor` with `[FailingExtractor, SuccessExtractor]`.
- **Action**: Call `fallback.extract("https://example.com").await`.
- **Assertion**: `Ok` with `title == Some("Fallback Title")` and `image_url == Some("https://example.com/img.jpg")`.

### 6. FallbackMetadataExtractor returns first success without trying later extractors

- **Name**: `returns_first_success_without_trying_later`
- **Type**: unit
- **Disposition**: new (Task 3 Step 3)
- **Harness**: `#[tokio::test]`
- **File**: `server/src/adapters/metadata/fallback.rs`
- **Preconditions**: `FallbackMetadataExtractor` with `[SuccessExtractor("First"), SuccessExtractor("Second")]`.
- **Action**: Call `fallback.extract("https://example.com").await`.
- **Assertion**: `Ok` with `title == Some("First")`.

### 7. FallbackMetadataExtractor returns last error when all fail

- **Name**: `returns_last_error_when_all_fail`
- **Type**: unit
- **Disposition**: new (Task 3 Step 3)
- **Harness**: `#[tokio::test]`
- **File**: `server/src/adapters/metadata/fallback.rs`
- **Preconditions**: `FallbackMetadataExtractor` with `[FailingExtractor, FailingExtractor]`.
- **Action**: Call `fallback.extract("https://example.com").await`.
- **Assertion**: `result.is_err()`.

### 8. HtmlMetadataExtractor returns CF error on `cf-mitigated: challenge` header

- **Name**: `returns_cf_error_on_cf_mitigated_header`
- **Type**: integration
- **Disposition**: new (extension, not in implementation plan)
- **Harness**: `#[tokio::test]`
- **File**: `server/src/adapters/metadata/html.rs`
- **Preconditions**: Axum mock server at `127.0.0.1:0` returning `200 OK` with header `cf-mitigated: challenge` and a CF challenge body.
- **Action**: Call `HtmlMetadataExtractor::new().extract(mock_url).await`.
- **Assertion**: `Err` containing `CF_CHALLENGE_MSG` (`"blocked by Cloudflare challenge"`).

### 9. IframelyExtractor parses successful API response

- **Name**: `parses_iframely_response`
- **Type**: integration
- **Disposition**: new (Task 5 Step 1)
- **Harness**: `#[tokio::test]`
- **File**: `server/src/adapters/metadata/iframely.rs`
- **Preconditions**: Axum mock server at `127.0.0.1:0` with `GET /api/iframely` returning JSON with `meta.title`, `meta.description`, `links.thumbnail[0].href`.
- **Action**: Call `IframelyExtractor::with_base_url(key, mock_url).extract(target_url).await`.
- **Assertion**: `Ok` with `title == Some("Test Article")`, `description == Some("A test description")`, `image_url == Some("https://cdn.example.com/thumb.jpg")`.

### 10. IframelyExtractor returns error on API failure

- **Name**: `returns_error_on_api_failure` (iframely)
- **Type**: boundary
- **Disposition**: new (Task 5 Step 1)
- **Harness**: `#[tokio::test]`
- **File**: `server/src/adapters/metadata/iframely.rs`
- **Preconditions**: Axum mock server returning `403 Forbidden` at `GET /api/iframely`.
- **Action**: Call `IframelyExtractor::with_base_url(key, mock_url).extract(target_url).await`.
- **Assertion**: `result.is_err()`.

### 11. OpengraphIoExtractor parses successful API response

- **Name**: `parses_opengraph_io_response`
- **Type**: integration
- **Disposition**: new (Task 6 Step 2)
- **Harness**: `#[tokio::test]`
- **File**: `server/src/adapters/metadata/opengraph_io.rs`
- **Preconditions**: Axum mock server at `127.0.0.1:0` with `fallback(get(...))` returning JSON with `hybridGraph.title`, `hybridGraph.description`, `hybridGraph.image`. Uses `Router::new().fallback()` because the opengraph.io API encodes the target URL in the path, making static route matching impractical.
- **Action**: Call `OpengraphIoExtractor::with_base_url(key, mock_url).extract(target_url).await`.
- **Assertion**: `Ok` with `title == Some("OG Test")`, `description == Some("OG description")`, `image_url == Some("https://cdn.example.com/og.jpg")`.

### 12. OpengraphIoExtractor returns error on API failure

- **Name**: `returns_error_on_api_failure` (opengraph_io)
- **Type**: boundary
- **Disposition**: new (Task 6 Step 2)
- **Harness**: `#[tokio::test]`
- **File**: `server/src/adapters/metadata/opengraph_io.rs`
- **Preconditions**: Axum mock server returning `500 Internal Server Error` via `fallback(get(...))`.
- **Action**: Call `OpengraphIoExtractor::with_base_url(key, mock_url).extract(target_url).await`.
- **Assertion**: `result.is_err()`.

### 13. MetadataFallbackBackend config defaults to None

- **Name**: `metadata_fallback_backend_default_is_none`
- **Type**: unit
- **Disposition**: new (Task 4 Step 2)
- **Harness**: `#[test]`
- **File**: `server/src/config.rs`
- **Preconditions**: None.
- **Action**: Parse `"none"` through the `MetadataFallbackBackend` match expression.
- **Assertion**: `matches!(backend, MetadataFallbackBackend::None)`.

### 14. MetadataFallbackBackend config parses "iframely"

- **Name**: `metadata_fallback_backend_parses_iframely`
- **Type**: unit
- **Disposition**: new (Task 4 Step 2)
- **Harness**: `#[test]`
- **File**: `server/src/config.rs`
- **Preconditions**: None.
- **Action**: Parse `"iframely"` through the `MetadataFallbackBackend` match expression.
- **Assertion**: `matches!(backend, MetadataFallbackBackend::Iframely)`.

### 15. MetadataFallbackBackend config parses "opengraph_io"

- **Name**: `metadata_fallback_backend_parses_opengraph_io`
- **Type**: unit
- **Disposition**: new (Task 4 Step 2)
- **Harness**: `#[test]`
- **File**: `server/src/config.rs`
- **Preconditions**: None.
- **Action**: Parse `"opengraph_io"` through the `MetadataFallbackBackend` match expression.
- **Assertion**: `matches!(backend, MetadataFallbackBackend::OpengraphIo)`.

### 16. Existing scraper tests pass after module move

- **Name**: `extracts_og_image_from_property`, `falls_back_to_name_attribute`, `falls_back_to_twitter_image`, `no_image_meta_returns_none`, `resolve_url_handles_absolute`, `resolve_url_handles_relative`, `og_image_takes_priority_over_twitter_image`, `resolve_url_handles_data_uri`
- **Type**: regression
- **Disposition**: existing (moved from `scraper.rs` to `metadata/html.rs` in Task 3)
- **Harness**: `#[test]`
- **File**: `server/src/adapters/metadata/html.rs`
- **Preconditions**: Module move completed.
- **Action**: `cargo test -p boopmark-server html::tests`
- **Assertion**: All 8 existing tests pass unchanged.

### 17. Existing import_tests pass with updated MetadataExtractor mock signature

- **Name**: All 15+ tests in `import_tests` module
- **Type**: regression
- **Disposition**: extend (Task 1 Step 3 -- signature change only)
- **Harness**: `#[tokio::test]`
- **File**: `server/src/app/bookmarks.rs`, `mod import_tests`
- **Preconditions**: `NoopMetadata` impl updated from `async fn extract(...)` to `fn extract(...) -> Pin<Box<dyn Future<...>>>` with `Box::pin(async { ... })` wrapper.
- **Action**: `cargo test -p boopmark-server import_tests`
- **Assertion**: All tests pass unchanged. Behavior is identical since the mock returns the same `Ok(UrlMetadata { ... })` value.

### 18. Existing fix_images_tests pass with updated MetadataExtractor mock signatures

- **Name**: `empty_bookmark_list_emits_single_done_event`, `skips_bookmarks_with_valid_images`, `records_failure_when_no_image_and_no_screenshot_svc`, `fixes_bookmark_with_broken_image_via_og_image`, `fixes_bookmark_via_screenshot_fallback`
- **Type**: regression
- **Disposition**: extend (Task 1 Step 3 -- signature change to both `NoopMetadata` and `HtmlMetadata`)
- **Harness**: `#[tokio::test]`
- **File**: `server/src/app/bookmarks.rs`, `mod fix_images_tests`
- **Preconditions**: Both `NoopMetadata` and `HtmlMetadata` impls updated to `Pin<Box<dyn Future>>` signature. `HtmlMetadata` specifically captures `self.image_url.clone()` before the async block.
- **Action**: `cargo test -p boopmark-server fix_images_tests`
- **Assertion**: All 5 tests pass unchanged.

### 19. BookmarkService::create skips screenshot when CF challenge detected

- **Name**: `create_skips_screenshot_on_cf_challenge`
- **Type**: unit
- **Disposition**: new (gap in implementation plan -- Task 8 modifies `create` but adds no test)
- **Harness**: `#[tokio::test]`
- **File**: `server/src/app/bookmarks.rs`, `mod import_tests` (or new submodule)
- **Preconditions**: A `CfBlockedMetadata` mock that returns `Err(DomainError::Internal(CF_CHALLENGE_MSG.to_string()))`. A mock `ScreenshotProvider` that tracks whether `capture` was called (e.g., via `AtomicBool`).
- **Action**: Call `svc.create(user_id, CreateBookmark { url, title: None, ... }).await`.
- **Assertion**: The returned bookmark has `image_url == None`. The screenshot mock's `capture` was never called (tracked via the `AtomicBool`).

### 20. BookmarkService helper tests pass unchanged

- **Name**: `needs_metadata_when_image_or_domain_is_missing`, `merge_metadata_preserves_user_text_but_returns_missing_image`
- **Type**: regression
- **Disposition**: existing (no changes needed)
- **Harness**: `#[test]`
- **File**: `server/src/app/bookmarks.rs`, `mod tests`
- **Preconditions**: None.
- **Action**: `cargo test -p boopmark-server bookmarks::tests`
- **Assertion**: Both pass unchanged. These test pure functions with no dependency on `MetadataExtractor`.

### 21. CF_CHALLENGE_MSG constant exists and is used consistently

- **Name**: (compile-time verification)
- **Type**: invariant
- **Disposition**: new (Task 2 Step 1)
- **Harness**: Compile check (`cargo build`)
- **File**: `server/src/domain/error.rs`
- **Preconditions**: Constant added.
- **Action**: `cargo build -p boopmark-server` -- any misuse of the constant (typo, wrong import) fails compilation.
- **Assertion**: Build succeeds. Both `server/src/adapters/metadata/html.rs` and `server/src/app/bookmarks.rs` import and use `CF_CHALLENGE_MSG`.

### 22. Full test suite passes after all changes

- **Name**: (full regression)
- **Type**: regression
- **Disposition**: existing
- **Harness**: `cargo test`
- **File**: All test files in workspace
- **Preconditions**: All 9 implementation tasks completed.
- **Action**: `cargo test -p boopmark-server`
- **Assertion**: All tests pass (zero failures).

### 23. Clippy passes with no warnings

- **Name**: (lint verification)
- **Type**: invariant
- **Disposition**: existing
- **Harness**: `cargo clippy`
- **File**: All source files
- **Preconditions**: All implementation tasks completed.
- **Action**: `cargo clippy -p boopmark-server -- -D warnings`
- **Assertion**: Zero warnings, zero errors.

### 24. Code formatting is clean

- **Name**: (format verification)
- **Type**: invariant
- **Disposition**: existing
- **Harness**: `cargo fmt`
- **File**: All source files
- **Preconditions**: All implementation tasks completed.
- **Action**: `cargo fmt -- --check`
- **Assertion**: No formatting issues.

---

## Test Execution Order

Tests are designed to be run at each implementation checkpoint:

| Checkpoint | Command | Expected |
|---|---|---|
| After Task 1 (dyn-compatible trait) | `cargo test -p boopmark-server` | All existing tests pass |
| After Task 2 (CF detection) | `cargo test -p boopmark-server -- cloudflare` | 4 new tests pass |
| After Task 3 (module reorg + fallback) | `cargo test -p boopmark-server` | All tests pass including 3 new fallback tests |
| After Task 4 (config) | `cargo test -p boopmark-server config::tests` | 3 new config tests pass |
| After Task 5 (iframely) | `cargo test -p boopmark-server iframely::tests` | 2 new tests pass |
| After Task 6 (opengraph_io) | `cargo test -p boopmark-server opengraph_io::tests` | 2 new tests pass |
| After Task 8 (CF screenshot skip) | `cargo test -p boopmark-server` | All pass |
| After Task 10 (final) | `cargo test -p boopmark-server` | All pass |
| After Task 10 (final) | `cargo clippy -p boopmark-server -- -D warnings` | Zero warnings |
| After Task 10 (final) | `cargo fmt -- --check` | Clean |

---

## Summary

| Category | Count |
|---|---|
| New tests | 15 (Tests 1-15, 19) |
| Extended/modified existing tests | 20+ (Tests 17, 18 -- signature updates across all import_tests and fix_images_tests) |
| Unchanged existing tests | 10 (Tests 16, 20) |
| Compile/lint checks | 3 (Tests 21, 23, 24) |
| Full regression | 1 (Test 22) |
| **Total distinct test cases** | **24** |
