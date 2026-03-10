# Store Google Avatar Locally — Test Plan

## Harness Requirements

**Harness: Playwright MCP (agent-browser)**

- **What it does:** Drives a real browser against the local dev stack (`http://localhost:4000`) for E2E verification.
- **What it exposes:** Page navigation, DOM inspection (element attributes, `src` values), network request monitoring, screenshot capture.
- **Estimated complexity:** Zero build effort — Playwright MCP is already available and configured.
- **Which tests depend on it:** Tests 1, 2, 3, 4.

**Harness: Human-in-the-loop Google OAuth**

- **What it does:** The user manually completes the Google OAuth consent flow (entering credentials, clicking "Allow") because automated Google login is not feasible without violating Google's ToS or maintaining fragile credential automation.
- **What it exposes:** A completed OAuth flow that triggers the avatar download-and-store code path.
- **Estimated complexity:** Zero build effort — requires a pause/resume in the E2E flow.
- **Which tests depend on it:** Tests 1, 2, 3.

No new test harnesses need to be built. No automated test suite additions are needed per the agreed strategy.

---

## Test Plan

### Test 1: Google login stores avatar in images bucket instead of hotlinking Google (Scenario)

- **Name:** After Google OAuth login, user avatar is served from local/S3 images bucket, not from lh3.googleusercontent.com
- **Type:** scenario
- **Harness:** Playwright MCP + human-in-the-loop
- **Preconditions:**
  - Local dev stack is running (`docker compose up`, `cargo run -p boopmark-server`)
  - `STORAGE_BACKEND` is set (either `local` or `s3`)
  - User is not logged in (no active session)
- **Actions:**
  1. Navigate to `http://localhost:4000`
  2. Take screenshot of login page (pre-login state)
  3. Click "Sign in with Google" button
  4. **PAUSE** — human completes Google OAuth consent flow
  5. After redirect back to app, take screenshot of the logged-in state showing the avatar in the header
  6. Inspect the `<img>` element inside `[data-testid="profile-menu-trigger"]` — read its `src` attribute
- **Expected outcome:**
  - The `src` attribute of the avatar `<img>` does NOT contain `lh3.googleusercontent.com`
  - If `STORAGE_BACKEND=local`: `src` starts with `http://localhost:4000/uploads/images/avatars/`
  - If `STORAGE_BACKEND=s3`: `src` contains the configured S3 public URL and path `avatars/`
  - The image renders visibly (not a broken image icon)
- **Source of truth:** User requirement ("store it locally in our storage bucket instead") and implementation plan (avatar stored under `avatars/{uuid}.{ext}` key in images bucket)
- **Interactions:** Google OAuth token exchange, reqwest HTTP client downloading from Google, ObjectStorage put operation, Postgres `users.image` column update via `upsert_user`

### Test 2: Avatar image file exists in the storage backend (Integration)

- **Name:** Avatar file is physically present in the images storage location after login
- **Type:** integration
- **Harness:** Playwright MCP + shell inspection
- **Preconditions:**
  - Test 1 has completed (user is logged in, avatar URL is known)
- **Actions:**
  1. Extract the avatar URL from the `<img>` `src` attribute (obtained in Test 1)
  2. If `STORAGE_BACKEND=local`: check that the file exists at `./uploads/images/avatars/<uuid>.<ext>` on the filesystem
  3. If `STORAGE_BACKEND=s3`: issue an HTTP GET to the avatar URL and verify it returns 200
- **Expected outcome:**
  - The file/object exists and is accessible
  - HTTP GET returns status 200
  - Response content-type is an image type (`image/jpeg`, `image/png`, `image/webp`, or `image/gif`)
- **Source of truth:** Implementation plan Task 3 (avatar stored via `images_storage.put()` which writes to local filesystem or S3)
- **Interactions:** ObjectStorage read path (LocalStorage filesystem or S3Storage bucket), static file serving (Axum `ServeDir` for local uploads)

### Test 3: No 429 errors on avatar image loading (Regression)

