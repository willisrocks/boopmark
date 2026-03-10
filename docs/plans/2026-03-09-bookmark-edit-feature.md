# Bookmark Edit Feature Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Add inline edit support for saved bookmarks (title, description, tags) with an optional "LLM Suggest Edits" button that re-extracts metadata and provides updated suggestions using the user's existing tag vocabulary.

**Architecture:** A modal-based edit flow that reuses the existing add modal pattern. `GET /bookmarks/{id}/edit` returns an edit modal pre-populated with the bookmark's current values. `PUT /bookmarks/{id}` saves the form and returns the updated card partial (swapped in-place via HTMX). An optional "Suggest Edits" button in the edit modal triggers `POST /bookmarks/{id}/suggest`, which re-scrapes the URL and runs LLM enrichment with the user's existing tags weighted by popularity. The LLM enricher port (`EnrichmentInput`) is extended with an optional `existing_tags` field so the Anthropic adapter can instruct the LLM to prefer reusing existing tags.

**Tech Stack:** Rust/Axum 0.8, SQLx 0.8, Askama 0.12, HTMX 2, Tailwind CSS 4

---

### Task 1: Add `tags_with_counts` to the repository layer

**Files:**
- Modify: `server/src/domain/ports/bookmark_repo.rs`
- Modify: `server/src/adapters/postgres/bookmark_repo.rs`
- Modify: `server/src/app/bookmarks.rs`

**Step 1: Add `tags_with_counts` to the `BookmarkRepository` trait**

In `server/src/domain/ports/bookmark_repo.rs`, add this method to the trait (after the `all_tags` method on line 21):

```rust
    async fn tags_with_counts(&self, user_id: Uuid) -> Result<Vec<(String, i64)>, DomainError>;
```

**Step 2: Implement `tags_with_counts` in the Postgres adapter**

In `server/src/adapters/postgres/bookmark_repo.rs`, add this implementation inside the `impl BookmarkRepository for PostgresPool` block (after the `all_tags` method, before the closing `}`):

```rust
    async fn tags_with_counts(&self, user_id: Uuid) -> Result<Vec<(String, i64)>, DomainError> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT unnest(tags) AS tag, COUNT(*) AS count FROM bookmarks WHERE user_id = $1 GROUP BY tag ORDER BY count DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?;

        Ok(rows)
    }
```

**Step 3: Add `tags_with_counts` to `BookmarkService`**

In `server/src/app/bookmarks.rs`, add this method inside the `impl` block (after the `all_tags` method, around line 88):

```rust
    pub async fn tags_with_counts(&self, user_id: Uuid) -> Result<Vec<(String, i64)>, DomainError> {
        self.repo.tags_with_counts(user_id).await
    }
```

**Step 4: Verify it compiles**

Run: `cargo build`
Expected: Build succeeds

**Step 5: Commit**

```bash
git add server/src/domain/ports/bookmark_repo.rs server/src/adapters/postgres/bookmark_repo.rs server/src/app/bookmarks.rs
git commit -m "feat: add tags_with_counts to repository for popularity-weighted tag suggestions"
```

---

### Task 2: Extend `EnrichmentInput` with optional existing tags

**Files:**
- Modify: `server/src/domain/ports/llm_enricher.rs`
- Modify: `server/src/adapters/anthropic.rs`
- Modify: `server/src/web/pages/bookmarks.rs`

**Step 1: Add `existing_tags` field to `EnrichmentInput`**

In `server/src/domain/ports/llm_enricher.rs`, add a new field to `EnrichmentInput`:

```rust
pub struct EnrichmentInput {
    pub url: String,
    pub scraped_title: Option<String>,
    pub scraped_description: Option<String>,
    pub existing_tags: Option<Vec<(String, i64)>>,
}
```

**Step 2: Update `AnthropicEnricher::build_prompt` to use existing tags**

In `server/src/adapters/anthropic.rs`, replace the `build_prompt` method (lines 22-37) with:

