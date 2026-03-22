# Open Source Readiness Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prepare Boopmark for open-sourcing by abstracting the screenshot service into a proper port, improving the self-hosting experience, and adding comprehensive documentation.

**Architecture:** Add a `ScreenshotProvider` port trait following the existing hexagonal pattern. Replace MinIO with RustFS. Add bootstrap tooling for self-hosters. Rewrite documentation for open-source audiences.

**Tech Stack:** Rust/Axum (existing), RustFS (replaces MinIO), Node 24 (Dockerfile CSS build stage), bash (bootstrap scripts)

**Spec:** `docs/superpowers/specs/2026-03-22-open-source-readiness-design.md`

---

## Chunk 1: Screenshot Port Abstraction

### Task 1: Create ScreenshotProvider Port Trait

**Files:**
- Create: `server/src/domain/ports/screenshot.rs`
- Modify: `server/src/domain/ports/mod.rs:1-10`

- [ ] **Step 1: Create the port trait file**

Create `server/src/domain/ports/screenshot.rs`.

Uses `Pin<Box<dyn Future>>` return type (same pattern as `LlmEnricher` port) to keep the trait object-safe for `Arc<dyn ScreenshotProvider>`:

```rust
use crate::domain::error::DomainError;
use std::future::Future;
use std::pin::Pin;

pub trait ScreenshotProvider: Send + Sync {
    fn capture(
        &self,
        url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, DomainError>> + Send + '_>>;
}
```

- [ ] **Step 2: Register the port module**

In `server/src/domain/ports/mod.rs`, add `pub mod screenshot;` to the module list.

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p boopmark-server`
Expected: SUCCESS (no consumers yet)

- [ ] **Step 4: Commit**

```bash
git add server/src/domain/ports/screenshot.rs server/src/domain/ports/mod.rs
git commit -m "feat: add ScreenshotProvider port trait"
```

---

### Task 2: Create PlaywrightScreenshot Adapter

**Files:**
- Create: `server/src/adapters/screenshot/playwright.rs`
- Create: `server/src/adapters/screenshot/mod.rs`
- Delete: `server/src/adapters/screenshot.rs` (replaced by directory)
- No changes needed to: `server/src/adapters/mod.rs` (already has `pub mod screenshot;`)

- [ ] **Step 1: Create adapter directory structure**

The current `server/src/adapters/screenshot.rs` needs to become `server/src/adapters/screenshot/` (a directory with `mod.rs`). This follows the same pattern as `adapters/storage/` and `adapters/login/`.

Create `server/src/adapters/screenshot/playwright.rs`:

```rust
use crate::domain::error::DomainError;
use crate::domain::ports::screenshot::ScreenshotProvider;
use std::future::Future;
use std::pin::Pin;

pub struct PlaywrightScreenshot {
    http: reqwest::Client,
    base_url: String,
}

impl PlaywrightScreenshot {
    pub fn new(base_url: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build screenshot HTTP client");
        Self { http, base_url }
    }
}

impl ScreenshotProvider for PlaywrightScreenshot {
    fn capture(
        &self,
        url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, DomainError>> + Send + '_>> {
        let url = url.to_string();
        Box::pin(async move {
            let resp = self
                .http
                .post(format!("{}/screenshot", self.base_url))
                .json(&serde_json::json!({ "url": url }))
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
        })
    }
}
```

- [ ] **Step 2: Create the Noop adapter**

Create `server/src/adapters/screenshot/noop.rs`:

```rust
use crate::domain::error::DomainError;
use crate::domain::ports::screenshot::ScreenshotProvider;
use std::future::Future;
use std::pin::Pin;

/// No-op screenshot provider used when screenshots are disabled.
pub struct NoopScreenshot;

