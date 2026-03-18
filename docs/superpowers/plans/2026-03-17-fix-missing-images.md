# Fix Missing Images Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an on-demand background job that finds bookmarks with missing or broken images, re-scrapes og:image, and falls back to a Playwright screenshot sidecar — surfaced via API, web UI, and CLI.

**Architecture:** A `fix_missing_images` method on `BookmarkService` accepts an `mpsc::Sender<ProgressEvent>` and processes bookmarks in a `tokio::spawn` task, streaming JSON progress events via SSE. A per-user `HashSet` in `AppState` prevents concurrent duplicate jobs. A minimal Node.js Playwright service provides screenshot fallback.

**Tech Stack:** Rust/Axum 0.8 SSE (`axum::response::sse`), `tokio-stream` (`ReceiverStream`), `reqwest` 0.12, Node.js 20 + Playwright Chromium, HTMX + `fetch()` streaming, clap 4.

---

## Chunk 1: Card UI + Screenshot Sidecar + Rust Adapter

### Task 1: Update bookmark card image aspect ratio

**Files:**
- Modify: `templates/bookmarks/card.html`

- [ ] **Step 1: Update both image divs from `h-40` to `aspect-[40/21]`**

  Replace the two `h-40` divs (image present + placeholder):

  ```html
  <!-- image present -->
  <div class="aspect-[40/21] bg-[#151827] flex items-center justify-center overflow-hidden">
      <img src="{{ img }}" alt="" class="w-full h-full object-cover" loading="lazy" data-testid="bookmark-card-image">
  </div>

  <!-- placeholder (no image) -->
  <div class="aspect-[40/21] bg-[#151827] flex items-center justify-center">
      <span class="text-4xl text-gray-600">&#128278;</span>
  </div>
  ```

- [ ] **Step 2: Verify templates compile**

  Run: `cargo build -p boopmark-server`
  Expected: Compiles without errors (Askama validates templates at compile time).

- [ ] **Step 3: Commit**

  ```bash
  git add templates/bookmarks/card.html
  git commit -m "feat: update bookmark card image to aspect-[40/21] (og:image standard)"
  ```

---

### Task 2: Screenshot sidecar (Node.js + Playwright)

**Files:**
- Create: `screenshot-svc/package.json`
- Create: `screenshot-svc/index.js`
- Create: `screenshot-svc/Dockerfile`
- Modify: `docker-compose.yml`

- [ ] **Step 1: Create `screenshot-svc/package.json`**

  ```json
  {
    "name": "screenshot-svc",
    "version": "1.0.0",
    "description": "Playwright screenshot microservice for Boopmark",
    "main": "index.js",
    "scripts": {
      "start": "node index.js"
    },
    "dependencies": {
      "playwright": "^1.50.0"
    }
  }
  ```

- [ ] **Step 2: Create `screenshot-svc/index.js`**

  ```js
  const { chromium } = require('playwright');
  const http = require('http');

  const PORT = process.env.PORT || 3001;
  let browser;

  async function init() {
    browser = await chromium.launch({ args: ['--no-sandbox'] });
    console.log(`Screenshot service ready on port ${PORT}`);
  }

  const server = http.createServer(async (req, res) => {
    if (req.method !== 'POST' || req.url !== '/screenshot') {
      res.writeHead(404);
      res.end();
      return;
    }

    let body = '';
    req.on('data', chunk => { body += chunk; });
    req.on('end', async () => {
      let url;
      try {
        ({ url } = JSON.parse(body));
      } catch {
        res.writeHead(400);
        res.end(JSON.stringify({ error: 'invalid JSON' }));
        return;
      }

      if (!url) {
        res.writeHead(400);
        res.end(JSON.stringify({ error: 'url required' }));
        return;
      }

      let page;
      try {
        page = await browser.newPage();
        await page.setViewportSize({ width: 1200, height: 630 });
        await page.goto(url, { waitUntil: 'networkidle', timeout: 15000 });
        const screenshot = await page.screenshot({ type: 'jpeg', quality: 85 });
        res.writeHead(200, { 'Content-Type': 'image/jpeg' });
        res.end(screenshot);
      } catch (err) {
        console.error(`Screenshot failed for ${url}:`, err.message);
        res.writeHead(500);
        res.end(JSON.stringify({ error: err.message }));
      } finally {
        if (page) await page.close().catch(() => {});
      }
    });
  });

  init().then(() => server.listen(PORT));
  ```

