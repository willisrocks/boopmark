# Open Source Readiness — Test Plan

## Strategy Reconciliation

The agreed testing strategy covers six verification points. This plan reconciles them with the 13-task implementation plan:

1. **Unit tests for new adapters (PlaywrightScreenshot, NoopScreenshot)** — Covered by Task 2 Steps 5-6. Tests are embedded in the new adapter files and verified in Task 4 Step 7.
2. **Update existing BookmarkService tests to use NoopScreenshot** — Task 5 handles the `import_tests` module. Additionally, the `fix_images_tests` module (not mentioned in the implementation plan) contains 5 tests that also construct `BookmarkService` with `screenshot_service_url: Option<String>` — these must also be updated.
3. **Full `cargo test` suite passes** — Verified at Task 4 Step 7 and Task 5 Step 3.
4. **`cargo clippy` passes with no warnings** — Not explicitly mentioned in the plan. Added as a verification step after the final Rust changes (Task 5).
5. **Docker build succeeds (validates CSS stage + hash_password binary)** — Task 8 Step 4.
6. **`docker compose config` validates** — Task 7 Step 4.

### Scope refinements

- The implementation plan mentions updating `make_service` and `make_service_with_failing_upsert` in the `import_tests` module (Task 5). However, the `fix_images_tests` module at line 1228 has **five** tests that directly construct `BookmarkService::new(...)` with `None` or `Some(screenshot_url)` as the 4th argument. These must also be updated to pass `Arc<dyn ScreenshotProvider>`. This is a gap in the implementation plan.
- The `fixes_bookmark_via_screenshot_fallback` test (line 1635) currently passes `Some(screenshot_url)` to construct a `BookmarkService` with a real HTTP-based screenshot sidecar. After the refactor, this test must pass `Arc::new(PlaywrightScreenshot::new(screenshot_url))` instead. The `start_fake_screenshot_svc()` helper remains valid.
- `cargo clippy` is added as an explicit verification since it can catch issues like unused imports after the `ScreenshotClient` removal.

---

## Test Cases

### 1. PlaywrightScreenshot::capture returns JPEG bytes on success

- **Type**: unit
- **Disposition**: new (Task 2 Step 5)
- **File**: `server/src/adapters/screenshot/playwright.rs`
- **Test name**: `capture_returns_bytes_from_sidecar`
- **Preconditions**: Fake Axum server on random port returning minimal JPEG at `POST /screenshot`.
- **Expected**: `Ok(bytes)` where `bytes[..2] == [0xFF, 0xD8]`.
- **Verification**: `cargo test -p boopmark-server capture_returns_bytes_from_sidecar`

### 2. PlaywrightScreenshot::capture returns error on sidecar failure

- **Type**: unit
- **Disposition**: new (Task 2 Step 5)
- **File**: `server/src/adapters/screenshot/playwright.rs`
- **Test name**: `capture_returns_error_on_sidecar_failure`
- **Preconditions**: Fake Axum server returning 500 at `POST /screenshot`.
- **Expected**: `Err(...)`.
- **Verification**: `cargo test -p boopmark-server capture_returns_error_on_sidecar_failure`

### 3. NoopScreenshot::capture returns error

- **Type**: unit
- **Disposition**: new (Task 2 Step 6)
- **File**: `server/src/adapters/screenshot/noop.rs`
- **Test name**: `noop_capture_returns_error`
- **Preconditions**: None.
- **Expected**: `Err(DomainError::Internal("screenshots are disabled"))`.
- **Verification**: `cargo test -p boopmark-server noop_capture_returns_error`

### 4. import_tests: make_service uses NoopScreenshot

- **Type**: existing (updated)
- **Disposition**: modified (Task 5 Step 2)
- **File**: `server/src/app/bookmarks.rs`, `mod import_tests`
- **What changes**:
  - Add `use crate::adapters::screenshot::noop::NoopScreenshot;` to `import_tests` module
  - `make_service()` (line 720): change 4th arg from `None` to `Arc::new(NoopScreenshot)`
  - `make_service_with_failing_upsert()` (line 731): change 4th arg from `None` to `Arc::new(NoopScreenshot)`
