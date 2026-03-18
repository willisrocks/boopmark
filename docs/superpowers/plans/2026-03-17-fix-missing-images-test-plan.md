# Fix Missing Images — Test Plan

## Strategy Reconciliation

The agreed testing strategy holds against the implementation plan with no scope or cost changes. Minor refinements:

- The web UI uses `fetch() + ReadableStream` (not EventSource/HTMX SSE extension), so web UI tests verify the `fetch`-based streaming behavior.
- The E2E 409 concurrency test uses `page.evaluate` with `Promise.all` for reliable request overlap timing, since Playwright's `page.request.post()` without `await` does not guarantee overlap.
- The E2E server (started by `scripts/e2e/start-server.sh`) does not run the screenshot sidecar, does not set `SCREENSHOT_SERVICE_URL`, and uses `STORAGE_BACKEND=local`. Bookmarks without og:image will appear as `failed` in fix-images output — this is correct and expected.
- The `AuthUser` extractor handles both `Authorization: Bearer` (API key) and session cookie auth, so `POST /api/v1/bookmarks/fix-images` works for both. The web uses a separate `GET /settings/fix-images/stream` route for browser fetch compatibility.

---

## Harness Requirements

**No new harnesses need to be built.** The existing infrastructure covers all needs:

- **Playwright E2E harness**: `playwright.config.js` + `scripts/e2e/start-server.sh` launches a real server on `http://127.0.0.1:4010` with `ENABLE_E2E_AUTH=1`, real Postgres, and local storage.
- **Rust unit test harness**: `cargo test -p boopmark-server` runs in-process tests with `MockRepo`, `NoopMetadata`, and `NoopStorage` (patterns established in `server/src/app/bookmarks.rs` `mod import_tests`).
- **CLI E2E helper**: Reuse the `runBoop(args, apiKey)` pattern from `tests/e2e/cli-enrichment.spec.js`.
- **API key creation helper**: Reuse the `createApiKey(page, name)` pattern from `tests/e2e/api-enrichment.spec.js`.
- **Auth helper**: Mirror `signIn(page)` pattern from `tests/e2e/suggest.spec.js`.

---

## Test Cases

### 1. API SSE streams progress and completes with done:true

- **Type**: scenario
- **Disposition**: new
- **Harness**: Playwright E2E (`tests/e2e/fix-images.spec.js`)
- **Preconditions**: User signed in, API key created, at least one bookmark exists (with `image_url: null`).
- **Actions**:
  1. Sign in via E2E auth button.
  2. Create an API key via settings UI.
  3. Create a bookmark via `POST /api/v1/bookmarks` with bearer auth.
  4. Using `page.evaluate`, call `fetch('/api/v1/bookmarks/fix-images', { method: 'POST', headers: { 'Authorization': 'Bearer <key>', 'Accept': 'text/event-stream' } })` and read the response body as text.
  5. Assert response status is 200.
  6. Parse `data:` lines and check the last event.
- **Expected**: Last event has `done: true`, numeric `fixed`/`failed`/`checked`/`total`.

### 2. API returns 401 when unauthenticated

- **Type**: boundary
- **Disposition**: new
- **Harness**: Playwright E2E (`tests/e2e/fix-images.spec.js`)
- **Preconditions**: No authentication (bare `request` fixture, no cookies or bearer).
- **Actions**: `request.post('/api/v1/bookmarks/fix-images', { headers: { 'Accept': 'text/event-stream' } })`.
- **Expected**: Status 401.

### 3. Concurrent requests return 409 on the second

- **Type**: scenario
- **Disposition**: new
- **Harness**: Playwright E2E (`tests/e2e/fix-images.spec.js`)
- **Preconditions**: User signed in, API key, at least one bookmark.
- **Actions**: Using `page.evaluate`, fire two concurrent `fetch` POST requests via `Promise.all([fetch(...), fetch(...)])` with the same bearer token.
- **Expected**: Exactly one response with status 200 and one with status 409 (order may vary).

### 4. Settings page has Image Repair section

- **Type**: scenario
- **Disposition**: new
- **Harness**: Playwright E2E (`tests/e2e/fix-images.spec.js`)
- **Preconditions**: User signed in.
- **Actions**: Navigate to `/settings`. Check for heading, description, button, and hidden progress section.
- **Expected**: "Image Repair" heading visible, "Fix Missing Images" button visible and enabled, `#fix-images-progress` has class `hidden`.

### 5. Clicking Fix Missing Images shows progress bar and completes

- **Type**: scenario
- **Disposition**: new
- **Harness**: Playwright E2E (`tests/e2e/fix-images.spec.js`)
- **Preconditions**: User signed in.
- **Actions**:
  1. Navigate to `/settings`.
  2. Click "Fix Missing Images".
  3. Assert progress section visible, button disabled.
  4. Wait for `#fix-images-label` to contain "Done." (timeout 30s).
  5. Assert button re-enabled.
- **Expected**: Web UI SSE consumer parses events, updates progress bar, shows "Done. Fixed N images. M failed."

### 6. boop images fix completes with "Done" output

- **Type**: scenario
- **Disposition**: new
- **Harness**: Playwright E2E (`tests/e2e/fix-images.spec.js`) using `runBoop` pattern from `cli-enrichment.spec.js`
- **Preconditions**: CLI binary built, API key available.
- **Actions**: Call `runBoop('images fix', apiKey)` with E2E server URL.
- **Expected**: Output contains "Done.", "Fixed", "failed". Exit code 0.

### 7. boop images --help shows the fix subcommand