- [ ] **Step 3: Create `screenshot-svc/Dockerfile`**

  ```dockerfile
  FROM mcr.microsoft.com/playwright:v1.50.0-noble

  WORKDIR /app
  COPY package.json .
  RUN npm install --omit=dev
  RUN npx playwright install chromium

  COPY index.js .

  EXPOSE 3001
  CMD ["node", "index.js"]
  ```

- [ ] **Step 4: Test the sidecar locally**

  ```bash
  cd screenshot-svc && npm install && node index.js &
  sleep 3
  curl -s -X POST http://localhost:3001/screenshot \
    -H 'Content-Type: application/json' \
    -d '{"url":"https://example.com"}' \
    --output /tmp/test-screenshot.jpg
  file /tmp/test-screenshot.jpg
  kill %1
  cd ..
  ```

  Expected: `/tmp/test-screenshot.jpg: JPEG image data`

- [ ] **Step 5: Add screenshot sidecar to `docker-compose.yml`**

  Add after the `minio` service, before the `server` service:

  ```yaml
    screenshot-svc:
      build: ./screenshot-svc
      ports:
        - "3001:3001"
      environment:
        PORT: "3001"
  ```

  Add to `server` service's `depends_on`:
  ```yaml
      screenshot-svc:
        condition: service_started
  ```

  Add `SCREENSHOT_SERVICE_URL: http://screenshot-svc:3001` to `server` service `environment`.

- [ ] **Step 6: Commit**

  ```bash
  git add screenshot-svc/ docker-compose.yml
  git commit -m "feat: add Playwright screenshot sidecar service"
  ```

---

### Task 3: Rust screenshot adapter

**Files:**
- Create: `server/src/adapters/screenshot.rs`
- Modify: `server/src/adapters/mod.rs`

- [ ] **Step 1: Write failing test**

  Create `server/src/adapters/screenshot.rs` with the test first:

  ```rust
  use crate::domain::error::DomainError;

  pub struct ScreenshotClient {
      http: reqwest::Client,
      base_url: String,
  }

  impl ScreenshotClient {
      pub fn new(base_url: String) -> Self {
          let http = reqwest::Client::builder()
              .timeout(std::time::Duration::from_secs(30))
              .build()
              .expect("failed to build screenshot HTTP client");
          Self { http, base_url }
      }

      pub async fn capture(&self, page_url: &str) -> Result<Vec<u8>, DomainError> {
          let resp = self
              .http
              .post(format!("{}/screenshot", self.base_url))
              .json(&serde_json::json!({ "url": page_url }))
              .send()
              .await
              .map_err(|e| DomainError::Internal(e.to_string()))?;

          if !resp.status().is_success() {
              return Err(DomainError::Internal(format!(
                  "screenshot sidecar returned {}",
                  resp.status()
              )));
          }

          resp.bytes()
              .await
              .map(|b| b.to_vec())
              .map_err(|e| DomainError::Internal(e.to_string()))
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use axum::{Router, routing::post, response::IntoResponse};

      async fn fake_screenshot() -> impl IntoResponse {
          // Return minimal valid JPEG bytes (SOI + EOI markers)
          let jpeg_bytes: Vec<u8> = vec![0xFF, 0xD8, 0xFF, 0xD9];
          (
              axum::http::StatusCode::OK,
              [("Content-Type", "image/jpeg")],
              jpeg_bytes,
          )
      }

      #[tokio::test]
      async fn capture_returns_bytes_from_sidecar() {
          let app = Router::new().route("/screenshot", post(fake_screenshot));
          let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
          let addr = listener.local_addr().unwrap();
          tokio::spawn(axum::serve(listener, app).into_future());

          let client = ScreenshotClient::new(format!("http://{}", addr));
          let result = client.capture("https://example.com").await;

          assert!(result.is_ok());
          let bytes = result.unwrap();
          assert_eq!(&bytes[..2], &[0xFF, 0xD8]); // JPEG SOI marker
      }

      #[tokio::test]
      async fn capture_returns_error_on_sidecar_failure() {
          let app = Router::new().route(
              "/screenshot",
              post(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR }),
          );
          let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
          let addr = listener.local_addr().unwrap();
          tokio::spawn(axum::serve(listener, app).into_future());

          let client = ScreenshotClient::new(format!("http://{}", addr));
          let result = client.capture("https://example.com").await;

          assert!(result.is_err());
      }
  }
  ```