```rust
    fn build_prompt(input: &EnrichmentInput) -> String {
        let existing_tags_instruction = match &input.existing_tags {
            Some(tags) if !tags.is_empty() => {
                let tag_list: Vec<String> = tags.iter().map(|(t, c)| format!("{t} ({c})")).collect();
                format!(
                    "\n\nThe user already has these tags (listed most-popular first): {}. \
                     Prefer reusing these existing tags. Only create new tags if none of these fit.",
                    tag_list.join(", ")
                )
            }
            _ => String::new(),
        };

        format!(
            "You are a bookmark organizer. Given a URL and its scraped metadata, suggest:\n\
             1. A concise, clear title (improve the scraped title if present)\n\
             2. A brief, useful description (1-2 sentences, improve the scraped description if present)\n\
             3. 3-5 relevant tags for categorization{existing_tags_instruction}\n\n\
             URL: {}\n\
             Scraped title: {}\n\
             Scraped description: {}\n\n\
             Respond with ONLY valid JSON in this exact format, no other text:\n\
             {{\"title\": \"...\", \"description\": \"...\", \"tags\": [\"tag1\", \"tag2\", \"tag3\"]}}",
            input.url,
            input.scraped_title.as_deref().unwrap_or("(none)"),
            input.scraped_description.as_deref().unwrap_or("(none)"),
        )
    }
```

**Step 3: Fix all `EnrichmentInput` construction sites**

In `server/src/web/pages/bookmarks.rs`, update the `try_llm_enrich` function where `EnrichmentInput` is constructed (around line 290). Add the `existing_tags` field:

```rust
    let input = EnrichmentInput {
        url: url.to_string(),
        scraped_title: metadata.as_ref().and_then(|m| m.title.clone()),
        scraped_description: metadata.as_ref().and_then(|m| m.description.clone()),
        existing_tags: None,
    };
```

**Step 4: Fix the test construction sites**

In `server/src/adapters/anthropic.rs`, update all test `EnrichmentInput` constructions (in `build_prompt_includes_url_and_scraped_metadata` and `build_prompt_handles_missing_metadata`) to include `existing_tags: None`.

**Step 5: Add a test for the existing tags prompt**

In `server/src/adapters/anthropic.rs`, add a new test:

```rust
    #[test]
    fn build_prompt_includes_existing_tags_when_present() {
        let input = EnrichmentInput {
            url: "https://example.com".to_string(),
            scraped_title: Some("Example".to_string()),
            scraped_description: None,
            existing_tags: Some(vec![
                ("rust".to_string(), 5),
                ("web".to_string(), 3),
            ]),
        };
        let prompt = AnthropicEnricher::build_prompt(&input);
        assert!(prompt.contains("rust (5)"));
        assert!(prompt.contains("web (3)"));
        assert!(prompt.contains("Prefer reusing these existing tags"));
    }
```

**Step 6: Verify it compiles and tests pass**

Run: `cargo test`
Expected: All tests pass

**Step 7: Commit**

```bash
git add server/src/domain/ports/llm_enricher.rs server/src/adapters/anthropic.rs server/src/web/pages/bookmarks.rs
git commit -m "feat: extend LLM enricher to accept existing tags for popularity-weighted suggestions"
```

---

### Task 3: Create the edit modal template

**Files:**
- Create: `templates/bookmarks/edit_modal.html`
- Create: `templates/bookmarks/edit_suggest_fields.html`

**Step 1: Create `templates/bookmarks/edit_suggest_fields.html`**

This is analogous to `add_modal_suggest_fields.html` but uses `edit_`-prefixed IDs and the edit suggest endpoint:

```html
<div id="edit-suggest-target">
    <div id="edit-suggest-spinner" class="htmx-indicator flex items-center justify-center py-8">
        <svg class="animate-spin h-8 w-8 text-blue-500" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
            <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
            <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z"></path>
        </svg>
        <span class="ml-2 text-gray-400 text-sm">Fetching suggestions...</span>
    </div>
    <div class="space-y-4">
        <div>
            <label class="block text-sm text-gray-400 mb-1">Title</label>
            <input id="edit-title-input"
                   type="text"
                   name="title"
                   value="{{ suggest_title }}"
                   placeholder="Title"
                   class="w-full px-3 py-2 rounded-lg bg-[#0f1117] border border-gray-700 text-gray-200 placeholder-gray-600 focus:outline-none focus:border-blue-500">
        </div>
        <div>
            <label class="block text-sm text-gray-400 mb-1">Description</label>
            <input id="edit-description-input"
                   type="text"
                   name="description"
                   value="{{ suggest_description }}"
                   placeholder="Description"
                   class="w-full px-3 py-2 rounded-lg bg-[#0f1117] border border-gray-700 text-gray-200 placeholder-gray-600 focus:outline-none focus:border-blue-500">
        </div>
        <div>
            <label class="block text-sm text-gray-400 mb-1">Tags</label>
            <input type="text" name="tags_input" value="{{ suggest_tags }}" placeholder="tag1, tag2, tag3 (comma separated)"
                   class="w-full px-3 py-2 rounded-lg bg-[#0f1117] border border-gray-700 text-gray-200 placeholder-gray-600 focus:outline-none focus:border-blue-500">
        </div>
    </div>
</div>
```

