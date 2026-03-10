# Store Google Avatar Locally Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Download and cache Google OAuth profile images in an S3-compatible images bucket so the app never hotlinks `lh3.googleusercontent.com`, avoiding 429 rate-limit errors.

**Architecture:** Add a second `ObjectStorage` instance (`images_storage`) dedicated to an images bucket. During the Google OAuth callback, after fetching userinfo, download the avatar image, store it in the images bucket, and save the stored URL in the `users.image` column instead of the raw Google URL. The `AuthService` gains a new dependency on `ObjectStorage` (or a small `ImageStore` helper) so the auth flow can store images. Existing bookmark preview image storage is unaffected.

**Tech Stack:** Rust, Axum, aws-sdk-s3, reqwest, SQLx, Askama templates

---

### Task 1: Add images bucket config fields

**Files:**
- Modify: `server/src/config.rs`

**Step 1: Add the new config fields**

Add `s3_images_bucket` to `Config`:

```rust
// In Config struct, after s3_public_url:
pub s3_images_bucket: String,
```

In `Config::from_env()`, after the `s3_public_url` line:

```rust
s3_images_bucket: env::var("S3_IMAGES_BUCKET").unwrap_or_else(|_| "boopmark-images".into()),
```

**Step 2: Verify it compiles**

Run: `cargo build -p boopmark-server`
Expected: compiles with no errors

**Step 3: Commit**

```bash
git add server/src/config.rs
git commit -m "feat: add S3_IMAGES_BUCKET config field"
```

---

### Task 2: Wire up a second ObjectStorage for images in main.rs and AppState

**Files:**
- Modify: `server/src/main.rs`
- Modify: `server/src/web/state.rs`

**Step 1: Add `images_storage` to AppState**

In `server/src/web/state.rs`, add a field to `AppState`:

```rust
use crate::domain::ports::storage::ObjectStorage;

pub struct AppState {
    pub bookmarks: Bookmarks,
    pub auth: Arc<AuthService<PostgresPool, PostgresPool, PostgresPool>>,
    pub settings: Arc<SettingsService<PostgresPool>>,
    pub config: Arc<Config>,
    pub enricher: Arc<dyn LlmEnricher>,
    pub images_storage: Arc<dyn ObjectStorage>,
}
```

**Step 2: Construct the images storage in main.rs**

In `server/src/main.rs`, after creating the bookmark storage, construct a second storage backend for images using the same S3 client / local pattern but with `config.s3_images_bucket`:

```rust
let images_storage: Arc<dyn ObjectStorage> = match config.storage_backend {
    StorageBackend::Local => Arc::new(LocalStorage::new(
        "./uploads/images".into(),
        format!("{}/uploads/images", config.app_url),
    )),
    StorageBackend::S3 => {
        // Reuse the same aws_config — just need a new S3Storage with images bucket
        let s3_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .load()
            .await;
        let s3_client = aws_sdk_s3::Client::new(&s3_config);
        Arc::new(S3Storage::new(
            s3_client,
            config.s3_images_bucket.clone(),
            config
                .s3_public_url
                .clone()
                .map(|u| u.replace(&config.s3_bucket, &config.s3_images_bucket))
                .unwrap_or_else(|| {
                    format!("https://{}.s3.amazonaws.com", config.s3_images_bucket)
                }),
        ))
    }
};
```

Then pass `images_storage` into the `AppState` constructor:

```rust
let state = AppState {
    bookmarks,
    auth: auth_service,
    settings: settings_service,
    config: Arc::new(config.clone()),
    enricher,
    images_storage,
};
```

**Step 3: Verify it compiles**

Run: `cargo build -p boopmark-server`
Expected: compiles with no errors

**Step 4: Commit**

```bash
git add server/src/main.rs server/src/web/state.rs
git commit -m "feat: wire up images storage backend in AppState"
```

---

### Task 3: Download and store the avatar during OAuth callback

**Files:**
- Modify: `server/src/web/pages/auth.rs`

**Step 1: Add avatar download logic to `google_callback`**

After fetching `userinfo` and before calling `state.auth.upsert_user`, download the Google avatar image and store it in the images bucket. Replace the raw Google URL with the stored URL:

```rust
// Download and cache avatar image
let stored_image = if let Some(ref picture_url) = userinfo.picture {
    let client = reqwest::Client::new();
    match client.get(picture_url).send().await {
        Ok(resp) if resp.status().is_success() => {
            let content_type = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("image/jpeg")
                .to_string();
            match resp.bytes().await {
                Ok(bytes) => {
                    let ext = match content_type.split(';').next().unwrap_or("").trim() {
                        "image/png" => "png",
                        "image/gif" => "gif",
                        "image/webp" => "webp",
                        _ => "jpg",
                    };
                    let key = format!("avatars/{}.{}", uuid::Uuid::new_v4(), ext);
                    match state.images_storage.put(&key, bytes.to_vec(), &content_type).await {
                        Ok(url) => Some(url),
                        Err(e) => {
                            tracing::warn!("Failed to store avatar: {e}");
                            userinfo.picture.clone()
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to read avatar bytes: {e}");
                    userinfo.picture.clone()
                }
            }
        }
        _ => userinfo.picture.clone(),
    }
} else {
    None
};

// Upsert user and create session
let user = state
    .auth
    .upsert_user(userinfo.email, userinfo.name, stored_image)
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
```

Note: On any failure downloading/storing the avatar, we fall back to the Google URL (graceful degradation). Add `use uuid::Uuid;` at the top if not already imported.

**Step 2: Verify it compiles**

Run: `cargo build -p boopmark-server`
Expected: compiles with no errors

**Step 3: Commit**

```bash
git add server/src/web/pages/auth.rs
git commit -m "feat: download and store Google avatar in images bucket on OAuth login"
```

---

### Task 4: E2E verification with agent-browser

**Files:** None (manual testing)

**Step 1: Start the local dev stack**

```bash
docker compose up -d
cargo run -p boopmark-server
```

**Step 2: Navigate to the app and log in via Google**

Use agent-browser (Playwright MCP) to:
1. Navigate to `http://localhost:4000`
2. Take a screenshot showing the login page
3. **PAUSE for human-in-the-loop** — the user needs to log in via Google manually
4. After login, take a screenshot showing the profile avatar loading from the local/S3 images bucket URL (not from `lh3.googleusercontent.com`)
5. Inspect the `<img>` tag `src` attribute to confirm it points to the images bucket URL

**Step 3: Verify no 429 errors**

Check browser console / network tab for 429 errors on avatar images. Take screenshot as proof.

**Step 4: Commit screenshots as proof**

```bash
git add screenshot-*.png
git commit -m "test: add E2E screenshots proving avatar is served from images bucket"
```