- [ ] **Step 2: Run tests to verify they fail**

  Run: `cargo test -p boopmark-server screenshot`
  Expected: Compile error — `ScreenshotClient` not yet in `adapters/mod.rs`.

- [ ] **Step 3: Register module in `server/src/adapters/mod.rs`**

  ```rust
  pub mod anthropic;
  pub mod postgres;
  pub mod scraper;
  pub mod screenshot;
  pub mod storage;
  ```

- [ ] **Step 4: Add `futures` to server dev-dependencies for `into_future()` in tests**

  In `server/Cargo.toml`, update `[dev-dependencies]` to:
  ```toml
  [dev-dependencies]
  tokio = { workspace = true, features = ["test-util"] }
  futures = "0.3"
  ```

  This is a dev-only dep — do NOT add `futures` to workspace `[dependencies]`.

- [ ] **Step 5: Run tests to verify they pass**

  Run: `cargo test -p boopmark-server screenshot`
  Expected: 2 tests pass.

- [ ] **Step 6: Commit**

  ```bash
  git add server/src/adapters/screenshot.rs server/src/adapters/mod.rs \
    server/Cargo.toml Cargo.toml
  git commit -m "feat: add ScreenshotClient adapter for Playwright sidecar"
  ```

---

## Chunk 2: Service Layer

### Task 4: Add `update_image_url` to bookmark repository

**Files:**
- Modify: `server/src/domain/ports/bookmark_repo.rs`
- Modify: `server/src/adapters/postgres/bookmark_repo.rs`

- [ ] **Step 1: Add method to the `BookmarkRepository` trait**

  In `server/src/domain/ports/bookmark_repo.rs`, add to the trait:

  ```rust
  async fn update_image_url(
      &self,
      id: Uuid,
      user_id: Uuid,
      image_url: &str,
  ) -> Result<(), DomainError>;
  ```

- [ ] **Step 2: Run `cargo build` to confirm compile errors**

  Run: `cargo build -p boopmark-server`
  Expected: Two compile errors — `PostgresBookmarkRepo` and the test `MockRepo` in `bookmarks.rs` both fail to implement `update_image_url`.

- [ ] **Step 3a: Add `update_image_url` stub to `MockRepo` in `server/src/app/bookmarks.rs` tests**

  In the `impl BookmarkRepository for MockRepo` block (inside `mod import_tests`), add after the `upsert_full` method:

  ```rust
  async fn update_image_url(
      &self,
      id: Uuid,
      user_id: Uuid,
      image_url: &str,
  ) -> Result<(), DomainError> {
      let mut bookmarks = self.bookmarks.lock().unwrap();
      if let Some(b) = bookmarks.iter_mut().find(|b| b.id == id && b.user_id == user_id) {
          b.image_url = Some(image_url.to_string());
          Ok(())
      } else {
          Err(DomainError::NotFound)
      }
  }
  ```

- [ ] **Step 3b: Implement in `server/src/adapters/postgres/bookmark_repo.rs`**

  Add this impl block after the existing `update` method:

  ```rust
  async fn update_image_url(
      &self,
      id: Uuid,
      user_id: Uuid,
      image_url: &str,
  ) -> Result<(), DomainError> {
      sqlx::query!(
          "UPDATE bookmarks SET image_url = $1, updated_at = now() \
           WHERE id = $2 AND user_id = $3",
          image_url,
          id,
          user_id
      )
      .execute(&self.pool)
      .await
      .map(|_| ())
      .map_err(|e| DomainError::Internal(e.to_string()))
  }
  ```