**Step 2: Create `templates/bookmarks/edit_modal.html`**

```html
<div id="edit-modal" class="fixed inset-0 bg-black/50 flex items-center justify-center z-50">
    <div class="bg-[#1e2235] rounded-xl border border-gray-700 p-6 w-full max-w-md">
        <div class="flex justify-between items-center mb-4">
            <h2 class="text-lg font-medium">Edit Bookmark</h2>
            <button onclick="document.getElementById('edit-modal').remove()"
                    class="text-gray-500 hover:text-gray-300">&times;</button>
        </div>
        <form hx-put="/bookmarks/{{ bookmark_id }}"
              hx-target="#bookmark-{{ bookmark_id }}"
              hx-swap="outerHTML"
              hx-on::after-request="if(event.detail.successful && event.detail.elt === this) { document.getElementById('edit-modal').remove(); }">
            {% include "bookmarks/edit_suggest_fields.html" %}
            <div class="flex gap-3 justify-end mt-4">
                {% if has_llm %}
                <button type="button"
                        hx-post="/bookmarks/{{ bookmark_id }}/suggest"
                        hx-target="#edit-suggest-target"
                        hx-swap="outerHTML"
                        hx-include="closest form"
                        hx-indicator="#edit-suggest-spinner"
                        class="px-4 py-2 rounded-lg border border-purple-700 text-purple-400 hover:text-purple-200 hover:border-purple-500 mr-auto">
                    Suggest Edits
                </button>
                {% endif %}
                <button type="button"
                        onclick="document.getElementById('edit-modal').remove()"
                        class="px-4 py-2 rounded-lg border border-gray-700 text-gray-400 hover:text-gray-200">
                    Cancel
                </button>
                <button type="submit"
                        class="px-4 py-2 rounded-lg bg-blue-600 hover:bg-blue-700 text-white">
                    Save
                </button>
            </div>
        </form>
    </div>
</div>
```

**Step 3: Commit**

```bash
git add templates/bookmarks/edit_modal.html templates/bookmarks/edit_suggest_fields.html
git commit -m "feat: add edit modal and suggest fields templates"
```

---

### Task 4: Add edit button to the bookmark card template

**Files:**
- Modify: `templates/bookmarks/card.html`

**Step 1: Add an Edit button next to the Delete button**

In `templates/bookmarks/card.html`, replace the `<div class="flex items-center justify-between mt-3">` block (lines 26-34) with:

```html
        <div class="flex items-center justify-between mt-3">
            <span class="text-xs text-gray-600">{{ bookmark.created_at_display }}</span>
            <div class="flex gap-2">
                <button class="text-xs text-gray-600 hover:text-blue-400"
                        hx-get="/bookmarks/{{ bookmark.id }}/edit"
                        hx-target="body"
                        hx-swap="beforeend">
                    Edit
                </button>
                <button class="text-xs text-gray-600 hover:text-red-400"
                        hx-delete="/bookmarks/{{ bookmark.id }}"
                        hx-target="#bookmark-{{ bookmark.id }}"
                        hx-swap="outerHTML swap:200ms"
                        hx-confirm="Delete this bookmark?">
                    Delete
                </button>
            </div>
        </div>
```

**Step 2: Verify it compiles**

Run: `cargo build`
Expected: Build succeeds

**Step 3: Commit**

```bash
git add templates/bookmarks/card.html
git commit -m "feat: add edit button to bookmark card"
```

---

### Task 5: Add page route handlers for edit, update, and edit-suggest

**Files:**
- Modify: `server/src/web/pages/bookmarks.rs`
- Modify: `server/src/web/pages/mod.rs`

**Step 1: Add the edit modal Askama template struct**