impl ScreenshotProvider for NoopScreenshot {
    fn capture(
        &self,
        _url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, DomainError>> + Send + '_>> {
        Box::pin(async { Err(DomainError::Internal("screenshots are disabled".into())) })
    }
}
```

- [ ] **Step 3: Create the screenshot module file**

Delete `server/src/adapters/screenshot.rs` and create `server/src/adapters/screenshot/mod.rs`:

```rust
pub mod noop;
pub mod playwright;
```

- [ ] **Step 4: Note — crate will not compile yet**

The crate will NOT compile at this point because `bookmarks.rs` still references the deleted `ScreenshotClient`. This is expected. Continue writing tests below (they will be verified after Tasks 3-4 fix the call sites).

- [ ] **Step 5: Write tests for PlaywrightScreenshot**

Add to the bottom of `server/src/adapters/screenshot/playwright.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, response::IntoResponse, routing::post};

    async fn fake_screenshot() -> impl IntoResponse {
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
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let client = PlaywrightScreenshot::new(format!("http://{}", addr));
        let result = client.capture("https://example.com").await;

        assert!(result.is_ok());
        let bytes = result.unwrap();
        assert_eq!(&bytes[..2], &[0xFF, 0xD8]);
    }

    #[tokio::test]
    async fn capture_returns_error_on_sidecar_failure() {
        let app = Router::new().route(
            "/screenshot",
            post(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let client = PlaywrightScreenshot::new(format!("http://{}", addr));
        let result = client.capture("https://example.com").await;

        assert!(result.is_err());
    }
}
```

- [ ] **Step 6: Write test for NoopScreenshot**

Add to the bottom of `server/src/adapters/screenshot/noop.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn noop_capture_returns_error() {
        let client = NoopScreenshot;
        let result = client.capture("https://example.com").await;
        assert!(result.is_err());
    }
}
```

- [ ] **Step 7: Tests will be verified later**

The screenshot adapter tests (3 tests) will be verified in Task 4 Step 7 after all call sites are updated.

- [ ] **Step 8: Do NOT commit yet**

The crate will not compile until Tasks 3-4 update `bookmarks.rs` and `main.rs` to stop using the deleted `ScreenshotClient`. All adapter, wiring, and config changes will be committed together in Task 4.

---

### Task 3: Wire ScreenshotProvider into BookmarkService

**Files:**
- Modify: `server/src/app/bookmarks.rs:18-78,395-410`
- Modify: `server/src/main.rs:51-119`

Uses `Arc<dyn ScreenshotProvider>` as a trait object field (not a generic parameter) to avoid combinatorial explosion in the `Bookmarks` enum. This matches how `LoginProvider` is handled in `AppState`. The `Bookmarks` enum and `with_bookmarks!` macro remain unchanged.

- [ ] **Step 1: Update BookmarkService struct to use Arc\<dyn ScreenshotProvider\>**

In `server/src/app/bookmarks.rs`, add the import at the top:
```rust
use crate::domain::ports::screenshot::ScreenshotProvider;
```

Change the struct definition (lines 18-24) from:
```rust
pub struct BookmarkService<R, M, S> {
    repo: Arc<R>,
    metadata: Arc<M>,
    storage: Arc<S>,
    http_client: reqwest::Client,
    screenshot_service_url: Option<String>,
}
```

To:
```rust
pub struct BookmarkService<R, M, S> {
    repo: Arc<R>,
    metadata: Arc<M>,
    storage: Arc<S>,
    screenshot: Arc<dyn ScreenshotProvider>,
    http_client: reqwest::Client,
}
```

The impl block bounds (lines 26-30) stay the same — 3 generic params, no change.

- [ ] **Step 2: Update the constructor**

Change the `new()` function (lines 32-49) from:
```rust
    pub fn new(
        repo: Arc<R>,
        metadata: Arc<M>,
        storage: Arc<S>,
        screenshot_service_url: Option<String>,
    ) -> Self {
        let http_client = reqwest::Client::builder()
            .user_agent("Boopmark/1.0 (+https://boopmark.app)")
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");
        Self {
            repo,
            metadata,
            storage,
            http_client,
            screenshot_service_url,
        }
    }
```

To:
```rust
    pub fn new(
        repo: Arc<R>,
        metadata: Arc<M>,
        storage: Arc<S>,
        screenshot: Arc<dyn ScreenshotProvider>,
    ) -> Self {
        let http_client = reqwest::Client::builder()
            .user_agent("Boopmark/1.0 (+https://boopmark.app)")
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");
        Self {
            repo,
            metadata,
            storage,
            screenshot,
            http_client,
        }
    }
```

- [ ] **Step 3: Update the create() screenshot call site**

In `server/src/app/bookmarks.rs`, replace the screenshot block (lines 66-78):

From:
```rust
        if input.image_url.is_none()
            && let Some(svc_url) = &self.screenshot_service_url
        {
            let client =
                crate::adapters::screenshot::ScreenshotClient::new(svc_url.clone());
            if let Ok(bytes) = client.capture(&input.url).await {
                let key = format!("images/{}.jpg", Uuid::new_v4());
                if let Ok(stored_url) = self.storage.put(&key, bytes, "image/jpeg").await {
                    input.image_url = Some(stored_url);
                }
            }
        }
```

To:
```rust
        if input.image_url.is_none() {
            if let Ok(bytes) = self.screenshot.capture(&input.url).await {
                let key = format!("images/{}.jpg", Uuid::new_v4());
                if let Ok(stored_url) = self.storage.put(&key, bytes, "image/jpeg").await {
                    input.image_url = Some(stored_url);
                }
            }
        }
