# API & CLI LLM Enrichment — Implementation Plan

**Goal:** Add LLM-powered bookmark enrichment (title, description, tags) to the REST API and CLI, matching the web app's existing capability.

**Architecture:** Extract the enrichment logic currently inlined in `web/pages/bookmarks.rs` (`try_llm_enrich` function, lines 304-334) into a new app-layer `EnrichmentService`. Both page handlers and API handlers call this service. The API auto-enriches on create when the user has LLM enabled and fields are missing. The API enriches on update when `?suggest=true` is passed. A new `POST /api/v1/bookmarks/suggest` endpoint returns suggestions without saving. The CLI gains an `edit` command with `--suggest`, and the `add` command gains `--description` and richer output.

---

## Design Decisions

### D1: EnrichmentService owns both scrape and LLM enrichment

**Rationale:** The current `try_llm_enrich` in `pages/bookmarks.rs` depends on `AppState` directly (accessing `state.settings` and `state.enricher`). Extracting this to an app-layer service is the correct hexagonal architecture move — it removes LLM orchestration logic from the web layer and makes it reusable by both page and API handlers. The service encapsulates the full scrape-then-enrich flow, so callers don't need to know about `MetadataExtractor`, `LlmEnricher`, or `SettingsService` individually.

### D2: EnrichmentService is generic over MetadataExtractor and LlmSettingsRepository

**Rationale:** Matching the existing codebase pattern (e.g., `BookmarkService<R, M, S>` is generic over its dependencies). The `LlmEnricher` trait uses dynamic dispatch (`Arc<dyn LlmEnricher>`) because it's already designed that way in the codebase, so `EnrichmentService` takes `Arc<dyn LlmEnricher>` rather than being generic over it. This avoids changing the existing `LlmEnricher` trait to be `Sized`.

### D3: Remove `enricher` field from AppState after migration

**Rationale:** After `EnrichmentService` is wired in, nothing else in the codebase accesses `state.enricher` directly. Keeping it would create two paths to the same functionality. Removing it enforces a single entry point for enrichment.

### D4: Auto-enrich on API create is opt-out (automatic when user has LLM configured)

**Rationale:** This matches the web app behavior — the suggest flow fires on URL blur. API users get the same benefit automatically. If the user hasn't configured an LLM API key, `EnrichmentService.suggest()` gracefully returns scrape-only results (no error). Users who send all fields pre-populated will skip enrichment because the condition `input.title.is_none() || input.description.is_none() || input.tags.as_ref().map_or(true, |t| t.is_empty())` won't be true.

### D5: The `Bookmarks` enum and `with_bookmarks!` macro approach stays as-is

**Rationale:** `EnrichmentService` doesn't need to interact with `BookmarkService` at all — it only needs `MetadataExtractor`, `LlmEnricher`, and `SettingsService`, none of which vary by storage backend. So `EnrichmentService` doesn't need the enum dispatch pattern and can be a single `Arc<EnrichmentService<...>>` on `AppState`.

### D6: `metadata` (the `Arc<HtmlMetadataExtractor>`) must be cloned before being moved into `BookmarkService::new()`

**Rationale:** Currently in `main.rs`, `metadata` is moved into `BookmarkService::new()`. `EnrichmentService` also needs it. The fix is simple: clone `metadata` before the `BookmarkService` construction. Both `Local` and `S3` branches need this change.

---

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `server/src/app/enrichment.rs` | App-layer `EnrichmentService` — owns scrape + LLM enrich logic |
| Modify | `server/src/app/mod.rs` | Export new `enrichment` module |
| Modify | `server/src/web/state.rs` | Add `EnrichmentService` to `AppState`, remove `enricher` field |
| Modify | `server/src/main.rs` | Wire `EnrichmentService` into `AppState`, clone `metadata` before `BookmarkService`, stop passing `enricher` directly |
| Modify | `server/src/web/api/bookmarks.rs` | Add suggest endpoint, enrich on create, optional enrich on update |
| Modify | `server/src/web/pages/bookmarks.rs` | Replace inline `try_llm_enrich` with calls to `EnrichmentService` |
| Modify | `cli/src/main.rs` | Add `edit` command with `--suggest` flag, add `--description` to `add`, add `suggest` command, richer output |

---

## Task 1: Create EnrichmentService

This service encapsulates the scrape-then-enrich flow that currently lives in `web/pages/bookmarks.rs:304-334`. It depends on three existing ports: `MetadataExtractor`, `LlmEnricher`, and `SettingsService`.