In `server/src/web/pages/bookmarks.rs`, add these new template structs (after the `SuggestFields` struct around line 106):

```rust
#[derive(Template)]
#[template(path = "bookmarks/edit_modal.html")]
struct EditModal {
    bookmark_id: Uuid,
    suggest_title: String,
    suggest_description: String,
    suggest_tags: String,
    has_llm: bool,
}

#[derive(Template)]
#[template(path = "bookmarks/edit_suggest_fields.html")]
struct EditSuggestFields {
    suggest_title: String,
    suggest_description: String,
    suggest_tags: String,
}
```

**Step 2: Add the `edit` handler function**

Add this handler function after the `delete` function (around line 314):

```rust
pub async fn edit(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> axum::response::Response {
    let bookmark = match with_bookmarks!(&state.bookmarks, svc => svc.get(id, user.id).await) {
        Ok(b) => b,
        Err(e) => return (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    };

    let has_llm = state
        .settings
        .get_decrypted_api_key(user.id)
        .await
        .ok()
        .flatten()
        .is_some();

    render(&EditModal {
        bookmark_id: bookmark.id,
        suggest_title: bookmark.title.unwrap_or_default(),
        suggest_description: bookmark.description.unwrap_or_default(),
        suggest_tags: bookmark.tags.join(", "),
        has_llm,
    })
}
```

**Step 3: Add the `UpdateForm` deserialize struct and `update` handler**

Add after the `edit` handler:

```rust
#[derive(Deserialize)]
pub struct EditForm {
    title: Option<String>,
    description: Option<String>,
    tags_input: Option<String>,
}

pub async fn update(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
    Form(form): Form<EditForm>,
) -> axum::response::Response {
    let tags = form
        .tags_input
        .filter(|t| !t.is_empty())
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect());

    let input = crate::domain::bookmark::UpdateBookmark {
        title: form.title.and_then(non_empty),
        description: form.description.and_then(non_empty),
        tags: Some(tags.unwrap_or_default()),
    };

    match with_bookmarks!(&state.bookmarks, svc => svc.update(id, user.id, input).await) {
        Ok(bookmark) => render(&BookmarkCard {
            bookmark: bookmark.into(),
        }),
        Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    }
}
```

Note: We always pass `Some(...)` for `tags` so that clearing all tags actually updates the stored value to an empty array rather than being treated as "no change" by the `COALESCE` in the SQL.

**Step 4: Add the `edit_suggest` handler**

Add after the `update` handler:

```rust
pub async fn edit_suggest(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
    Form(form): Form<SuggestForm>,
) -> axum::response::Response {
    // Get the bookmark to find its URL
    let bookmark = match with_bookmarks!(&state.bookmarks, svc => svc.get(id, user.id).await) {
        Ok(b) => b,
        Err(e) => return (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    };

    let metadata = with_bookmarks!(&state.bookmarks, svc =>
        svc.extract_metadata(&bookmark.url).await
    )
    .ok();

    // Get existing tags with counts for LLM context
    let existing_tags = with_bookmarks!(&state.bookmarks, svc =>
        svc.tags_with_counts(user.id).await
    )
    .ok();

    let enrichment = try_llm_enrich_with_tags(&state, user.id, &bookmark.url, &metadata, existing_tags).await;

    // Preserve user-typed tags; only use LLM tags if user hasn't typed any
    let user_tags = form.tags_input.and_then(non_empty);
    let suggest_tags = user_tags.unwrap_or_else(|| {
        enrichment
            .as_ref()
            .map(|e| e.tags.join(", "))
            .unwrap_or_default()
    });

    render(&EditSuggestFields {
        suggest_title: fill_if_blank(
            form.title,
            enrichment
                .as_ref()
                .and_then(|e| e.title.clone())
                .or_else(|| metadata.as_ref().and_then(|m| m.title.clone())),
        ),
        suggest_description: fill_if_blank(
            form.description,
            enrichment
                .as_ref()
                .and_then(|e| e.description.clone())
                .or_else(|| metadata.as_ref().and_then(|m| m.description.clone())),
        ),
        suggest_tags,
    })
}
```

**Step 5: Add the `try_llm_enrich_with_tags` helper**

Add this new helper function after `try_llm_enrich` (around line 303):