- [ ] **Step 4: Build to confirm it compiles**

  Run: `cargo build -p boopmark-server`
  Expected: Compiles cleanly.

- [ ] **Step 5: Commit**

  ```bash
  git add server/src/domain/ports/bookmark_repo.rs \
    server/src/adapters/postgres/bookmark_repo.rs \
    server/src/app/bookmarks.rs
  git commit -m "feat: add update_image_url to BookmarkRepository"
  ```

---

### Task 5: `fix_missing_images` service method + AppState dedup + Config

**Files:**
- Modify: `server/src/config.rs`
- Modify: `server/src/web/state.rs`
- Modify: `server/src/app/bookmarks.rs`

- [ ] **Step 1: Add `screenshot_service_url` to `Config`**

  In `server/src/config.rs`, add field to `Config`:
  ```rust
  pub screenshot_service_url: Option<String>,
  ```

  In `Config::from_env()`, add:
  ```rust
  screenshot_service_url: env::var("SCREENSHOT_SERVICE_URL").ok(),
  ```

- [ ] **Step 2: Add `active_image_fix_jobs` to `AppState`**

  In `server/src/web/state.rs`, add import at top:
  ```rust
  use std::collections::HashSet;
  use std::sync::Mutex;
  use uuid::Uuid;
  ```

  Add field to `AppState`:
  ```rust
  pub active_image_fix_jobs: Arc<Mutex<HashSet<Uuid>>>,
  ```

  Update `AppState` construction sites in `server/src/main.rs` to include:
  ```rust
  active_image_fix_jobs: Arc::new(Mutex::new(HashSet::new())),
  ```

- [ ] **Step 3: Add `ProgressEvent` type and `fix_missing_images` to `BookmarkService`**

  Add to `server/src/app/bookmarks.rs` (after the existing `use` imports):

  ```rust
  use tokio::sync::mpsc;

  #[derive(serde::Serialize, Clone, Debug)]
  pub struct ProgressEvent {
      pub checked: usize,
      pub total: usize,
      pub fixed: usize,
      pub failed: usize,
      pub done: bool,
  }
  ```

  Add method to the `BookmarkService` impl block:

  ```rust
  pub async fn fix_missing_images(
      &self,
      user_id: Uuid,
      screenshot_service_url: Option<&str>,
      tx: mpsc::Sender<ProgressEvent>,
  ) {
      let bookmarks = match self.repo.export_all(user_id).await {
          Ok(b) => b,
          Err(_) => return,
      };

      let total = bookmarks.len();
      let mut checked = 0;
      let mut fixed = 0;
      let mut failed = 0;

      for bookmark in bookmarks {
          let needs_fix = match &bookmark.image_url {
              None => true,
              Some(url) => self
                  .http_client
                  .head(url)
                  .send()
                  .await
                  .map(|r| !r.status().is_success())
                  .unwrap_or(true),
          };

          if needs_fix {
              match self.fetch_and_store_image(&bookmark.url, screenshot_service_url).await {
                  Ok(new_url) => {
                      if self
                          .repo
                          .update_image_url(bookmark.id, user_id, &new_url)
                          .await
                          .is_ok()
                      {
                          fixed += 1;
                      } else {
                          failed += 1;
                      }
                  }
                  Err(_) => failed += 1,
              }
          }

          checked += 1;
          tx.send(ProgressEvent { checked, total, fixed, failed, done: false })
              .await
              .ok();
      }

      tx.send(ProgressEvent { checked, total, fixed, failed, done: true })
          .await
          .ok();
  }

  /// Try og:image scrape first; fall back to screenshot sidecar.
  async fn fetch_and_store_image(
      &self,
      page_url: &str,
      screenshot_service_url: Option<&str>,
  ) -> Result<String, crate::domain::error::DomainError> {
      // 1. Try og:image
      if let Ok(meta) = self.metadata.extract(page_url).await {
          if let Some(image_url) = meta.image_url {
              if let Ok(stored) = self.download_and_store_image(&image_url).await {
                  return Ok(stored);
              }
          }
      }

      // 2. Fall back to screenshot sidecar via ScreenshotClient adapter
      let svc_url = screenshot_service_url
          .ok_or_else(|| crate::domain::error::DomainError::Internal("no screenshot svc".into()))?;

      let screenshot_client = crate::adapters::screenshot::ScreenshotClient::new(svc_url.to_string());
      let bytes = screenshot_client.capture(page_url).await?;

      let key = format!("images/{}.jpg", uuid::Uuid::new_v4());
      self.storage.put(&key, bytes, "image/jpeg").await
  }
  ```

  > Note: `download_and_store_image` is an existing private method on `BookmarkService` — `fetch_and_store_image` reuses it for the og:image path.