- **Type**: boundary
- **Disposition**: new
- **Harness**: Playwright E2E (`tests/e2e/fix-images.spec.js`)
- **Preconditions**: CLI binary built.
- **Actions**: `execSync('cargo run -p boop -- images --help')`.
- **Expected**: Output contains "fix" and "image".

### 8. fix_missing_images skips bookmarks with working images (unit)

- **Type**: unit
- **Disposition**: new
- **Harness**: Rust unit test (`server/src/app/bookmarks.rs`, `mod fix_images_tests`)
- **Preconditions**: `MockRepo` seeded with one bookmark whose `image_url` points to a local test server returning 200 on HEAD.
- **Expected**: Final event: `{ done: true, fixed: 0, failed: 0, checked: 1, total: 1 }`.

### 9. fix_missing_images records failure when no image and no screenshot service (unit)

- **Type**: unit
- **Disposition**: new
- **Harness**: Rust unit test
- **Preconditions**: `MockRepo` seeded with one bookmark, `image_url: None`. `NoopMetadata` returns no og:image. `screenshot_service_url = None`.
- **Expected**: Final event: `{ done: true, fixed: 0, failed: 1, checked: 1, total: 1 }`.

### 10. fix_missing_images handles empty bookmark list (unit)

- **Type**: unit
- **Disposition**: new
- **Harness**: Rust unit test
- **Preconditions**: Empty `MockRepo`.
- **Expected**: Single event: `{ checked: 0, total: 0, fixed: 0, failed: 0, done: true }`.

### 11. fix_missing_images fixes bookmark with broken image via og:image re-scrape (unit)

- **Type**: unit
- **Disposition**: new
- **Harness**: Rust unit test
- **Preconditions**: `MockRepo` seeded with bookmark whose `image_url` returns 404 on HEAD. Local test server serves HTML with og:image meta tag.
- **Expected**: Final event: `{ done: true, fixed: 1, failed: 0 }`.

### 12. ScreenshotClient.capture returns JPEG bytes on success (unit)

- **Type**: unit
- **Disposition**: new (already in implementation plan Task 3)
- **Harness**: Rust unit test (`server/src/adapters/screenshot.rs`)
- **Expected**: `Ok(bytes)` where `bytes[..2] == [0xFF, 0xD8]`.

### 13. ScreenshotClient.capture returns error on sidecar failure (unit)

- **Type**: unit
- **Disposition**: new (already in implementation plan Task 3)
- **Harness**: Rust unit test
- **Expected**: `Err(DomainError::Internal(...))` when sidecar returns 500.

### 14. Existing suggest E2E tests still pass after card template change (regression)

- **Type**: regression
- **Disposition**: existing
- **Harness**: `npx playwright test tests/e2e/suggest.spec.js`
- **Expected**: All assertions pass. The `h-40 → aspect-[40/21]` change does not break DOM structure or `data-testid` attributes.

### 15. Card template compiles with Askama (invariant)

- **Type**: invariant
- **Disposition**: existing
- **Harness**: `cargo build -p boopmark-server`
- **Expected**: No compile errors after template change.

### 16. Agent-browser: card visual appearance at correct aspect ratio (ad-hoc)

- **Type**: scenario
- **Disposition**: new
- **Harness**: Agent-browser ad-hoc (post-implementation, devproxy stack)
- **Actions**: Navigate to bookmark grid, take snapshot, verify ~1.91:1 aspect ratio.
- **Expected**: Cards show landscape image areas matching the 40:21 og:image standard ratio.

### 17. Agent-browser: og:image scraping produces real card images (ad-hoc)

- **Type**: scenario
- **Disposition**: new
- **Harness**: Agent-browser ad-hoc (devproxy stack)
- **Actions**: Add bookmark to a site with known og:image (e.g. github.com), run fix-images, verify card shows real image.
- **Expected**: Card displays og:image, not the placeholder emoji.

### 18. Agent-browser: screenshot sidecar produces real screenshots (ad-hoc)

- **Type**: scenario
- **Disposition**: new
- **Harness**: Agent-browser ad-hoc (devproxy stack with screenshot-svc running)
- **Actions**: Add bookmark to a site without og:image, run fix-images, verify card shows screenshot.
- **Expected**: Card displays screenshot, not placeholder.

### 19. Agent-browser: web UI progress bar animates and shows completion (ad-hoc)

- **Type**: scenario
- **Disposition**: new
- **Harness**: Agent-browser ad-hoc (devproxy stack)
- **Actions**: Navigate to settings, click button, take snapshot at partial progress and at completion.
- **Expected**: Snapshots show progress bar filling and "Done. Fixed X images. Y failed." message.

---

## Coverage Summary

**Covered:**
- API endpoint: SSE streaming, authentication, 409 dedup guard, JSON event format
- Web UI: settings page rendering, button interaction, progress bar, completion state
- CLI: subcommand registration, SSE stream consumption, progress output
- Service layer: progress event emission, valid-image skip, failure counting, empty-list edge case, broken-image detection
- Screenshot adapter: success and failure HTTP paths
- Card template: aspect ratio change regression safety, visual appearance

**Excluded (with rationale):**
- Screenshot sidecar unit/integration tests in CI: requires real Chromium, not available in test environments. Covered by agent-browser ad-hoc checks and `ScreenshotClient` adapter unit tests.
- Real external og:image scraping in CI: network isolation. Covered by agent-browser ad-hoc checks.
- Database-level `update_image_url` SQL correctness: verified implicitly through full-stack E2E tests against real Postgres.

**Risks:**
- Screenshot sidecar only verified via ad-hoc agent-browser checks, not automated CI. If the sidecar breaks (Playwright version drift, Dockerfile issues), it won't be caught until someone runs the check.
- Low risk on `update_image_url` SQL (simple statement, follows existing patterns).
