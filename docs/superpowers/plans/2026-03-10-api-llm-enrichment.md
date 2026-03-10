# API & CLI LLM Enrichment — Implementation Plan

**Goal:** Add LLM-powered bookmark enrichment (title, description, tags) to the REST API and CLI, matching the web app's existing capability. Include full E2E test coverage for both API and CLI.

**Architecture:** Extract the enrichment logic currently inlined in `web/pages/bookmarks.rs` (`try_llm_enrich` function, lines 304-334) into a new app-layer `EnrichmentService`. Both page handlers and API handlers call this service. A new `POST /api/v1/bookmarks/suggest` endpoint returns suggestions without saving. Create and update endpoints support opt-in enrichment via `?suggest=true`. The CLI gains `edit` and `suggest` commands, and the `add` command gains `--description`, `--suggest`, and richer output.

---

## Design Decisions

### D1: EnrichmentService owns both scrape and LLM enrichment

**Rationale:** The current `try_llm_enrich` in `pages/bookmarks.rs` depends on `AppState` directly (accessing `state.settings` and `state.enricher`). Extracting this to an app-layer service is the correct hexagonal architecture move — it removes LLM orchestration logic from the web layer and makes it reusable by both page and API handlers. The service encapsulates the full scrape-then-enrich flow, so callers don't need to know about `MetadataExtractor`, `LlmEnricher`, or `SettingsService` individually.

### D2: EnrichmentService is generic over MetadataExtractor and LlmSettingsRepository

**Rationale:** Matching the existing codebase pattern (e.g., `BookmarkService<R, M, S>` is generic over its dependencies). The `LlmEnricher` trait uses dynamic dispatch (`Arc<dyn LlmEnricher>`) because it's already designed that way in the codebase, so `EnrichmentService` takes `Arc<dyn LlmEnricher>` rather than being generic over it. This avoids changing the existing `LlmEnricher` trait to be `Sized`.

### D3: Remove `enricher` field from AppState after migration

**Rationale:** After `EnrichmentService` is wired in, nothing else in the codebase accesses `state.enricher` directly. Keeping it would create two paths to the same functionality. Removing it enforces a single entry point for enrichment.

### D4: API enrichment is opt-in via `?suggest=true` on create and update

**Rationale:** The web app triggers enrichment as an explicit user action (suggest button on URL blur), not automatically on save. The API should follow the same pattern: enrichment is opt-in via `?suggest=true` query parameter on both `POST /api/v1/bookmarks` and `PUT /api/v1/bookmarks/{id}`. This avoids adding unexpected latency to simple create/update calls (scraping + potential LLM round-trip), gives API consumers explicit control, and is consistent with the web app's separation of suggest-from-save. Without `?suggest=true`, create and update behave exactly as they do today — `BookmarkService` still scrapes metadata for missing fields as its default behavior, but the LLM enrichment step is skipped.

### D5: The `Bookmarks` enum and `with_bookmarks!` macro approach stays as-is

**Rationale:** `EnrichmentService` doesn't need to interact with `BookmarkService` at all — it only needs `MetadataExtractor`, `LlmEnricher`, and `SettingsService`, none of which vary by storage backend. So `EnrichmentService` doesn't need the enum dispatch pattern and can be a single `Arc<EnrichmentService<...>>` on `AppState`.

### D6: `metadata` (the `Arc<HtmlMetadataExtractor>`) must be cloned before being moved into `BookmarkService::new()`

**Rationale:** Currently in `main.rs`, `metadata` is moved into `BookmarkService::new()`. `EnrichmentService` also needs it. The fix is simple: clone `metadata` before the `BookmarkService` construction. Both `Local` and `S3` branches need this change.

### D7: E2E tests use Playwright following the existing api-keys.spec.js pattern