- [ ] **Step 4: Build to confirm it compiles**

  Run: `cargo build -p boopmark-server`
  Expected: Compiles cleanly. Fix any type errors.

- [ ] **Step 5: Commit**

  ```bash
  git add server/src/config.rs server/src/web/state.rs server/src/app/bookmarks.rs
  git commit -m "feat: add fix_missing_images service method with dedup and screenshot fallback"
  ```

---

## Chunk 3: API & Web Endpoints

### Task 6: API SSE endpoint

**Files:**
- Modify: `Cargo.toml` (workspace — add `tokio-stream`)
- Modify: `server/Cargo.toml` (add `tokio-stream`)
- Create: `server/src/web/api/image_fix.rs`
- Modify: `server/src/web/api/mod.rs`

- [ ] **Step 1: Add `tokio-stream` to workspace and server**

  In `Cargo.toml` workspace dependencies:
  ```toml
  tokio-stream = "0.1"
  ```

  In `server/Cargo.toml` dependencies:
  ```toml
  tokio-stream.workspace = true
  ```

- [ ] **Step 2: Write the API handler in `server/src/web/api/image_fix.rs`**

  ```rust
  use axum::extract::State;
  use axum::http::StatusCode;
  use axum::response::{IntoResponse, Response};
  use axum::response::sse::{Event, Sse};
  use std::convert::Infallible;
  use tokio::sync::mpsc;
  use tokio_stream::StreamExt;
  use tokio_stream::wrappers::ReceiverStream;

  use crate::app::bookmarks::ProgressEvent;
  use crate::web::extractors::AuthUser;
  use crate::web::state::{AppState, Bookmarks};

  pub async fn fix_images(
      State(state): State<AppState>,
      AuthUser(user): AuthUser,
  ) -> Response {
      let user_id = user.id;

      {
          let mut jobs = state.active_image_fix_jobs.lock().unwrap();
          if jobs.contains(&user_id) {
              return StatusCode::CONFLICT.into_response();
          }
          jobs.insert(user_id);
      }

      let (tx, rx) = mpsc::channel::<ProgressEvent>(32);
      let jobs = state.active_image_fix_jobs.clone();
      let screenshot_url = state.config.screenshot_service_url.clone();

      tokio::spawn(async move {
          match &state.bookmarks {
              Bookmarks::Local(svc) => {
                  svc.fix_missing_images(user_id, screenshot_url.as_deref(), tx).await
              }
              Bookmarks::S3(svc) => {
                  svc.fix_missing_images(user_id, screenshot_url.as_deref(), tx).await
              }
          }
          jobs.lock().unwrap().remove(&user_id);
      });

      let stream = ReceiverStream::new(rx).map(|event| {
          let json = serde_json::to_string(&event).unwrap_or_default();
          Ok::<_, Infallible>(Event::default().data(json))
      });

      Sse::new(stream).into_response()
  }

  pub fn routes() -> axum::Router<AppState> {
      // Note: AuthUser handles both Bearer token (API key) and session cookie auth.
      // This route therefore works for both API consumers and browser sessions.
      axum::Router::new().route(
          "/fix-images",
          axum::routing::post(fix_images),
      )
  }
  ```

