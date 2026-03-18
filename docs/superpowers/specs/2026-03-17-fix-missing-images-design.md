# Fix Missing Images — Design Spec

**Date:** 2026-03-17
**Status:** Approved

## Overview

A user-triggered, on-demand background job that finds bookmarks with missing or broken images, attempts to fetch an og:image from the bookmark URL, and falls back to a real screenshot via a self-hosted Playwright sidecar. Surfaced via web UI, REST API, and CLI.

---

## What Counts as "Missing"

- `image_url IS NULL` — never fetched
- `image_url IS NOT NULL` but a HEAD request to the URL returns a non-2xx response

---

## Service Layer

A new `fix_missing_images(user_id)` method on `BookmarkService`.

### Flow

1. **Dedup check** — if a job is already running for this user (via `Arc<Mutex<HashSet<UserId>>>` in `AppState`), return 409 Conflict immediately.
2. **Load candidates** — single query: all bookmarks for the user.
3. **Spawn task** — `tokio::spawn` with an `mpsc::Sender<ProgressEvent>`.
4. **Task loop** — for each bookmark:
   - If `image_url` is NULL → proceed to fix
   - If `image_url` is set → HEAD request; if non-2xx → proceed to fix
   - **Fix flow:**
     1. Scrape bookmark URL for og:image → download → store (existing path)
     2. If no og:image found → call screenshot sidecar → store result
     3. Update `image_url` in DB
   - Send one progress event after each bookmark regardless of outcome
5. **Cleanup** — remove user from dedup set when task finishes (success or error).

### Progress Event Shape

```json
{"checked": 5, "total": 47, "fixed": 2, "failed": 1}
```

### Final Event

```json
{"done": true, "fixed": 12, "failed": 3}
```

Individual failures (scrape fails, sidecar unavailable, no image found) increment `failed` and continue — the job never aborts mid-run.

---

## Screenshot Sidecar

A minimal Node.js service added to Docker Compose using Playwright/Chromium.

**Endpoint:**
```
POST /screenshot
Body: { "url": "https://example.com" }
Response: image/jpeg bytes
```

**Behavior:**
- Launches a persistent Chromium instance on startup (reused across requests)
- Viewport: **1200×630** (matches og:image standard aspect ratio of 40:21)
- Navigates to URL, waits for `networkidle`, takes a JPEG screenshot of the full viewport
- Returns raw bytes — Axum streams directly to storage

**Axum integration:**
- `reqwest` POST to `http://screenshot-svc:3001/screenshot`
- Returned bytes go through the same `download_and_store_image` path used for og:images
- Configured via `SCREENSHOT_SERVICE_URL` env var (default: `http://localhost:3001`)
- Sidecar unavailable → screenshot step skipped, bookmark counted as `failed`

---

## Card Image Area

Update the bookmark card image container from a fixed `h-40` height to `aspect-[40/21]` (1200×630 og:image standard). This ensures og:images display with zero cropping and screenshots are taken at the same ratio.

```html
<!-- before -->
<div class="h-40 bg-[#151827] flex items-center justify-center overflow-hidden">

<!-- after -->
<div class="aspect-[40/21] bg-[#151827] flex items-center justify-center overflow-hidden">
```

---

## API

### Endpoint

```
POST /api/v1/bookmarks/fix-images
Authorization: Bearer <token>
Accept: text/event-stream
```

**Responses:**
- `409 Conflict` — job already running for this user
- `200 text/event-stream` — SSE stream

### SSE Format

Standard SSE with JSON `data:` lines:

```
data: {"checked":5,"total":47,"fixed":2,"failed":1}

data: {"checked":6,"total":47,"fixed":2,"failed":2}

data: {"done":true,"fixed":12,"failed":3}
```

Unrecoverable server error closes the stream with:
```
event: error
data: {"error": "internal server error"}
```

---

## Web UI

A dedicated GET route for HTMX SSE (browser `EventSource` is GET-only):

```
GET /settings/fix-images/stream   (session auth, SSE)
```

Both routes share the same underlying service method.

### Settings Page

1. `[Fix Missing Images]` button + empty progress area on initial load
2. Button click → `hx-get` swaps in a progress component that includes `hx-ext="sse" sse-connect="/settings/fix-images/stream"` — HTMX SSE extension connects immediately
3. Progress bar (Tailwind) + label update via a small `<script>` reading JSON events:

```html
<div class="w-full bg-gray-700 rounded-full h-2">
  <div id="progress-fill"
       class="bg-blue-500 h-2 rounded-full transition-all duration-300"
       style="width: 0%"></div>
</div>
<p id="progress-label">Checking images: 0 / 47 — Fixed: 0 — Failed: 0</p>
```

4. On `done:true` event: button re-enables, final summary shown (`Fixed 8 images, 3 failed`)
5. If 409: show `"Already running..."` message

---

## CLI

New subcommand: `boop images fix`

**Behavior:**
1. `POST /api/v1/bookmarks/fix-images` with bearer token + `Accept: text/event-stream`
2. Read SSE stream line by line, parsing `data:` events
3. Print a single updating line using `\r`:
   ```
   Checking images: 12 / 47 — Fixed: 8 — Failed: 1
   ```
4. On `done:true`: print final newline + summary:
   ```
   Done. Fixed 8 images. 3 failed (no image found).
   ```
5. If 409: print `"A fix-images job is already running for your account."` and exit non-zero

---

## Component Summary

| Component | Change |
|---|---|
| `BookmarkService` | Add `fix_missing_images(user_id)` method |
| `AppState` | Add `Arc<Mutex<HashSet<UserId>>>` for dedup |
| `server/src/adapters/screenshot/` | New Playwright sidecar HTTP client |
| `screenshot-svc/` | New Node.js Docker Compose service |
| `docker-compose.yml` | Add screenshot sidecar service |
| `server/src/web/api/` | Add `POST /api/v1/bookmarks/fix-images` route |
| `server/src/web/` | Add `GET /settings/fix-images/stream` route |
| `templates/settings/` | Add fix-images section with progress bar |
| `templates/bookmarks/card.html` | Update image container to `aspect-[40/21]` |
| `cli/src/main.rs` | Add `boop images fix` subcommand |