- **Affected tests** (all 21 tests in the `import_tests` module):
  - `import_creates_new_bookmark`
  - `import_skips_existing_url_when_strategy_is_skip`
  - `import_upserts_existing_url_when_strategy_is_upsert`
  - `import_records_error_for_invalid_url`
  - `restore_creates_new_bookmark_with_original_id`
  - `restore_records_error_when_id_is_missing`
  - `restore_upserts_existing_bookmark`
  - `restore_creates_via_insert_with_id`
  - (and remaining tests in the module)
- **Expected**: All pass unchanged (behavior is identical since NoopScreenshot returns Err, same as `None` did for screenshot logic).
- **Verification**: `cargo test -p boopmark-server import_tests`

### 5. fix_images_tests: empty_bookmark_list_emits_single_done_event

- **Type**: existing (updated)
- **Disposition**: modified (gap in implementation plan — must be done alongside Task 5)
- **File**: `server/src/app/bookmarks.rs`, `mod fix_images_tests`, line 1520
- **What changes**: Replace `None` (4th arg to `BookmarkService::new`) with `Arc::new(NoopScreenshot)`.
- **Import needed**: Add `use crate::adapters::screenshot::noop::NoopScreenshot;` to `fix_images_tests` module.
- **Expected**: Passes unchanged.
- **Verification**: `cargo test -p boopmark-server empty_bookmark_list`

### 6. fix_images_tests: skips_bookmarks_with_valid_images

- **Type**: existing (updated)
- **Disposition**: modified
- **File**: `server/src/app/bookmarks.rs`, `mod fix_images_tests`, line 1541
- **What changes**: Replace `None` (4th arg) with `Arc::new(NoopScreenshot)`.
- **Expected**: Passes unchanged.
- **Verification**: `cargo test -p boopmark-server skips_bookmarks_with_valid_images`

### 7. fix_images_tests: records_failure_when_no_image_and_no_screenshot_svc

- **Type**: existing (updated)
- **Disposition**: modified
- **File**: `server/src/app/bookmarks.rs`, `mod fix_images_tests`, line 1565
- **What changes**: Replace `None` (4th arg) with `Arc::new(NoopScreenshot)`.
- **Expected**: Passes unchanged (NoopScreenshot returns Err, same effective behavior as `None` since `fetch_and_store_image` now calls `self.screenshot.capture()` which returns Err).
- **Verification**: `cargo test -p boopmark-server records_failure_when_no_image_and_no_screenshot_svc`

### 8. fix_images_tests: fixes_bookmark_with_broken_image_via_og_image

- **Type**: existing (updated)
- **Disposition**: modified
- **File**: `server/src/app/bookmarks.rs`, `mod fix_images_tests`, line 1585
- **What changes**: Replace `None` (4th arg) with `Arc::new(NoopScreenshot)`.
- **Expected**: Passes unchanged (this test uses og:image path, not screenshot fallback).
- **Verification**: `cargo test -p boopmark-server fixes_bookmark_with_broken_image_via_og_image`

### 9. fix_images_tests: fixes_bookmark_via_screenshot_fallback

- **Type**: existing (updated)
- **Disposition**: modified (significant change)
- **File**: `server/src/app/bookmarks.rs`, `mod fix_images_tests`, line 1635
- **What changes**:
  - Add `use crate::adapters::screenshot::playwright::PlaywrightScreenshot;` to the module
  - Replace `Some(screenshot_url)` (4th arg) with `Arc::new(PlaywrightScreenshot::new(screenshot_url))`
  - The `start_fake_screenshot_svc()` helper (line 1499) remains as-is
- **Expected**: Passes unchanged (PlaywrightScreenshot adapter calls the same endpoint the old ScreenshotClient did).
- **Verification**: `cargo test -p boopmark-server fixes_bookmark_via_screenshot_fallback`

### 10. Full cargo test suite

- **Type**: regression
- **Disposition**: existing
- **Verification**: `cargo test`
- **Expected**: All tests pass (server + CLI crates). No test should break from the other tasks (config changes, Dockerfile, justfile, etc.) since those are not exercised by unit tests.