- [ ] **Step 3: Replace the entire contents of `server/src/web/api/mod.rs`**

  ```rust
  pub mod auth;
  pub mod bookmarks;
  pub mod image_fix;
  pub mod transfer;

  use crate::web::state::AppState;
  use axum::Router;

  pub fn routes() -> Router<AppState> {
      Router::new()
          .nest("/bookmarks", bookmarks::routes()
              .merge(transfer::routes())
              .merge(image_fix::routes()))
          .nest("/auth", auth::routes())
  }
  ```

- [ ] **Step 4: Build to confirm it compiles**

  Run: `cargo build -p boopmark-server`
  Expected: Compiles cleanly.

- [ ] **Step 5: Smoke test the endpoint**

  Start the server and test with curl:
  ```bash
  curl -N -X POST http://localhost:4000/api/v1/bookmarks/fix-images \
    -H "Authorization: Bearer YOUR_API_KEY" \
    -H "Accept: text/event-stream"
  ```
  Expected: SSE stream of JSON progress events, ending with `"done":true`.

  Test 409:
  ```bash
  # Start two concurrent requests — second should return 409
  curl -N -X POST http://localhost:4000/api/v1/bookmarks/fix-images \
    -H "Authorization: Bearer YOUR_API_KEY" \
    -H "Accept: text/event-stream" &
  sleep 0.5
  curl -s -o /dev/null -w "%{http_code}" -X POST \
    http://localhost:4000/api/v1/bookmarks/fix-images \
    -H "Authorization: Bearer YOUR_API_KEY"
  # Expected: 409
  ```

- [ ] **Step 6: Commit**

  ```bash
  git add Cargo.toml server/Cargo.toml server/src/web/api/image_fix.rs \
    server/src/web/api/mod.rs
  git commit -m "feat: add POST /api/v1/bookmarks/fix-images SSE endpoint"
  ```

---

### Task 7: Web SSE endpoint + settings UI

**Files:**
- Modify: `server/src/web/pages/settings.rs`
- Modify: `templates/settings/index.html`

- [ ] **Step 1: Add the web SSE handler to `server/src/web/pages/settings.rs`**

  Add imports at top:
  ```rust
  use axum::response::sse::{Event, Sse};
  use std::convert::Infallible;
  use tokio::sync::mpsc;
  use tokio_stream::StreamExt;
  use tokio_stream::wrappers::ReceiverStream;
  use crate::app::bookmarks::ProgressEvent;
  use crate::web::state::Bookmarks;
  ```

  Add handler function:
  ```rust
  async fn fix_images_stream(
      State(state): State<AppState>,
      AuthUser(user): AuthUser,
  ) -> axum::response::Response {
      use axum::http::StatusCode;
      use axum::response::IntoResponse;

      let user_id = user.id;

      {
          let mut jobs = state.active_image_fix_jobs.lock().unwrap();
          if jobs.contains(&user_id) {
              return StatusCode::CONFLICT.into_response();
          }
          jobs.insert(user_id);
      }

      let (tx, rx) = mpsc::channel::<ProgressEvent>(32);
      let jobs = state.active_image_fix_jobs.clone();
      let screenshot_url = state.config.screenshot_service_url.clone();

      tokio::spawn(async move {
          match &state.bookmarks {
              Bookmarks::Local(svc) => {
                  svc.fix_missing_images(user_id, screenshot_url.as_deref(), tx).await
              }
              Bookmarks::S3(svc) => {
                  svc.fix_missing_images(user_id, screenshot_url.as_deref(), tx).await
              }
          }
          jobs.lock().unwrap().remove(&user_id);
      });

      let stream = ReceiverStream::new(rx).map(|event| {
          let json = serde_json::to_string(&event).unwrap_or_default();
          Ok::<_, Infallible>(Event::default().data(json))
      });

      Sse::new(stream).into_response()
  }
  ```

  Register route in `pub fn routes()`:
  ```rust
  .route(
      "/settings/fix-images/stream",
      axum::routing::get(fix_images_stream),
  )
  ```