```

Note: When `NoopScreenshot` is active, `capture()` returns `Err` which is silently handled by `if let Ok(...)`, so this works correctly for both enabled and disabled states.

- [ ] **Step 4: Update the fetch_and_store_image() call site**

In `server/src/app/bookmarks.rs`, replace the screenshot block (lines 398-409):

From:
```rust
        let svc_url = self
            .screenshot_service_url
            .as_deref()
            .ok_or_else(|| DomainError::Internal("no screenshot svc".into()))?;

        let screenshot_client =
            crate::adapters::screenshot::ScreenshotClient::new(svc_url.to_string());
        let bytes = screenshot_client.capture(page_url).await?;

        let key = format!("images/{}.jpg", Uuid::new_v4());
        self.storage.put(&key, bytes, "image/jpeg").await
```

To:
```rust
        let bytes = self.screenshot.capture(page_url).await?;

        let key = format!("images/{}.jpg", Uuid::new_v4());
        self.storage.put(&key, bytes, "image/jpeg").await
```

- [ ] **Step 5: Do NOT commit yet**

The code will not compile until Task 4 updates `main.rs`. Proceed directly to Task 4 — both tasks will be committed together.

---

### Task 4: Add ScreenshotBackend Config and Wire into main.rs

**Files:**
- Modify: `server/src/config.rs:10-30,38-78`
- Modify: `server/src/main.rs:1-25,51-119`

- [ ] **Step 1: Add ScreenshotBackend enum to config.rs**

In `server/src/config.rs`, add after the `StorageBackend` enum (after line 36):

```rust
#[derive(Debug, Clone)]
pub enum ScreenshotBackend {
    Playwright,
    Disabled,
}
```

- [ ] **Step 2: Replace screenshot_service_url with screenshot_backend in Config struct**

In the `Config` struct (line 29), replace:
```rust
    pub screenshot_service_url: Option<String>,
```

With:
```rust
    pub screenshot_backend: ScreenshotBackend,
    pub screenshot_service_url: Option<String>,
```

- [ ] **Step 3: Update from_env() to parse SCREENSHOT_BACKEND**

In `from_env()`, replace the screenshot_service_url line (line 76):
```rust
            screenshot_service_url: env::var("SCREENSHOT_SERVICE_URL").ok(),
```

With:
```rust
            screenshot_backend: match env::var("SCREENSHOT_BACKEND")
                .unwrap_or_else(|_| "disabled".into())
                .as_str()
            {
                "playwright" => ScreenshotBackend::Playwright,
                _ => ScreenshotBackend::Disabled,
            },
            screenshot_service_url: env::var("SCREENSHOT_SERVICE_URL").ok(),
```

- [ ] **Step 4: Update main.rs imports**

In `server/src/main.rs`, add these imports (there is no existing `ScreenshotClient` import to remove — it was used inline via full path):
```rust
use adapters::screenshot::noop::NoopScreenshot;
use adapters::screenshot::playwright::PlaywrightScreenshot;
use config::ScreenshotBackend;
use domain::ports::screenshot::ScreenshotProvider;
```

- [ ] **Step 5: Build the screenshot provider in main.rs**

In `server/src/main.rs`, add screenshot provider construction before the storage backend match (before line 51). Insert after `let metadata_for_enrichment = metadata.clone();` (line 49):

```rust
    let screenshot: Arc<dyn ScreenshotProvider> = match config.screenshot_backend {
        ScreenshotBackend::Playwright => {
            let url = config
                .screenshot_service_url
                .clone()
                .expect("SCREENSHOT_SERVICE_URL required when SCREENSHOT_BACKEND=playwright");
            Arc::new(PlaywrightScreenshot::new(url))
        }
        ScreenshotBackend::Disabled => Arc::new(NoopScreenshot),
    };
```

- [ ] **Step 6: Update BookmarkService construction in main.rs**

In the `StorageBackend::Local` arm (around line 62-68), change:
```rust
                Bookmarks::Local(Arc::new(BookmarkService::new(
                    db.clone(),
                    metadata,
                    storage,
                    config.screenshot_service_url.clone(),
                ))),
```

To:
```rust
                Bookmarks::Local(Arc::new(BookmarkService::new(
                    db.clone(),
                    metadata,
                    storage,
                    screenshot.clone(),
                ))),
```

Do the same for the `StorageBackend::S3` arm (around line 110-115):
```rust
                Bookmarks::S3(Arc::new(BookmarkService::new(
                    db.clone(),
                    metadata,
                    storage,
                    screenshot.clone(),
                ))),