```rust
async fn try_llm_enrich_with_tags(
    state: &AppState,
    user_id: Uuid,
    url: &str,
    metadata: &Option<UrlMetadata>,
    existing_tags: Option<Vec<(String, i64)>>,
) -> Option<EnrichmentOutput> {
    let (api_key, model) = match state.settings.get_decrypted_api_key(user_id).await {
        Ok(Some(pair)) => pair,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!(user_id = %user_id, error = %e, "failed to load LLM settings for enrichment");
            return None;
        }
    };

    let input = EnrichmentInput {
        url: url.to_string(),
        scraped_title: metadata.as_ref().and_then(|m| m.title.clone()),
        scraped_description: metadata.as_ref().and_then(|m| m.description.clone()),
        existing_tags,
    };

    match state.enricher.enrich(&api_key, &model, input).await {
        Ok(output) => Some(output),
        Err(e) => {
            tracing::warn!(user_id = %user_id, url = %url, error = %e, "LLM enrichment failed, falling back to scrape-only");
            None
        }
    }
}
```

**Step 6: Register the new routes**

In `server/src/web/pages/mod.rs`, add the new routes. Replace the existing routes function with:

```rust
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(home))
        .route("/bookmarks", get(bookmarks::list).post(bookmarks::create))
        .route("/bookmarks/suggest", post(bookmarks::suggest))
        .route(
            "/bookmarks/{id}",
            delete(bookmarks::delete).put(bookmarks::update),
        )
        .route("/bookmarks/{id}/edit", get(bookmarks::edit))
        .route("/bookmarks/{id}/suggest", post(bookmarks::edit_suggest))
        .merge(auth::routes())
        .merge(settings::routes())
}
```

Note: The existing `delete` route at `/bookmarks/{id}` is combined with the new `put` for `update` on the same path.

**Step 7: Add required imports**

In `server/src/web/pages/mod.rs`, add `put` to the routing import:

```rust
use axum::routing::{delete, get, post, put};
```

**Step 8: Verify it compiles**

Run: `cargo build`
Expected: Build succeeds

**Step 9: Run tests**

Run: `cargo test`
Expected: All tests pass

**Step 10: Commit**

```bash
git add server/src/web/pages/bookmarks.rs server/src/web/pages/mod.rs
git commit -m "feat: add edit, update, and edit-suggest page handlers with routes"
```

---

### Task 6: Rebuild Tailwind CSS

**Files:**
- Modify: `static/css/tailwind-output.css` (generated)

The new templates use classes like `border-purple-700`, `text-purple-400`, `hover:text-purple-200`, `hover:border-purple-500` which may not be in the current Tailwind output.

**Step 1: Rebuild Tailwind**

Run: `./tailwindcss-macos-arm64 -i static/css/tailwind-input.css -o static/css/tailwind-output.css --minify`
Expected: CSS file regenerated

**Step 2: Check if the output changed**

Run: `git diff --stat`
Expected: If the CSS changed, commit it. If unchanged, skip.

**Step 3: Commit (if changed)**

```bash
git add static/css/tailwind-output.css
git commit -m "build: rebuild Tailwind CSS for edit modal styles"
```

---

### Task 7: Verify via agent-browser screenshots

**Files:** None (manual verification)

Use the Playwright MCP / agent-browser against the local dev server at `http://localhost:4000/bookmarks`.

**Step 1: Take a screenshot showing the edit button on a bookmark card**

Navigate to `http://localhost:4000/bookmarks` and take a screenshot.
Expected: Each bookmark card shows an "Edit" button next to the "Delete" button.

**Step 2: Click the Edit button and screenshot the edit modal**

Click the "Edit" button on any card and take a screenshot.
Expected: An edit modal appears with the bookmark's current title, description, and tags pre-populated.

**Step 3: Modify a field, save, and screenshot the updated card**

Change the title in the edit modal and click "Save". Take a screenshot.
Expected: The modal closes, and the bookmark card shows the updated title.

**Step 4: Test the Suggest Edits button (if LLM is configured)**

Open the edit modal on a bookmark and click "Suggest Edits". Take a screenshot.
Expected: The form fields are populated with LLM-suggested values.

**Step 5: Save screenshots with descriptive names**

Save screenshots as:
- `screenshot-edit-button-visible.png`
- `screenshot-edit-modal-open.png`
- `screenshot-edit-saved.png`
- `screenshot-edit-suggest.png`