- [ ] **Step 2: Add the settings UI section to `templates/settings/index.html`**

  Add a new `<section>` before the closing `</div></main>`:

  ```html
  <section class="space-y-5">
      <div>
          <h2 class="text-lg font-semibold">Image Repair</h2>
          <p class="text-sm text-gray-400">Fetch missing or broken bookmark images.</p>
      </div>

      <div id="fix-images-section" class="space-y-3">
          <button
              id="fix-images-btn"
              class="px-4 py-2 bg-blue-600 hover:bg-blue-700 rounded-lg text-sm font-medium disabled:opacity-50 disabled:cursor-not-allowed"
          >
              Fix Missing Images
          </button>

          <div id="fix-images-progress" class="hidden space-y-2">
              <div class="w-full bg-gray-700 rounded-full h-2">
                  <div
                      id="fix-images-fill"
                      class="bg-blue-500 h-2 rounded-full transition-all duration-300"
                      style="width: 0%"
                  ></div>
              </div>
              <p id="fix-images-label" class="text-sm text-gray-400"></p>
          </div>
      </div>

      <script>
          document.getElementById('fix-images-btn').addEventListener('click', async () => {
              const btn = document.getElementById('fix-images-btn');
              const progress = document.getElementById('fix-images-progress');
              const fill = document.getElementById('fix-images-fill');
              const label = document.getElementById('fix-images-label');

              btn.disabled = true;
              progress.classList.remove('hidden');
              label.textContent = 'Starting…';

              let response;
              try {
                  response = await fetch('/settings/fix-images/stream');
              } catch {
                  label.textContent = 'Connection error.';
                  btn.disabled = false;
                  return;
              }

              if (response.status === 409) {
                  label.textContent = 'A fix-images job is already running.';
                  btn.disabled = false;
                  return;
              }

              if (!response.ok) {
                  label.textContent = 'Server error. Please try again.';
                  btn.disabled = false;
                  return;
              }

              const reader = response.body.getReader();
              const decoder = new TextDecoder();
              let buf = '';

              try {
                  while (true) {
                      const { done, value } = await reader.read();
                      if (done) break;
                      buf += decoder.decode(value, { stream: true });
                      const lines = buf.split('\n');
                      buf = lines.pop();
                      for (const line of lines) {
                          if (!line.startsWith('data: ')) continue;
                          let data;
                          try { data = JSON.parse(line.slice(6)); } catch { continue; }
                          const pct = data.total > 0
                              ? ((data.checked / data.total) * 100).toFixed(0)
                              : 0;
                          fill.style.width = `${pct}%`;
                          if (data.done) {
                              label.textContent = `Done. Fixed ${data.fixed} images. ${data.failed} failed.`;
                              btn.disabled = false;
                          } else {
                              label.textContent =
                                  `Checking images: ${data.checked} / ${data.total} — Fixed: ${data.fixed} — Failed: ${data.failed}`;
                          }
                      }
                  }
              } catch {
                  label.textContent = 'Stream interrupted.';
                  btn.disabled = false;
              }
          });
      </script>
  </section>
  ```

- [ ] **Step 3: Build to confirm it compiles**

  Run: `cargo build -p boopmark-server`
  Expected: Compiles cleanly.

- [ ] **Step 4: Smoke test the web UI**

  Start the server, sign in, open `/settings`, click "Fix Missing Images".
  Expected: Progress bar fills, label updates, button re-enables on completion.

- [ ] **Step 5: Commit**

  ```bash
  git add server/src/web/pages/settings.rs templates/settings/index.html
  git commit -m "feat: add fix-images web UI with SSE progress bar to settings page"
  ```

---

## Chunk 4: CLI

### Task 8: CLI `boop images fix` subcommand

**Files:**
- Modify: `Cargo.toml` (workspace — add `stream` feature to reqwest)
- Modify: `cli/Cargo.toml` (add `futures`)
- Modify: `cli/src/main.rs`

- [ ] **Step 1: Enable reqwest `stream` feature in workspace**

  In `Cargo.toml`:
  ```toml
  reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls", "multipart", "stream"] }
  ```

- [ ] **Step 2: Add `futures` directly to `cli/Cargo.toml`**

  > Note: `futures` was added as a server **dev-dependency** in Chunk 1 Task 3 Step 4 — it is NOT in the workspace `[dependencies]`, so it cannot be referenced via `{ workspace = true }` here.

  In `cli/Cargo.toml` dependencies, add:
  ```toml
  futures = "0.3"
  ```