```

- [ ] **Step 7: Verify it compiles and all tests pass (including Task 2 screenshot adapter tests)**

Run: `cargo check -p boopmark-server && cargo test -p boopmark-server`
Expected: All tests pass, including 3 new screenshot adapter tests from Task 2

- [ ] **Step 8: Commit (includes Tasks 2 and 3 changes)**

This commit includes all changes from Tasks 2, 3, and 4 — the new adapter files, `bookmarks.rs` wiring, config changes, and `main.rs` updates:

```bash
git rm server/src/adapters/screenshot.rs
git add server/src/adapters/screenshot/ server/src/app/bookmarks.rs server/src/config.rs server/src/main.rs
git commit -m "feat: add Playwright and Noop screenshot adapters, wire ScreenshotBackend config"
```

---

### Task 5: Update Existing BookmarkService Tests

**Files:**
- Modify: `server/src/app/bookmarks.rs` (test module at bottom of file)

- [ ] **Step 1: Check existing tests**

Read the test module at the bottom of `server/src/app/bookmarks.rs` to find all test functions that construct `BookmarkService`. Each one currently passes `screenshot_service_url: Option<String>` — these need to pass `Arc<dyn ScreenshotProvider>` (or `Arc::new(NoopScreenshot)`) instead.

- [ ] **Step 2: Update test constructors**

There are two test helper functions that construct `BookmarkService`:
- `make_service` (around line 720) — passes `None` as the 4th arg
- `make_service_with_failing_upsert` (around line 731) — passes `None` as the 4th arg

Add the import to the test module:
```rust
use crate::adapters::screenshot::noop::NoopScreenshot;
```

In both helpers, change the 4th argument from `None` to `Arc::new(NoopScreenshot)`:
```rust
BookmarkService::new(repo, metadata, storage, Arc::new(NoopScreenshot))
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p boopmark-server`
Expected: All tests pass

- [ ] **Step 4: Commit**

```bash
git add server/src/app/bookmarks.rs
git commit -m "test: update BookmarkService tests to use NoopScreenshot adapter"
```

---

## Chunk 2: Config, Docker Compose & Infrastructure

### Task 6: Change Login Adapter Default to Local Password

**Files:**
- Modify: `server/src/config.rs:57`

- [ ] **Step 1: Change the LOGIN_ADAPTER default**

In `server/src/config.rs`, in the `from_env()` function, change:
```rust
            login_adapter: match env::var("LOGIN_ADAPTER")
                .unwrap_or_else(|_| "google".into())
                .as_str()
```

To:
```rust
            login_adapter: match env::var("LOGIN_ADAPTER")
                .unwrap_or_else(|_| "local_password".into())
                .as_str()
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p boopmark-server`
Expected: SUCCESS

- [ ] **Step 3: Commit**

```bash
git add server/src/config.rs
git commit -m "feat: default LOGIN_ADAPTER to local_password for self-hosting"
```

---

### Task 7: Replace MinIO with RustFS in Docker Compose

**Files:**
- Modify: `docker-compose.yml`

- [ ] **Step 1: Replace the minio service with a commented-out rustfs block**

In `docker-compose.yml`, replace the `minio` service block (lines 18-26):

```yaml
  minio:
    image: minio/minio
    command: server /data --console-address ":9001"
    environment:
      MINIO_ROOT_USER: minioadmin
      MINIO_ROOT_PASSWORD: minioadmin
    ports:
      - "9000:9000"
      - "9001:9001"
```

With a commented-out RustFS block:

```yaml
  # Uncomment to enable S3-compatible storage (set STORAGE_BACKEND=s3 in .env)
  # rustfs:
  #   image: rustfs/rustfs
  #   command: server /data --console-address ":9001"
  #   environment:
  #     RUSTFS_ROOT_USER: rustfsadmin
  #     RUSTFS_ROOT_PASSWORD: rustfsadmin
  #   ports:
  #     - "9000:9000"
  #     - "9001:9001"
```

- [ ] **Step 2: Make screenshot-svc optional (commented out)**

Replace the `screenshot-svc` block (lines 28-33) with:

```yaml
  # Uncomment to enable screenshot capture (set SCREENSHOT_BACKEND=playwright in .env)
  # screenshot-svc:
  #   build: ./screenshot-svc
  #   ports:
  #     - "3001:3001"
  #   environment:
  #     PORT: "3001"
```

- [ ] **Step 3: Update server service dependencies and environment**

In the `server` service, update the environment block to remove hardcoded S3 and screenshot values. Change:

```yaml
    environment:
      DATABASE_URL: postgres://boopmark:devpassword@db:5432/boopmark
      S3_ENDPOINT: http://minio:9000
      LOGIN_ADAPTER: "local_password"
      SCREENSHOT_SERVICE_URL: http://screenshot-svc:3001
```

To:

```yaml
    environment:
      DATABASE_URL: postgres://boopmark:devpassword@db:5432/boopmark
      LOGIN_ADAPTER: "local_password"
```

Update `depends_on` to remove `minio` and `screenshot-svc`:

```yaml
    depends_on:
      db:
        condition: service_healthy