**Rationale:** The codebase already has Playwright E2E tests that exercise the REST API via `page.evaluate(async () => fetch(...))` (see `tests/e2e/api-keys.spec.js`). API enrichment E2E tests follow this same pattern: sign in via E2E auth, create an API key, then call the enrichment endpoints with Bearer auth from a fresh browser context. CLI E2E tests build the CLI binary and shell out to it pointed at the E2E server.

### D8: E2E tests use the E2E server URL as the test bookmark URL

**Rationale:** Using external URLs (e.g., `https://github.com/...`) in E2E tests introduces network dependency and flakiness. The E2E server at `http://127.0.0.1:4010` is guaranteed to be running (Playwright starts it) and returns HTML that the scraper can extract metadata from. For tests that need a URL to enrich, use `http://127.0.0.1:4010/` — the scraper will extract whatever title/meta tags the login page contains. For tests that need predictable field values, provide all fields explicitly and assert they are preserved.

### D9: `SuggestionResult` serves as the API response type directly

**Rationale:** `SuggestionResult` already derives `Serialize`. Defining a separate `SuggestResponse` type in the API handler with identical fields is unnecessary duplication. The API handler returns `Json(result)` directly.

### D10: `UpdateBookmark` does not include `image_url` or `domain`

**Rationale:** The domain type `UpdateBookmark` only has `title`, `description`, and `tags` fields. When `?suggest=true` is passed on update, enrichment can only fill these three fields. `image_url` and `domain` are set at create time and not updateable. This matches the existing web app behavior where the edit modal only shows title, description, and tags.

---

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `server/src/app/enrichment.rs` | App-layer `EnrichmentService` — owns scrape + LLM enrich logic |
| Modify | `server/src/app/mod.rs` | Export new `enrichment` module |
| Modify | `server/src/web/state.rs` | Add `EnrichmentService` to `AppState`, remove `enricher` field |
| Modify | `server/src/main.rs` | Wire `EnrichmentService` into `AppState`, clone `metadata` before `BookmarkService`, stop passing `enricher` directly |
| Modify | `server/src/web/api/bookmarks.rs` | Add suggest endpoint, opt-in enrich on create and update |
| Modify | `server/src/web/pages/bookmarks.rs` | Replace inline `try_llm_enrich` with calls to `EnrichmentService` |
| Modify | `cli/src/main.rs` | Add `edit` command with `--suggest` flag, add `--description` and `--suggest` to `add`, add `suggest` command, richer output |
| Create | `tests/e2e/api-enrichment.spec.js` | E2E tests for API enrichment endpoints |
| Create | `tests/e2e/cli-enrichment.spec.js` | E2E tests for CLI enrichment commands |

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
#[derive(Serialize)]
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

Clone metadata before the match block:

```rust
let metadata = Arc::new(HtmlMetadataExtractor::new());
let metadata_for_enrichment = metadata.clone();  // ADD THIS LINE

let (bookmarks, images_storage) = match config.storage_backend {
    // ... both arms use `metadata` as before
};
```

After the match block (where `enricher` is created on line 101), create `EnrichmentService`:
```rust
let enrichment_service = Arc::new(EnrichmentService::new(
    metadata_for_enrichment,
    enricher.clone(),
    settings_service.clone(),
));
```

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

Add request type:
```rust
#[derive(Deserialize)]
struct SuggestRequest {
    url: String,
}
```

Add handler (uses `SuggestionResult` from `enrichment.rs` directly as the JSON response — see D9):
```rust
async fn suggest(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Json(input): Json<SuggestRequest>,
) -> impl IntoResponse {
    let result = state.enrichment.suggest(user.id, &input.url, None).await;
    Json(result)
}
```

Add import: `use crate::app::enrichment::SuggestionResult;` (for type awareness, though not explicitly used in handler signature since it returns `impl IntoResponse`).

Register route in `routes()`:
```rust
.route("/suggest", post(suggest))
```

### Step 3.2: Opt-in enrich on `create_bookmark`

Add query params struct:
```rust
#[derive(Debug, Deserialize)]
struct CreateParams {
    suggest: Option<bool>,
}
```