- [ ] **Step 3: Add the `Images` command group to `cli/src/main.rs`**

  In the `Commands` enum, add:
  ```rust
  /// Manage bookmark images
  Images {
      #[command(subcommand)]
      command: ImagesCommands,
  },
  ```

  Add the subcommand enum after `Commands`:
  ```rust
  #[derive(Debug, clap::Subcommand)]
  enum ImagesCommands {
      /// Fetch missing or broken bookmark images
      Fix,
  }
  ```

- [ ] **Step 4: Add the `ProgressEvent` deserialization type**

  Near the top of `cli/src/main.rs` (with other local structs):
  ```rust
  #[derive(serde::Deserialize, Debug)]
  struct FixProgress {
      checked: usize,
      total: usize,
      fixed: usize,
      failed: usize,
      done: bool,
  }
  ```

- [ ] **Step 5: Implement the `Images { Fix }` arm in the command match**

  In the main match block handling commands, add:
  ```rust
  Commands::Images { command } => match command {
      ImagesCommands::Fix => {
          let client = reqwest::Client::new();
          let response = client
              .post(format!("{}/api/v1/bookmarks/fix-images", config.server))
              .bearer_auth(&config.api_key)
              .header("Accept", "text/event-stream")
              .send()
              .await?;

          if response.status() == reqwest::StatusCode::CONFLICT {
              eprintln!("A fix-images job is already running for your account.");
              std::process::exit(1);
          }

          if !response.status().is_success() {
              eprintln!("Error: server returned {}", response.status());
              std::process::exit(1);
          }

          use futures::StreamExt;
          let mut stream = response.bytes_stream();
          let mut buf = String::new();

          while let Some(chunk) = stream.next().await {
              let chunk = chunk?;
              buf.push_str(&String::from_utf8_lossy(&chunk));

              loop {
                  match buf.find('\n') {
                      None => break,
                      Some(pos) => {
                          let line = buf[..pos].trim().to_string();
                          buf.drain(..=pos);

                          if let Some(json_str) = line.strip_prefix("data: ") {
                              if let Ok(event) = serde_json::from_str::<FixProgress>(json_str) {
                                  if event.done {
                                      println!(
                                          "\nDone. Fixed {} images. {} failed (no image found).",
                                          event.fixed, event.failed
                                      );
                                      return Ok(());
                                  } else {
                                      print!(
                                          "\rChecking images: {} / {} — Fixed: {} — Failed: {}   ",
                                          event.checked, event.total, event.fixed, event.failed
                                      );
                                      use std::io::Write;
                                      std::io::stdout().flush().ok();
                                  }
                              }
                          }
                      }
                  }
              }
          }
      }
  },
  ```

- [ ] **Step 6: Build the CLI to confirm it compiles**

  Run: `cargo build -p boop`
  Expected: Compiles cleanly.

- [ ] **Step 7: Test CLI help output**

  Run: `cargo run -p boop -- images --help`
  Expected:
  ```
  Manage bookmark images

  Usage: boop images <COMMAND>

  Commands:
    fix  Fetch missing or broken bookmark images
  ```

  Run: `cargo run -p boop -- images fix --help`
  Expected: Shows `fix` subcommand help.

- [ ] **Step 8: End-to-end test the CLI**

  With the server running and bookmarks imported:
  ```bash
  cargo run -p boop -- images fix
  ```
  Expected: Live progress output, then final "Done." line.

- [ ] **Step 9: Commit**

  ```bash
  git add Cargo.toml cli/Cargo.toml cli/src/main.rs
  git commit -m "feat: add boop images fix CLI command with live SSE progress"
  ```

---

## Final verification

- [ ] Run full test suite: `cargo test`
- [ ] Run E2E: `npx playwright test tests/e2e/suggest.spec.js`
- [ ] Verify via devproxy: `devproxy up`, open settings page, click "Fix Missing Images"
- [ ] Verify CLI against prod: `cargo run -p boop -- images fix`