```

- [ ] **Step 4: Verify docker compose config is valid**

Run: `docker compose config --quiet`
Expected: No errors

- [ ] **Step 5: Commit**

```bash
git add docker-compose.yml
git commit -m "feat: simplify docker-compose for self-hosting — MinIO and screenshot-svc now optional"
```

---

### Task 8: Add Tailwind CSS Build Stage to Dockerfile

**Files:**
- Modify: `Dockerfile`

- [ ] **Step 1: Add Node 24 CSS build stage**

In `Dockerfile`, add a CSS build stage before the existing `builder` stage. Insert at the top of the file (before line 1):

```dockerfile
FROM node:24-slim AS css
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY tailwind.config.js ./
COPY static/css/input.css static/css/input.css
COPY templates/ templates/
RUN npx tailwindcss -i static/css/input.css -o static/css/output.css --minify

```

- [ ] **Step 2: Build hash_password example binary**

In the Rust builder stage, after the **second** `cargo build --release -p boopmark-server` (the one after `COPY server/ server/` and `RUN touch server/src/main.rs`, currently line 13 — this is the real build, not the cache-warming build on line 8), add:
```dockerfile
RUN cargo build --release -p boopmark-server --example hash_password
```

- [ ] **Step 3: Copy built CSS and hash_password into the final stage**

In the final stage (the `debian:trixie-slim` section), change:
```dockerfile
COPY static/ static/
```

To:
```dockerfile
COPY --from=builder /app/target/release/examples/hash_password .
COPY static/ static/
COPY --from=css /app/static/css/output.css static/css/output.css
```

This copies the full static dir first (for JS, images, etc.), then overwrites `output.css` with the freshly built version. The `hash_password` binary is needed for Docker-only user creation via `add-user.sh`.

- [ ] **Step 4: Verify Docker build works**

Run: `docker compose build server`
Expected: Build succeeds with the CSS stage and hash_password binary

- [ ] **Step 5: Commit**

```bash
git add Dockerfile
git commit -m "feat: add Node 24 Tailwind CSS build stage and hash_password binary to Dockerfile"
```

---

### Task 9: Add MIT License and Cargo.toml License Fields

**Files:**
- Create: `LICENSE`
- Modify: `Cargo.toml` (workspace root)
- Modify: `server/Cargo.toml`
- Modify: `cli/Cargo.toml`

- [ ] **Step 1: Create LICENSE file**

Create `LICENSE` in the repository root with the standard MIT license text. Use the current year (2026) and copyright holder "Chris Fenton" (or however the owner wants to be credited — check git log for the name).

```
MIT License

Copyright (c) 2026 Chris Fenton

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

- [ ] **Step 2: Add license field to workspace Cargo.toml**