**Files:**
- Create: `server/src/app/enrichment.rs`
- Modify: `server/src/app/mod.rs`

### Step 1.1: Create `server/src/app/enrichment.rs`

Define `SuggestionResult` struct (with `Serialize` derive for JSON responses) and `EnrichmentService` struct. The service has a single public method `suggest(user_id, url, existing_tags) -> SuggestionResult` that:

1. Scrapes metadata via `MetadataExtractor::extract(url)` (skipping if URL is blank)
2. Attempts LLM enrichment via `try_llm_enrich` (checking settings, building `EnrichmentInput`, calling `LlmEnricher::enrich`)
3. Merges results: LLM takes priority over scrape for title/description/tags; image_url and domain come from scrape only

The `SuggestionResult` fields:
```rust
pub struct SuggestionResult {
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub image_url: Option<String>,
    pub domain: Option<String>,
}
```

The service generic signature:
```rust
pub struct EnrichmentService<M, R> {
    metadata: Arc<M>,
    enricher: Arc<dyn LlmEnricher>,
    settings: Arc<SettingsService<R>>,
}
```

With bounds: `M: MetadataExtractor + Send + Sync`, `R: LlmSettingsRepository + Send + Sync`.

The `try_llm_enrich` private method is extracted verbatim from `web/pages/bookmarks.rs:304-334`, adjusted to use `self.settings` and `self.enricher` instead of `state.settings` and `state.enricher`.

### Step 1.2: Add module to `server/src/app/mod.rs`

Add `pub mod enrichment;` to the module list.

### Step 1.3: Verify compilation

Run: `cargo check -p boopmark-server`

---

## Task 2: Wire EnrichmentService into AppState and migrate page handlers

This task does three things atomically to avoid an intermediate state where the old `enricher` field is orphaned:

1. Adds `EnrichmentService` to `AppState`
2. Migrates page handlers to use it
3. Removes the now-unused `enricher` field from `AppState`

**Files:**
- Modify: `server/src/web/state.rs`
- Modify: `server/src/main.rs`
- Modify: `server/src/web/pages/bookmarks.rs`

### Step 2.1: Modify `server/src/web/state.rs`

Add to `AppState`:
```rust
pub enrichment: Arc<EnrichmentService<HtmlMetadataExtractor, PostgresPool>>,
```

Remove the existing field:
```rust
pub enricher: Arc<dyn LlmEnricher>,  // DELETE THIS LINE
```

Add necessary imports: `use crate::app::enrichment::EnrichmentService;`

### Step 2.2: Modify `server/src/main.rs`

**Critical detail:** `metadata` is currently moved into `BookmarkService::new()` on both the `Local` and `S3` branches. We need to clone it first for `EnrichmentService`.

In both the `StorageBackend::Local` and `StorageBackend::S3` match arms:
- Before creating `BookmarkService::new(db.clone(), metadata, storage)`, clone metadata: `let metadata_for_enrichment = metadata.clone();`
- Then pass `metadata` (owned) into `BookmarkService` as before.

After the match block (where `enricher` is created on line 101), create `EnrichmentService`:
```rust
let enrichment_service = Arc::new(EnrichmentService::new(
    metadata_for_enrichment,
    enricher.clone(),
    settings_service.clone(),
));
```

**Wait — `metadata_for_enrichment` is created inside the match arms but `EnrichmentService` is created after them.** Solution: move `metadata_for_enrichment` out by declaring it before the match and assigning inside each arm. Or simpler: clone metadata before the match:

```rust
let metadata = Arc::new(HtmlMetadataExtractor::new());
let metadata_for_enrichment = metadata.clone();  // ADD THIS LINE

let (bookmarks, images_storage) = match config.storage_backend {
    // ... both arms use `metadata` as before
};
```

Then construct `EnrichmentService` using `metadata_for_enrichment`.

Update the `AppState` struct literal:
- Add `enrichment: enrichment_service,`
- Remove `enricher,` line (since the field no longer exists on `AppState`)

Note: `enricher` is still created (line 101) and used only by `EnrichmentService` now. Keep the variable, just don't add it to `AppState`.

### Step 2.3: Migrate `suggest()` handler in `server/src/web/pages/bookmarks.rs`

Replace lines 261-302 (the `suggest()` handler body). Instead of calling `with_bookmarks!` for metadata extraction and then calling the local `try_llm_enrich`, call:

```rust
let result = state.enrichment.suggest(user.id, &form.url, None).await;
```

Then build `SuggestFields` from `result`:
- `suggest_title`: use `fill_if_blank(form.title, result.title)`
- `suggest_description`: use `fill_if_blank(form.description, result.description)`
- `suggest_preview_image_url`: `result.image_url`
- `suggest_tags`: preserve user-typed tags logic — if `form.tags_input` is non-empty, use that; otherwise use `result.tags.join(", ")`

### Step 2.4: Migrate `edit_suggest()` handler in `server/src/web/pages/bookmarks.rs`

Replace lines 432-485. Instead of calling `with_bookmarks!` for metadata extraction and `try_llm_enrich`:

1. Keep: Get bookmark by ID (lines 439-442)
2. Keep: Get `tags_with_counts` (lines 450-453)
3. Replace: Call `state.enrichment.suggest(user.id, &bookmark.url, existing_tags).await`
4. Build `EditSuggestFields` from result, with the same fallback-to-form-values logic

### Step 2.5: Delete the `try_llm_enrich` private function (lines 304-334)

It now lives in `EnrichmentService`. Also remove the now-unused imports: `EnrichmentInput`, `EnrichmentOutput` from the `use` statements at the top of the file, and `UrlMetadata` if no longer used.

### Step 2.6: Verify compilation and tests

Run: `cargo test`

All existing tests must pass. The page handlers now delegate to `EnrichmentService`.

---

## Task 3: Add API enrichment endpoints

**Files:**
- Modify: `server/src/web/api/bookmarks.rs`

### Step 3.1: Add `POST /api/v1/bookmarks/suggest` endpoint

Add request/response types:
```rust
#[derive(Deserialize)]
struct SuggestRequest {
    url: String,
}

#[derive(Serialize)]
struct SuggestResponse {
    title: Option<String>,
    description: Option<String>,
    tags: Vec<String>,
    image_url: Option<String>,
    domain: Option<String>,
}
```

Add handler:
```rust
async fn suggest(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Json(input): Json<SuggestRequest>,
) -> impl IntoResponse {
    let result = state.enrichment.suggest(user.id, &input.url, None).await;
    Json(SuggestResponse {
        title: result.title,
        description: result.description,
        tags: result.tags,
        image_url: result.image_url,
        domain: result.domain,
    })
}
```

Register route in `routes()`:
```rust
.route("/suggest", post(suggest))
```

### Step 3.2: Auto-enrich on `create_bookmark`

Modify the existing `create_bookmark` handler. Before calling `with_bookmarks!`, check if fields are missing and enrich:

```rust
async fn create_bookmark(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Json(mut input): Json<CreateBookmark>,
) -> impl IntoResponse {
    // Auto-enrich missing fields if user has LLM enabled
    if input.title.is_none() || input.description.is_none() || input.tags.as_ref().map_or(true, |t| t.is_empty()) {
        let suggestions = state.enrichment.suggest(user.id, &input.url, None).await;
        if input.title.is_none() {
            input.title = suggestions.title;
        }
        if input.description.is_none() {
            input.description = suggestions.description;
        }
        if input.tags.as_ref().map_or(true, |t| t.is_empty()) && !suggestions.tags.is_empty() {
            input.tags = Some(suggestions.tags);
        }
        if input.image_url.is_none() {
            input.image_url = suggestions.image_url;
        }
        if input.domain.is_none() {
            input.domain = suggestions.domain;
        }
    }

    let result = with_bookmarks!(&state.bookmarks, svc => svc.create(user.id, input).await);
    match result {
        Ok(bookmark) => Ok((StatusCode::CREATED, Json(bookmark))),
        Err(e) => Err(error_response(e)),
    }
}
```

**Note on double-scrape avoidance:** `EnrichmentService.suggest()` already scrapes metadata internally. `BookmarkService::create()` also scrapes via `needs_metadata()`. After enrichment fills in title/description/domain, `BookmarkService::create` will still see `image_url` as `None` if the scrape found an image (since `SuggestionResult.image_url` comes from the scrape but gets set on `input.image_url` in the enrichment merge above). Wait — actually the code above does set `input.image_url = suggestions.image_url`, so `BookmarkService::create`'s `needs_metadata()` check should short-circuit if all fields are filled. The only case where `BookmarkService` still scrapes is if enrichment found no image. This is acceptable — the duplicate scrape is harmless and the code is cleaner than threading scrape results through.