Modify the existing `create_bookmark` handler. When `?suggest=true` is passed, call `EnrichmentService` to fill missing fields before creating:

```rust
async fn create_bookmark(
    AuthUser(user): AuthUser,
    State(state): State<AppState>,
    Query(params): Query<CreateParams>,
    Json(mut input): Json<CreateBookmark>,
) -> impl IntoResponse {
    // Enrich missing fields when explicitly requested via ?suggest=true
    if params.suggest.unwrap_or(false) {
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

**Note on double-scrape when `?suggest=true`:** `EnrichmentService.suggest()` scrapes metadata internally. `BookmarkService::create()` also has a `needs_metadata()` check that scrapes. After enrichment fills in title/description/domain/image_url, the service-layer check should see all fields filled and skip its own scrape. The only case where `BookmarkService` still scrapes is if enrichment found nothing (e.g., LLM disabled and scrape failed). This is acceptable.

**Note on create without `?suggest=true`:** The existing `BookmarkService::create()` already scrapes metadata for missing title/description/domain/image_url as part of its `needs_metadata()` logic. So even without `?suggest=true`, basic scrape-based metadata filling still happens — the `?suggest=true` flag adds LLM enrichment on top.

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
                // Note: image_url and domain are not fields on UpdateBookmark (see D10)
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

### Step 4.1: Add `--description` and `--suggest` to `Add` command

```rust
Add {
    url: String,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    description: Option<String>,
    #[arg(long)]
    tags: Option<String>,
    /// Use LLM to suggest missing title, description, and tags
    #[arg(long)]
    suggest: bool,
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

### Step 4.6: Add `SuggestRequest` and `SuggestResponse` structs for CLI

```rust
#[derive(Serialize)]
struct SuggestRequest {
    url: String,
}

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

Update to accept `description` and `suggest`, include them in `CreateBookmarkRequest`, append `?suggest=true` when requested, and show richer output:

```rust
Commands::Add { url, title, description, tags, suggest } => {
    let client = AppConfig::load().client()?;
    let tags = tags.map(|t| t.split(',').map(|s| s.trim().to_string()).collect());
    let body = CreateBookmarkRequest { url, title, description, tags };
    let path = if suggest { "/bookmarks?suggest=true" } else { "/bookmarks" };
    let resp = client.post_json(path, &body).await?;
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
    let body = SuggestRequest { url };
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

#[test]
fn test_cli_add_with_suggest() {
    let cli = Cli::try_parse_from(["boop", "add", "https://example.com", "--suggest"]).unwrap();
    assert!(matches!(cli.command, Commands::Add { suggest: true, .. }));
}
```

### Step 4.12: Verify CLI compilation and tests

Run: `cargo test -p boop`

---

## Task 5: API E2E Tests

Write Playwright E2E tests for the new API enrichment endpoints. Follow the existing pattern from `tests/e2e/api-keys.spec.js`: sign in via E2E auth, create an API key, then call the API with Bearer auth.

**Files:**
- Create: `tests/e2e/api-enrichment.spec.js`

### Step 5.1: Create `tests/e2e/api-enrichment.spec.js`

The test file should include the following tests. All tests that call API endpoints requiring auth must first sign in via E2E auth, create an API key via the settings UI, capture the raw key value, and use it as a Bearer token in `fetch()` calls from a fresh browser context (no session cookie, bearer only).

Extract shared helpers into a `beforeEach` or helper function that signs in, deletes existing API keys, creates a fresh key, captures the raw value, and provides it to the test. Follow the exact pattern from `tests/e2e/api-keys.spec.js` for the sign-in and key creation flow.

**Test 1: POST /api/v1/bookmarks/suggest returns enrichment data**

Sign in, create an API key, then from a fresh browser context:
```javascript
const resp = await freshPage.evaluate(async (key) => {
    const response = await fetch("/api/v1/bookmarks/suggest", {
        method: "POST",
        headers: {
            "Authorization": `Bearer ${key}`,
            "Content-Type": "application/json",
        },
        body: JSON.stringify({ url: "http://127.0.0.1:4010/" }),
    });
    return { status: response.status, body: await response.json() };
}, apiKey);
```
Assert: status is 200, response body has `title` (string or null), `description` (string or null), `tags` (array), `image_url` (string or null), `domain` (string or null). Assert structural correctness — the E2E server's HTML may or may not have `<title>` or meta tags, so don't assert specific values. Assert `tags` is an array (may be empty without LLM configured).

**Test 2: POST /api/v1/bookmarks?suggest=true creates a bookmark with enrichment**

Create a bookmark with only the URL, requesting enrichment:
```javascript
const resp = await freshPage.evaluate(async (key) => {
    const response = await fetch("/api/v1/bookmarks?suggest=true", {
        method: "POST",
        headers: {
            "Authorization": `Bearer ${key}`,
            "Content-Type": "application/json",
        },
        body: JSON.stringify({ url: "http://127.0.0.1:4010/" }),
    });
    return { status: response.status, body: await response.json() };
}, apiKey);
```
Assert: status is 201, response body has `id` (UUID), `url` is `"http://127.0.0.1:4010/"`. The bookmark was created successfully.

**Test 3: POST /api/v1/bookmarks preserves client-provided fields (no suggest)**

Create a bookmark with all fields pre-populated, no `?suggest=true`:
```javascript
const resp = await freshPage.evaluate(async (key) => {
    const response = await fetch("/api/v1/bookmarks", {
        method: "POST",
        headers: { "Authorization": `Bearer ${key}`, "Content-Type": "application/json" },
        body: JSON.stringify({
            url: "https://example.com/test-preserve",
            title: "My Title",
            description: "My Description",
            tags: ["tag1", "tag2"],
        }),
    });
    return { status: response.status, body: await response.json() };
}, apiKey);
```
Assert: status is 201, returned `title` is `"My Title"`, `description` is `"My Description"`, `tags` contains `"tag1"` and `"tag2"`.

**Test 4: PUT /api/v1/bookmarks/{id} normal update (no suggest)**

Create a bookmark, then update its title:
```javascript
const updateResp = await freshPage.evaluate(async ({ key, id }) => {
    const response = await fetch(`/api/v1/bookmarks/${id}`, {
        method: "PUT",
        headers: { "Authorization": `Bearer ${key}`, "Content-Type": "application/json" },
        body: JSON.stringify({ title: "Updated Title" }),
    });
    return { status: response.status, body: await response.json() };
}, { key: apiKey, id: bookmarkId });
```
Assert: status is 200, returned `title` is `"Updated Title"`.

**Test 5: PUT /api/v1/bookmarks/{id}?suggest=true enriches missing fields**

Create a bookmark with a title but no description, then update with `?suggest=true` and empty body:
```javascript
const updateResp = await freshPage.evaluate(async ({ key, id }) => {
    const response = await fetch(`/api/v1/bookmarks/${id}?suggest=true`, {
        method: "PUT",
        headers: { "Authorization": `Bearer ${key}`, "Content-Type": "application/json" },
        body: JSON.stringify({}),
    });
    return { status: response.status, body: await response.json() };
}, { key: apiKey, id: bookmarkId });
```
Assert: status is 200, returned bookmark has `id` matching `bookmarkId`. The title from the original create is preserved (since the update body didn't set it, and `suggest` fills in only `None` fields — but note: the update body sends `{}` so all fields are `None` in the `UpdateBookmark`, meaning suggest will fill them all). Assert the response has valid structure.

**Test 6: All new endpoints return 401 without auth**

```javascript
const suggestResp = await request.post("/api/v1/bookmarks/suggest", {
    headers: { "Content-Type": "application/json" },
    data: { url: "https://example.com" },
});
expect(suggestResp.status()).toBe(401);
```

### Step 5.2: Run API E2E tests

Run: `npx playwright test tests/e2e/api-enrichment.spec.js`
Expected: All tests pass.

---

## Task 6: CLI E2E Tests

Write Playwright E2E tests that build the CLI binary and exercise it against the E2E server. Follow the same E2E infrastructure (the Playwright webServer boots the server on port 4010).

**Files:**
- Create: `tests/e2e/cli-enrichment.spec.js`

### Step 6.1: Create `tests/e2e/cli-enrichment.spec.js`

The CLI E2E tests need an API key and the server URL. The approach: sign in via E2E auth, create an API key via the settings UI, capture it, then use Node's `child_process.execSync` to run the `boop` CLI binary.

Before the test suite, build the CLI:
```javascript
const { execSync } = require("child_process");
// Build CLI once before tests
execSync("cargo build -p boop", { stdio: "inherit" });
const BOOP = "./target/debug/boop";
```

Helper to run boop with config. **Important:** On macOS, `dirs::config_dir()` returns `$HOME/Library/Application Support`. Set `HOME` to a temp directory so the CLI writes its config there. Place the config file at `<tempdir>/Library/Application Support/boop/config.toml`:

```javascript
const fs = require("fs");
const path = require("path");
const os = require("os");

function runBoop(args, apiKey) {
    const tempHome = fs.mkdtempSync(path.join(os.tmpdir(), "boop-e2e-"));
    // dirs::config_dir() on macOS = $HOME/Library/Application Support
    const configDir = path.join(tempHome, "Library", "Application Support", "boop");
    fs.mkdirSync(configDir, { recursive: true });
    fs.writeFileSync(
        path.join(configDir, "config.toml"),
        `server_url = "http://127.0.0.1:4010"\napi_key = "${apiKey}"\n`
    );

    const env = { ...process.env, HOME: tempHome };
    return execSync(`${BOOP} ${args}`, { env, encoding: "utf-8", timeout: 30000 });
}
```

**Test 1: `boop add` creates a bookmark and shows output**

```javascript
const output = runBoop('add "https://example.com/cli-test-1"', apiKey);
expect(output).toContain("Added:");
expect(output).toContain("("); // contains UUID in parens
```

**Test 2: `boop suggest` returns suggestions**

```javascript
const output = runBoop('suggest "http://127.0.0.1:4010/"', apiKey);
// Without LLM configured, at minimum the output should not error.
// The suggest endpoint returns 200 even with no LLM; scrape results vary.
expect(output).toBeDefined();
```

**Test 3: `boop edit` with `--suggest` updates a bookmark**

First create a bookmark via API (using `page.evaluate` and Bearer token), capture its ID, then:
```javascript
const output = runBoop(`edit ${bookmarkId} --suggest`, apiKey);
expect(output).toContain("Updated:");
```

**Test 4: `boop edit` with explicit fields**

```javascript
const output = runBoop(`edit ${bookmarkId} --title "New Title" --description "New Desc"`, apiKey);
expect(output).toContain("Updated: New Title");
```

**Test 5: `boop add --suggest` creates with enrichment**

```javascript
const output = runBoop('add "http://127.0.0.1:4010/" --suggest', apiKey);
expect(output).toContain("Added:");
```

### Step 6.2: Run CLI E2E tests

Run: `npx playwright test tests/e2e/cli-enrichment.spec.js`
Expected: All tests pass.

---

## Task 7: Full integration verification

### Step 7.1: Full build

Run: `cargo build`
Expected: Clean build, no warnings.

### Step 7.2: Full test suite

Run: `cargo test`
Expected: All tests pass.

### Step 7.3: Run all E2E tests

Run: `npx playwright test`
Expected: All E2E tests pass, including the existing `suggest.spec.js` (page handler behavior unchanged), the new `api-enrichment.spec.js`, and `cli-enrichment.spec.js`.