### 11. Cargo clippy

- **Type**: lint
- **Disposition**: new verification step
- **Verification**: `cargo clippy -- -D warnings`
- **Expected**: No warnings. Specifically checks for:
  - No unused imports after `ScreenshotClient` removal
  - No dead code from removed `screenshot_service_url` field paths
  - Correct trait object usage for `Arc<dyn ScreenshotProvider>`

### 12. Docker compose config validation

- **Type**: infrastructure
- **Disposition**: new (Task 7 Step 4)
- **File**: `docker-compose.yml`
- **Verification**: `docker compose config --quiet`
- **Expected**: No errors. Validates that commented-out services and removed dependencies parse correctly.

### 13. Docker build succeeds

- **Type**: infrastructure
- **Disposition**: new (Task 8 Step 4)
- **File**: `Dockerfile`
- **Verification**: `docker compose build server`
- **Expected**: Build succeeds. Validates:
  - CSS build stage (Node 24) produces `static/css/output.css`
  - `hash_password` example binary compiles
  - Final image copies both artifacts correctly

### 14. Justfile syntax validation

- **Type**: infrastructure
- **Disposition**: new (Task 11 Step 4)
- **File**: `justfile`
- **Verification**: `just --list`
- **Expected**: Lists all commands including `bootstrap`, `dev`, `setup` without syntax errors.

---

## Existing Tests That Will Break During Implementation

### Tests that break after Task 2 (screenshot module restructure) and before Task 3-4 (wiring)

The crate will not compile between Task 2 and the end of Task 4 because:
- `server/src/adapters/screenshot.rs` is deleted and replaced with `server/src/adapters/screenshot/mod.rs`
- `bookmarks.rs` still references `crate::adapters::screenshot::ScreenshotClient`

**Resolution**: Tasks 2, 3, and 4 are committed together. No intermediate test run is possible or expected.

### Tests that break after Task 3 (BookmarkService refactor) and before Task 5 (test updates)

After Task 3 changes `BookmarkService::new()` to accept `Arc<dyn ScreenshotProvider>` instead of `Option<String>`, all tests that construct `BookmarkService` will fail to compile:

**`import_tests` module** (2 helper functions, ~21 tests affected):
- `make_service()` at line 720 — passes `None`
- `make_service_with_failing_upsert()` at line 731 — passes `None`

**`fix_images_tests` module** (5 direct constructor calls, 5 tests affected):
- `empty_bookmark_list_emits_single_done_event` at line 1523 — passes `None`
- `skips_bookmarks_with_valid_images` at line 1549 — passes `None`
- `records_failure_when_no_image_and_no_screenshot_svc` at line 1570 — passes `None`
- `fixes_bookmark_with_broken_image_via_og_image` at line 1618 — passes `None`
- `fixes_bookmark_via_screenshot_fallback` at line 1643 — passes `Some(screenshot_url)`

**Resolution**: Task 5 updates all of these. The `import_tests` constructors change to `Arc::new(NoopScreenshot)`. The `fix_images_tests` constructors change to `Arc::new(NoopScreenshot)` except for `fixes_bookmark_via_screenshot_fallback` which changes to `Arc::new(PlaywrightScreenshot::new(screenshot_url))`.

---

## Verification Command Summary

| Step | Command | When to run |
|------|---------|-------------|
| After Task 1 | `cargo check -p boopmark-server` | Port trait compiles |
| After Task 4 (includes 2+3) | `cargo check -p boopmark-server` | Full wiring compiles (tests may not) |
| After Task 5 | `cargo test -p boopmark-server` | All server tests pass |
| After Task 5 | `cargo clippy -- -D warnings` | No lint warnings |
| After Task 6 | `cargo check -p boopmark-server` | Config change compiles |
| After Task 7 | `docker compose config --quiet` | Docker Compose valid |
| After Task 8 | `docker compose build server` | Docker image builds |
| After Task 9 | `cargo check` | License fields valid |
| After Task 11 | `just --list` | Justfile parses |
| Final | `cargo test` | Full workspace tests pass |
| Final | `cargo clippy -- -D warnings` | No warnings |