### Step 3.3: Optional enrich on `update_bookmark`

Add query params struct:
```rust
#[derive(Debug, Deserialize)]
struct UpdateParams {
    suggest: Option<bool>,
}
```

Modify handler signature to accept `Query(params): Query<UpdateParams>`:
```rust
async fn update_bookmark(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<UpdateParams>,
    Json(mut input): Json<UpdateBookmark>,
) -> impl IntoResponse {
    if params.suggest.unwrap_or(false) {
        // Get the existing bookmark to access its URL
        let bookmark = with_bookmarks!(&state.bookmarks, svc => svc.get(id, user.id).await);
        match bookmark {
            Ok(bm) => {
                let existing_tags = with_bookmarks!(&state.bookmarks, svc =>
                    svc.tags_with_counts(user.id).await
                ).ok();
                let suggestions = state.enrichment.suggest(user.id, &bm.url, existing_tags).await;
                if input.title.is_none() {
                    input.title = suggestions.title;
                }
                if input.description.is_none() {
                    input.description = suggestions.description;
                }
                if input.tags.as_ref().map_or(true, |t| t.is_empty()) && !suggestions.tags.is_empty() {
                    input.tags = Some(suggestions.tags);
                }
            }
            Err(e) => return Err(error_response(e)),
        }
    }

    let result = with_bookmarks!(&state.bookmarks, svc => svc.update(id, user.id, input).await);
    match result {
        Ok(bookmark) => Ok(Json(bookmark)),
        Err(e) => Err(error_response(e)),
    }
}
```

### Step 3.4: Verify compilation and tests

Run: `cargo test`

---

## Task 4: CLI changes

**Files:**
- Modify: `cli/src/main.rs`

### Step 4.1: Add `--description` to `Add` command

```rust
Add {
    url: String,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    description: Option<String>,
    #[arg(long)]
    tags: Option<String>,
},
```

### Step 4.2: Add `Edit` command to `Commands` enum

```rust
/// Edit a bookmark
Edit {
    id: String,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    description: Option<String>,
    #[arg(long)]
    tags: Option<String>,
    /// Use LLM to suggest title, description, and tags
    #[arg(long)]
    suggest: bool,
},
```

### Step 4.3: Add `Suggest` command to `Commands` enum

```rust
/// Get LLM suggestions for a URL without saving
Suggest {
    url: String,
},
```

### Step 4.4: Add `description` field to `CreateBookmarkRequest`

```rust
#[derive(Serialize)]
struct CreateBookmarkRequest {
    url: String,
    title: Option<String>,
    description: Option<String>,
    tags: Option<Vec<String>>,
}
```

### Step 4.5: Add `UpdateBookmarkRequest` struct

```rust
#[derive(Serialize)]
struct UpdateBookmarkRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tags: Option<Vec<String>>,
}
```

### Step 4.6: Add `SuggestResponse` struct for CLI deserialization

```rust
#[allow(dead_code)]
#[derive(Deserialize)]
struct SuggestResponse {
    title: Option<String>,
    description: Option<String>,
    tags: Vec<String>,
    image_url: Option<String>,
    domain: Option<String>,
}
```

### Step 4.7: Add `put_json` method to `ApiClient`

```rust
async fn put_json(
    &self,
    path: &str,
    body: &impl Serialize,
) -> Result<reqwest::Response, String> {
    self.client
        .put(self.url(path))
        .bearer_auth(&self.api_key)
        .json(body)
        .send()
        .await
        .map_err(|e| e.to_string())
}
```

### Step 4.8: Update `Add` handler

Update to accept `description`, include it in `CreateBookmarkRequest`, and show richer output:

```rust
Commands::Add { url, title, description, tags } => {
    let client = AppConfig::load().client()?;
    let tags = tags.map(|t| t.split(',').map(|s| s.trim().to_string()).collect());
    let body = CreateBookmarkRequest { url, title, description, tags };
    let resp = client.post_json("/bookmarks", &body).await?;
    if resp.status().is_success() {
        let bm: Bookmark = resp.json().await.map_err(|e| e.to_string())?;
        println!("Added: {} ({})", bm.title.unwrap_or(bm.url), bm.id);
        if let Some(desc) = &bm.description {
            println!("  {desc}");
        }
        if !bm.tags.is_empty() {
            println!("  [{}]", bm.tags.join(", "));
        }
    } else {
        eprintln!("Failed: {}", resp.status());
    }
    Ok(())
}
```