- **Name:** Page load does not produce 429 (Too Many Requests) errors for avatar images
- **Type:** regression
- **Harness:** Playwright MCP (network monitoring)
- **Preconditions:**
  - User is logged in (Test 1 completed)
- **Actions:**
  1. Navigate to `http://localhost:4000/bookmarks` (the main page that shows the avatar in the header)
  2. Monitor network requests during page load
  3. Take screenshot of the page with avatar visible
  4. Check all network responses for any 429 status codes, particularly on image requests
- **Expected outcome:**
  - Zero network requests to `lh3.googleusercontent.com`
  - Zero 429 status responses
  - Avatar image loads successfully (200 status)
- **Source of truth:** User's original problem statement ("google oauth login image is getting a 429 because we are hot-loading it directly from google")
- **Interactions:** Browser network stack, image serving from local/S3 storage

### Test 4: Avatar graceful degradation when image download fails (Boundary)

- **Name:** When avatar download fails, user still logs in successfully with fallback behavior
- **Type:** boundary
- **Harness:** Playwright MCP (E2E test login path)
- **Preconditions:**
  - App is running with `ENABLE_E2E_AUTH=1`
  - E2E test user has no avatar (the `test_login` endpoint passes `None` for image)
- **Actions:**
  1. Navigate to `http://localhost:4000/auth/login`
  2. Use the E2E test login (if available) or observe behavior when no picture URL is provided
  3. Inspect the profile menu trigger element
- **Expected outcome:**
  - Login succeeds without error
  - The profile area shows the email-initial fallback (`<div>` with initial letter) instead of an `<img>` tag
  - No JavaScript errors in console
- **Source of truth:** Implementation plan Task 3 fallback logic ("On any failure downloading/storing the avatar, we fall back to the Google URL") and existing template logic in `header.html` (lines 32-38: `{% if let Some(img) = user.image %}` with fallback to email initial)
- **Interactions:** AuthService `upsert_user` with `None` image, Askama template conditional rendering

### Test 5: Images bucket config field is recognized (Unit)

- **Name:** Server accepts S3_IMAGES_BUCKET environment variable and defaults to "boopmark-images"
- **Type:** unit
- **Harness:** `cargo build` compilation check (no runtime test needed)
- **Preconditions:**
  - Implementation of Task 1 is complete
- **Actions:**
  1. Run `cargo build -p boopmark-server` with no `S3_IMAGES_BUCKET` env var set
  2. Verify compilation succeeds (the default value is applied)
- **Expected outcome:**
  - Build succeeds without errors
  - The `s3_images_bucket` field exists on `Config` struct with default `"boopmark-images"`
- **Source of truth:** Implementation plan Task 1
- **Interactions:** None (pure config parsing)

---

## Coverage Summary

### Covered

| Area | Tests |
|------|-------|
| Core user flow: Google login stores avatar locally | Test 1 |
| Storage integration: file exists in bucket | Test 2 |
| Original bug regression: no 429 errors | Test 3 |
| Graceful degradation: no avatar / failed download | Test 4 |
| Config field compilation | Test 5 |

### Explicitly excluded (per agreed strategy)

| Area | Reason | Risk |
|------|--------|------|
| Automated Google OAuth flow | Google ToS prohibits automated credential entry; human-in-the-loop agreed with user | Low — manual step covers the same code path |
| Automated test suite additions (e.g., new Playwright spec files) | User explicitly stated "No automated test suite additions needed" | Medium — no regression protection after initial verification, but the feature is simple and unlikely to regress without related changes |
| S3-specific bucket creation/permissions | Requires real S3 credentials and bucket provisioning; tested implicitly via E2E if `STORAGE_BACKEND=s3` | Low — standard S3 operations, same pattern as existing bookmark image storage |
| Avatar re-download on subsequent logins | The `upsert_user` SQL uses `COALESCE($3, users.image)` which means a new avatar URL replaces the old one on every login; old avatar files are orphaned in storage | Low for correctness (avatar stays current), but storage cleanup is a future concern |
| Multiple concurrent logins storing avatars | Race condition on UUID-based keys is effectively impossible | Negligible |