In the workspace root `Cargo.toml`, add a `[workspace.package]` section (if it doesn't exist) with the license:

```toml
[workspace.package]
license = "MIT"
```

- [ ] **Step 3: Add license to server/Cargo.toml**

In `server/Cargo.toml`, add to the `[package]` section:
```toml
license.workspace = true
```

- [ ] **Step 4: Add license to cli/Cargo.toml**

In `cli/Cargo.toml`, add to the `[package]` section:
```toml
license.workspace = true
```

- [ ] **Step 5: Verify**

Run: `cargo check`
Expected: SUCCESS

- [ ] **Step 6: Commit**

```bash
git add LICENSE Cargo.toml server/Cargo.toml cli/Cargo.toml
git commit -m "chore: add MIT license"
```

---

## Chunk 3: User Management & Bootstrap

### Task 10: Rewrite add-user.sh with Flag Support

**Files:**
- Modify: `scripts/add-user.sh`

- [ ] **Step 1: Rewrite the script with proper flag parsing**

Replace the entire contents of `scripts/add-user.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

# Load .env if present
if [ -f .env ]; then
  set -a; source .env; set +a
fi

# Defaults
ROLE="user"
PASSWORD=""
EMAIL=""

# Parse arguments
while [[ $# -gt 0 ]]; do
  case "$1" in
    --role)
      ROLE="$2"
      shift 2
      ;;
    --password)
      PASSWORD="$2"
      shift 2
      ;;
    *)
      if [ -z "$EMAIL" ]; then
        EMAIL="$1"
      fi
      shift
      ;;
  esac
done

# Interactive fallback
if [ -z "$EMAIL" ]; then
  read -rp "Email: " EMAIL
fi

# Validate role
case "$ROLE" in
  owner|admin|user) ;;
  *) echo "Error: --role must be owner, admin, or user" >&2; exit 1 ;;
esac

# Validate email
if [ -z "$EMAIL" ]; then
  echo "Error: email is required" >&2
  exit 1
fi

# Determine login adapter
LOGIN_ADAPTER="${LOGIN_ADAPTER:-local_password}"

# Password handling
if [ "$LOGIN_ADAPTER" = "local_password" ] && [ -z "$PASSWORD" ]; then
  read -rsp "Password: " PASSWORD
  echo
fi

if [ "$LOGIN_ADAPTER" = "local_password" ] && [ -z "$PASSWORD" ]; then
  echo "Error: password is required when LOGIN_ADAPTER=local_password" >&2
  exit 1
fi

# Find the db container
DB_CONTAINER=$(docker ps --filter "name=boopmark-db-1" --format "{{.Names}}" | head -1)
if [ -z "$DB_CONTAINER" ]; then
  # Fallback: try docker compose service name
  DB_CONTAINER=$(docker ps --filter "name=db-1" --format "{{.Names}}" | head -1)
fi
if [ -z "$DB_CONTAINER" ]; then
  echo "Error: no running boopmark db container found. Start services first." >&2
  exit 1
fi

# Owner guard
if [ "$ROLE" = "owner" ]; then
  EXISTING_OWNER=$(docker exec "$DB_CONTAINER" psql -U boopmark -d boopmark -t -c \
    "SELECT COUNT(*) FROM users WHERE role = 'owner' AND deactivated_at IS NULL;" | tr -d ' ')
  if [ "$EXISTING_OWNER" -gt 0 ]; then
    echo "Error: an owner already exists. Use the admin panel to manage users." >&2
    exit 1
  fi
fi

# Hash password if provided
HASH_CLAUSE="NULL"
if [ -n "$PASSWORD" ]; then
  echo "Hashing password..."
  if command -v cargo > /dev/null 2>&1; then
    HASH=$(cargo run -p boopmark-server --example hash_password -- "$PASSWORD" 2>/dev/null)
  else
    # Docker-only: exec into server container to hash
    SERVER_CONTAINER=$(docker ps --filter "name=server" --format "{{.Names}}" | head -1)
    if [ -z "$SERVER_CONTAINER" ]; then
      echo "Error: no running server container found and cargo not available." >&2
      exit 1
    fi
    HASH=$(docker exec "$SERVER_CONTAINER" ./hash_password "$PASSWORD")
  fi
  HASH_CLAUSE="'$HASH'"
fi

echo "Creating $ROLE user $EMAIL..."
docker exec "$DB_CONTAINER" psql -U boopmark -d boopmark \
  -c "INSERT INTO users (email, name, password_hash, role)
      VALUES ('$EMAIL', '$EMAIL', $HASH_CLAUSE, '$ROLE')
      ON CONFLICT (email) DO UPDATE SET
        password_hash = COALESCE(EXCLUDED.password_hash, users.password_hash),
        role = EXCLUDED.role;"

echo "Done! $ROLE user $EMAIL created."
```

- [ ] **Step 2: Test the script manually**

Ensure services are running, then test:

```bash
# Test creating a regular user (should work)
just add-user test@example.com --password testpass

# Test creating an owner (should work first time)
just add-user owner@example.com --password ownerpass --role owner

# Test creating a second owner (should fail with error)
just add-user owner2@example.com --password ownerpass --role owner
```

- [ ] **Step 3: Commit**

```bash
git add scripts/add-user.sh
git commit -m "feat: rewrite add-user.sh with --role and --password flags, owner guard"
```

---

### Task 11: Add Bootstrap Command to Justfile

**Files:**
- Modify: `justfile`

- [ ] **Step 1: Add the bootstrap command**

Add to the justfile (after the existing `setup` command):

```just
# One-command setup for self-hosters (Docker-only)
bootstrap *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail

    # Step 1: Create .env from example if it doesn't exist
    if [ ! -f .env ]; then
      echo "==> Creating .env from .env.example"
      cp .env.example .env

      # Generate random secrets
      echo "==> Generating secrets"
      SESSION_SECRET=$(openssl rand -hex 32)
      LLM_KEY=$(openssl rand -base64 32)
      if [[ "$OSTYPE" == "darwin"* ]]; then
        sed -i '' "s|^SESSION_SECRET=.*|SESSION_SECRET=$SESSION_SECRET|" .env
        sed -i '' "s|^LLM_SETTINGS_ENCRYPTION_KEY=.*|LLM_SETTINGS_ENCRYPTION_KEY=$LLM_KEY|" .env
      else
        sed -i "s|^SESSION_SECRET=.*|SESSION_SECRET=$SESSION_SECRET|" .env
        sed -i "s|^LLM_SETTINGS_ENCRYPTION_KEY=.*|LLM_SETTINGS_ENCRYPTION_KEY=$LLM_KEY|" .env
      fi
    else
      echo "==> .env already exists, skipping"
    fi

    # Step 2: Start services
    echo "==> Starting Docker services"
    docker compose up -d --build

    # Step 3: Wait for Postgres
    echo "==> Waiting for Postgres..."
    until docker compose exec db pg_isready -U boopmark > /dev/null 2>&1; do sleep 1; done

    # Step 4: Wait for server to be ready (it runs migrations on startup)
    echo "==> Waiting for server..."
    until curl -sf http://localhost:4000 > /dev/null 2>&1; do sleep 2; done

    # Step 5: Create owner user
    echo ""
    echo "==> Create your admin account"
    ./scripts/add-user.sh {{ARGS}} --role owner

    echo ""
    echo "==> Boopmark is ready at http://localhost:4000"
```

- [ ] **Step 2: Update the dev command**

Replace the existing `dev` command (lines 25-27):

```just
dev:
    docker compose up -d db minio
    cargo run -p boopmark-server
```

With:

```just
# Daily development driver (starts infra + runs server locally)
# Set USE_DEVPROXY=1 in .env to use devproxy instead of docker compose
dev:
    #!/usr/bin/env bash
    set -euo pipefail
    source .env 2>/dev/null || true
    if [ "${USE_DEVPROXY:-0}" = "1" ]; then
      devproxy up
    else
      docker compose up -d db
      cargo run -p boopmark-server
    fi
```

Note: This preserves the hybrid workflow (infra in Docker, server locally via cargo) as the default. `USE_DEVPROXY=1` switches to the full devproxy stack. Self-hosters use `just bootstrap` + `docker compose up -d` instead.

- [ ] **Step 3: Update the setup command for hybrid dev**

Replace the existing `setup` command to reflect new service names and workflow:

```just
# Contributor setup (hybrid: infra in Docker, server runs locally)
setup:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "==> Copying .env.example to .env (if needed)"
    [ -f .env ] || cp .env.example .env
    echo "==> Starting Docker services (db only)"
    docker compose up -d db
    echo "==> Waiting for Postgres..."
    until docker compose exec db pg_isready -U boopmark > /dev/null 2>&1; do sleep 1; done
    echo "==> Installing sqlx-cli (if needed)"
    command -v sqlx > /dev/null 2>&1 || cargo install sqlx-cli --no-default-features --features postgres,rustls
    echo "==> Running migrations"
    source .env
    sqlx migrate run --source migrations
    echo "==> Installing npm dependencies"
    npm install
    echo "==> Building Tailwind CSS"
    npx tailwindcss -i static/css/input.css -o static/css/output.css --minify
    echo "==> Building project"
    cargo build
    echo "==> Setup complete! Run 'cargo run -p boopmark-server' to start the server."
```

- [ ] **Step 4: Verify justfile syntax**

Run: `just --list`
Expected: Lists all commands including `bootstrap`, `dev`, `setup`

- [ ] **Step 5: Commit**

```bash
git add justfile
git commit -m "feat: add bootstrap command for self-hosters, update dev and setup commands"
```

---

## Chunk 4: Documentation

### Task 12: Rewrite .env.example

**Files:**
- Modify: `.env.example`

- [ ] **Step 1: Rewrite with grouped, commented env vars**

Replace the entire contents of `.env.example`:

```bash
# =============================================================================
# Boopmark Configuration
# =============================================================================
# Copy this file to .env and customize. Only DATABASE_URL, SESSION_SECRET, and
# LLM_SETTINGS_ENCRYPTION_KEY are required — everything else has sensible defaults.
#
# Quick start: run `just bootstrap` to auto-generate secrets and set up.
# =============================================================================

# --- Required ---

DATABASE_URL=postgres://boopmark:devpassword@localhost:5434/boopmark
SESSION_SECRET=change-me-in-production
LLM_SETTINGS_ENCRYPTION_KEY=MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=

# --- Server ---

APP_URL=http://localhost:4000
PORT=4000

# --- Authentication ---
# "local_password" (default) — email/password login
# "google" — Google OAuth (requires GOOGLE_CLIENT_ID and GOOGLE_CLIENT_SECRET)

LOGIN_ADAPTER=local_password
# GOOGLE_CLIENT_ID=your-google-client-id
# GOOGLE_CLIENT_SECRET=your-google-client-secret

# --- Storage ---
# "local" (default) — files stored on disk in ./uploads
# "s3" — S3-compatible storage (RustFS, AWS S3, Cloudflare R2, etc.)

STORAGE_BACKEND=local
# S3_ENDPOINT=http://localhost:9000
# S3_ACCESS_KEY=rustfsadmin
# S3_SECRET_KEY=rustfsadmin
# S3_REGION=us-east-1
# S3_IMAGES_BUCKET=boopmark-images
# S3_IMAGES_PUBLIC_URL=

# --- Screenshots ---
# "disabled" (default) — no screenshot capture
# "playwright" — requires a running screenshot-svc (see docker-compose.yml)

SCREENSHOT_BACKEND=disabled
# SCREENSHOT_SERVICE_URL=http://localhost:3001

# --- Development ---

ENABLE_E2E_AUTH=0
USE_DEVPROXY=0
```

- [ ] **Step 2: Commit**

```bash
git add .env.example
git commit -m "docs: rewrite .env.example with grouped, documented env vars"
```

---

### Task 13: Rewrite README.md

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Write the new README**

Replace the entire contents of `README.md` with a comprehensive open-source README. Include:

1. **Header** — project name, one-line description, license badge
2. **Features** — bullet list (bookmarks, tags, search, CLI, LLM enrichment, optional screenshots, invite-only access)
3. **Quick Start (Self-Hosting)** — 3 steps using `just bootstrap`
4. **Configuration** — table of all env vars with defaults and descriptions (reference `.env.example`)
5. **Deployment Guides**
   - Docker Compose (primary) — the default `just bootstrap` path
   - Railway + Neon — brief guide (provision Postgres, set env vars, deploy)
   - Optional features: enabling S3 storage (RustFS), enabling screenshots (Playwright sidecar)
6. **CLI (`boop`)** — install + configure + usage examples
7. **Development** — prerequisites, `just setup`, `just dev`, running tests
8. **Architecture** — brief hex architecture overview, ports & adapters list, link to deeper docs
9. **Contributing** — link to CONTRIBUTING.md
10. **License** — MIT

Keep it concise but complete. Target ~150-200 lines.

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: rewrite README for open-source release"
```

---

### Task 14: Create CONTRIBUTING.md

**Files:**
- Create: `CONTRIBUTING.md`

- [ ] **Step 1: Write the contributing guide**

Create `CONTRIBUTING.md` covering:

1. **Getting Started**
   - Prerequisites: Rust (stable), Node.js 24+, Docker
   - Fork & clone
   - `just setup` for hybrid development
   - `just dev` to start services, `cargo run -p boopmark-server` for the server
   - `just css` for Tailwind watch mode

2. **Project Structure** — brief overview of workspace layout (`server/`, `cli/`, `screenshot-svc/`, `templates/`, `static/`, `migrations/`)

3. **Architecture** — hex architecture explained:
   - Ports in `server/src/domain/ports/` (trait definitions)
   - Adapters in `server/src/adapters/` (implementations)
   - Adding a new adapter: create the impl, register in `adapters/mod.rs`, add config enum variant, wire in `main.rs`

4. **Testing**
   - `cargo test` for unit/integration tests
   - `npx playwright test tests/e2e/suggest.spec.js` for E2E
   - Write tests for new adapters following existing patterns

5. **Pull Requests**
   - Keep PRs focused (one feature/fix per PR)
   - Include tests
   - Follow existing code style

- [ ] **Step 2: Commit**

```bash
git add CONTRIBUTING.md
git commit -m "docs: add CONTRIBUTING.md for open-source contributors"
```

---

### Task 15: Clean Up Stale References

**Files:**
- Modify: `AGENTS.md` (the canonical file — `CLAUDE.md` is a symlink to it)

- [ ] **Step 1: Update stale ENABLE_LOCAL_AUTH references**

`AGENTS.md` (line 24) references `ENABLE_LOCAL_AUTH=1` which was replaced by `LOGIN_ADAPTER`. Update the "Local Auth (Development)" section in `AGENTS.md` to reference `LOGIN_ADAPTER=local_password` instead. Do NOT edit `CLAUDE.md` directly — it is a symlink to `AGENTS.md`.

Also search for any other stale references:

Run: `grep -r "ENABLE_LOCAL_AUTH" . --include="*.md" --exclude-dir=node_modules --exclude-dir=target --exclude-dir=docs/superpowers`

Update any remaining hits in non-historical documentation. `README.md` is already rewritten in Task 13.

- [ ] **Step 2: Commit**

```bash
git add AGENTS.md
git commit -m "chore: clean up stale ENABLE_LOCAL_AUTH references"
```

---

## Chunk 5: Final Verification

### Task 16: End-to-End Verification

- [ ] **Step 1: Run the full test suite**

Run: `cargo test`
Expected: All tests pass

- [ ] **Step 2: Verify Docker build**

Run: `docker compose build`
Expected: Build succeeds (including CSS stage)

- [ ] **Step 3: Verify docker compose config**

Run: `docker compose config --quiet`
Expected: No errors

- [ ] **Step 4: Verify clippy**

Run: `cargo clippy -p boopmark-server --all-targets -- -D warnings`
Expected: No warnings

- [ ] **Step 5: Test bootstrap flow manually (if possible)**

Start fresh:
```bash
docker compose down -v
rm -f .env
just bootstrap test@example.com --password testpass
```

Verify the server starts and you can log in at http://localhost:4000.

- [ ] **Step 6: Final commit if any fixes needed**

```bash
git add -A
git commit -m "chore: final verification fixes for open-source release"
```