### Step 4.9: Add `Edit` handler

```rust
Commands::Edit { id, title, description, tags, suggest } => {
    let client = AppConfig::load().client()?;
    let tags = tags.map(|t| t.split(',').map(|s| s.trim().to_string()).collect());
    let body = UpdateBookmarkRequest { title, description, tags };
    let path = if suggest {
        format!("/bookmarks/{id}?suggest=true")
    } else {
        format!("/bookmarks/{id}")
    };
    let resp = client.put_json(&path, &body).await?;
    if resp.status().is_success() {
        let bm: Bookmark = resp.json().await.map_err(|e| e.to_string())?;
        println!("Updated: {} ({})", bm.title.unwrap_or(bm.url), bm.id);
        if let Some(desc) = &bm.description {
            println!("  {desc}");
        }
        if !bm.tags.is_empty() {
            println!("  [{}]", bm.tags.join(", "));
        }
    } else {
        eprintln!("Failed: {}", resp.status());
    }
    Ok(())
}
```

### Step 4.10: Add `Suggest` handler

```rust
Commands::Suggest { url } => {
    let client = AppConfig::load().client()?;
    let body = serde_json::json!({ "url": url });
    let resp = client.post_json("/bookmarks/suggest", &body).await?;
    if resp.status().is_success() {
        let s: SuggestResponse = resp.json().await.map_err(|e| e.to_string())?;
        if let Some(title) = &s.title {
            println!("Title: {title}");
        }
        if let Some(desc) = &s.description {
            println!("Description: {desc}");
        }
        if !s.tags.is_empty() {
            println!("Tags: {}", s.tags.join(", "));
        }
        if let Some(domain) = &s.domain {
            println!("Domain: {domain}");
        }
    } else {
        eprintln!("Failed: {}", resp.status());
    }
    Ok(())
}
```

### Step 4.11: Add CLI unit tests

```rust
#[test]
fn test_cli_edit_recognized() {
    let cli = Cli::try_parse_from(["boop", "edit", "some-id", "--suggest"]).unwrap();
    assert!(matches!(cli.command, Commands::Edit { suggest: true, .. }));
}

#[test]
fn test_cli_edit_without_suggest() {
    let cli = Cli::try_parse_from(["boop", "edit", "some-id", "--title", "New Title"]).unwrap();
    assert!(matches!(cli.command, Commands::Edit { suggest: false, .. }));
}

#[test]
fn test_cli_suggest_recognized() {
    let cli = Cli::try_parse_from(["boop", "suggest", "https://example.com"]).unwrap();
    assert!(matches!(cli.command, Commands::Suggest { .. }));
}

#[test]
fn test_cli_add_with_description() {
    let cli = Cli::try_parse_from(["boop", "add", "https://example.com", "--description", "A test"]).unwrap();
    assert!(matches!(cli.command, Commands::Add { .. }));
}
```

### Step 4.12: Verify CLI compilation and tests

Run: `cargo test -p boop`

---

## Task 5: Full integration verification

### Step 5.1: Full build

Run: `cargo build`
Expected: Clean build, no warnings.

### Step 5.2: Full test suite

Run: `cargo test`
Expected: All tests pass.

### Step 5.3: E2E test

Run: `npx playwright test tests/e2e/suggest.spec.js`
Expected: Existing E2E suggest test still passes (page handler behavior unchanged).

---

## Testing Strategy Notes

The following tests should be validated during implementation:

### API E2E tests (to be written as integration tests or Playwright tests)

1. **POST /api/v1/bookmarks/suggest** — returns enrichment suggestions for a URL
2. **POST /api/v1/bookmarks** (create) — auto-enriches when fields are missing and user has LLM configured
3. **POST /api/v1/bookmarks** (create) — does NOT enrich when all fields are provided
4. **PUT /api/v1/bookmarks/{id}** — normal update without suggest works as before
5. **PUT /api/v1/bookmarks/{id}?suggest=true** — enriches missing fields with LLM suggestions
6. **All endpoints** — return 401 without auth

### CLI tests

1. `boop edit <id> --suggest` parses correctly
2. `boop edit <id> --title "foo"` parses correctly (suggest=false)
3. `boop suggest <url>` parses correctly
4. `boop add <url> --description "desc"` parses correctly

### Existing tests

1. `cargo test` — all existing unit tests pass
2. `npx playwright test tests/e2e/suggest.spec.js` — existing E2E test passes (page handler behavior unchanged)
