# Fix LLM Bookmark Enrichment Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Wire up LLM-powered enrichment so that when a user pastes a URL in the Add Bookmark modal, the scraped metadata is sent to the Anthropic API (using the user's saved key/model), and the LLM suggests an improved title, description, and tags.

**Architecture:** Add an `AnthropicEnricher` adapter that calls the Anthropic Messages API. Introduce a new `LlmEnricher` port trait. Thread the enricher + settings through the suggest handler so it runs after HTML scraping. The suggest template already fills title/description; extend it to also fill tags. The `BookmarkService` stays unaware of LLM enrichment — enrichment only happens in the suggest (preview) flow, not on `create`.

**Tech Stack:** Rust, Axum, reqwest (for Anthropic HTTP API), serde_json, existing SecretBox for decryption.

---

### Task 1: Add `tags` field to `UrlMetadata` and `SuggestFields`

**Files:**
- Modify: `server/src/domain/bookmark.rs` — add `tags: Option<Vec<String>>` to `UrlMetadata`
- Modify: `server/src/adapters/scraper.rs` — set `tags: None` in the returned `UrlMetadata`
- Modify: `server/src/web/pages/bookmarks.rs` — add `suggest_tags` field to `SuggestFields` struct
- Modify: `templates/bookmarks/add_modal_suggest_fields.html` — render `suggest_tags` into the tags input
- Modify: `templates/bookmarks/add_modal.html` — move the tags `<input>` inside the suggest-fields partial so it gets updated by HTMX
- Modify: `templates/bookmarks/grid.html` — add default `suggest_tags` to GridPage

**Step 1: Add `tags` to `UrlMetadata`**

In `server/src/domain/bookmark.rs`, add `tags` to the `UrlMetadata` struct:
```rust
#[derive(Debug, Serialize)]
pub struct UrlMetadata {
    pub title: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub domain: Option<String>,
    pub tags: Option<Vec<String>>,
}
```

**Step 2: Update scraper to set `tags: None`**

In `server/src/adapters/scraper.rs`, in the `extract` method's `Ok(UrlMetadata { ... })` block, add:
```rust
tags: None,
```

**Step 3: Update all test constructors of `UrlMetadata`**

In `server/src/app/bookmarks.rs` tests, add `tags: None` to the `UrlMetadata` literal in `merge_metadata_preserves_user_text_but_returns_missing_image`.

**Step 4: Move tags input into the suggest-fields partial**

Move the tags `<div>` from `templates/bookmarks/add_modal.html` into `templates/bookmarks/add_modal_suggest_fields.html`, just before the closing `</div>` of the `space-y-4` wrapper. Set its `value` to `{{ suggest_tags }}`.

In `templates/bookmarks/add_modal.html`, remove the tags `<div>` (lines 30-33).

**Step 5: Add `suggest_tags` to `SuggestFields` and `GridPage`**

In `server/src/web/pages/bookmarks.rs`:
- Add `suggest_tags: String` to the `SuggestFields` struct.
- Add `suggest_tags: String` to the `GridPage` struct.
- In the `list` handler, set `suggest_tags: String::new()`.
- In the `suggest` handler, set `suggest_tags: String::new()` (will be populated in a later task).

**Step 6: Run tests**

Run: `cargo test -p boopmark-server`
Expected: All existing tests pass (with the `tags: None` additions).

**Step 7: Commit**

```bash
git add server/src/domain/bookmark.rs server/src/adapters/scraper.rs server/src/web/pages/bookmarks.rs server/src/app/bookmarks.rs templates/bookmarks/add_modal.html templates/bookmarks/add_modal_suggest_fields.html
git commit -m "feat: add tags field to UrlMetadata and suggest fields template"
```

---

### Task 2: Create the `LlmEnricher` port and `AnthropicEnricher` adapter

**Files:**
- Create: `server/src/domain/ports/llm_enricher.rs` — new port trait
- Modify: `server/src/domain/ports/mod.rs` — add `pub mod llm_enricher;`
- Create: `server/src/adapters/anthropic.rs` — Anthropic Messages API adapter
- Modify: `server/src/adapters/mod.rs` — add `pub mod anthropic;`

**Step 1: Define the enrichment request/response types and trait**

Create `server/src/domain/ports/llm_enricher.rs`:
```rust
use crate::domain::error::DomainError;

/// Raw scraped metadata sent to the LLM for enrichment.
pub struct EnrichmentInput {
    pub url: String,
    pub scraped_title: Option<String>,
    pub scraped_description: Option<String>,
}

/// LLM-suggested improvements.
pub struct EnrichmentOutput {
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
}

#[trait_variant::make(Send)]
pub trait LlmEnricher: Send + Sync {
    async fn enrich(&self, input: EnrichmentInput) -> Result<EnrichmentOutput, DomainError>;
}
```

**Step 2: Register the port module**

In `server/src/domain/ports/mod.rs`, add:
```rust
pub mod llm_enricher;
```

**Step 3: Implement `AnthropicEnricher`**

Create `server/src/adapters/anthropic.rs`:
```rust
use crate::domain::error::DomainError;
use crate::domain::ports::llm_enricher::{EnrichmentInput, EnrichmentOutput, LlmEnricher};
use serde::{Deserialize, Serialize};

pub struct AnthropicEnricher {
    client: reqwest::Client,
}

impl AnthropicEnricher {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    fn build_prompt(input: &EnrichmentInput) -> String {
        format!(
            "You are a bookmark organizer. Given a URL and its scraped metadata, suggest:\n\
             1. A concise, clear title (improve the scraped title if present)\n\
             2. A brief, useful description (1-2 sentences, improve the scraped description if present)\n\
             3. 3-5 relevant tags for categorization\n\n\
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
}

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<Message>,
}

#[derive(Serialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    text: Option<String>,
}

#[derive(Deserialize)]
struct EnrichmentJson {
    title: Option<String>,
    description: Option<String>,
    tags: Option<Vec<String>>,
}

impl LlmEnricher for AnthropicEnricher {
    async fn enrich(
        &self,
        api_key: &str,
        model: &str,
        input: EnrichmentInput,
    ) -> Result<EnrichmentOutput, DomainError> {
        let prompt = Self::build_prompt(&input);

        let request_body = AnthropicRequest {
            model: model.to_string(),
            max_tokens: 512,
            messages: vec![Message {
                role: "user".to_string(),
                content: prompt,
            }],
        };

        let resp = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| DomainError::Internal(format!("Anthropic API error: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(DomainError::Internal(format!(
                "Anthropic API returned HTTP {status}: {body}"
            )));
        }

        let api_resp: AnthropicResponse = resp
            .json()
            .await
            .map_err(|e| DomainError::Internal(format!("Anthropic response parse error: {e}")))?;

        let text = api_resp
            .content
            .into_iter()
            .find_map(|block| block.text)
            .ok_or_else(|| DomainError::Internal("Anthropic response had no text".to_string()))?;

        // Parse the JSON from the response text, stripping any markdown fences
        let json_str = text
            .trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();

        let parsed: EnrichmentJson = serde_json::from_str(json_str)
            .map_err(|e| DomainError::Internal(format!("LLM JSON parse error: {e}")))?;

        Ok(EnrichmentOutput {
            title: parsed.title,
            description: parsed.description,
            tags: parsed.tags.unwrap_or_default(),
        })
    }
}
```

**Important:** Update the `LlmEnricher` trait to take `api_key` and `model` as parameters (since they are per-user, not per-adapter):
```rust
#[trait_variant::make(Send)]
pub trait LlmEnricher: Send + Sync {
    async fn enrich(
        &self,
        api_key: &str,
        model: &str,
        input: EnrichmentInput,
    ) -> Result<EnrichmentOutput, DomainError>;
}
```

**Step 4: Register the adapter module**

In `server/src/adapters/mod.rs`, add:
```rust
pub mod anthropic;
```

**Step 5: Verify it compiles**

Run: `cargo build -p boopmark-server`
Expected: Compiles without errors.

**Step 6: Commit**

```bash
git add server/src/domain/ports/llm_enricher.rs server/src/domain/ports/mod.rs server/src/adapters/anthropic.rs server/src/adapters/mod.rs
git commit -m "feat: add LlmEnricher port and AnthropicEnricher adapter"
```

---

### Task 3: Wire LLM enrichment into the suggest handler

**Files:**
- Modify: `server/src/web/state.rs` — add `enricher` and `secret_box` to `AppState`
- Modify: `server/src/main.rs` — construct `AnthropicEnricher` and add to state
- Modify: `server/src/web/pages/bookmarks.rs` — call LLM enricher in `suggest` handler after scraping, using user's settings

**Step 1: Add enricher and secret_box to AppState**

In `server/src/web/state.rs`, add:
```rust
use crate::adapters::anthropic::AnthropicEnricher;
use crate::app::secrets::SecretBox;
use std::sync::Arc;

pub struct AppState {
    pub bookmarks: Bookmarks,
    pub auth: Arc<AuthService<PostgresPool, PostgresPool, PostgresPool>>,
    pub settings: Arc<SettingsService<PostgresPool>>,
    pub config: Arc<Config>,
    pub enricher: Arc<AnthropicEnricher>,
    pub secret_box: Arc<SecretBox>,
}
```

**Step 2: Construct enricher in main.rs**

In `server/src/main.rs`, after constructing `secret_box`:
```rust
let enricher = Arc::new(AnthropicEnricher::new());
```
And add to the `AppState`:
```rust
let state = AppState {
    bookmarks,
    auth: auth_service,
    settings: settings_service,
    config: Arc::new(config.clone()),
    enricher,
    secret_box: secret_box.clone(),
};
```
Note: `secret_box` is already created; just clone it into state before also passing to `SettingsService`.

**Step 3: Update the suggest handler to call LLM enrichment**

In `server/src/web/pages/bookmarks.rs`, update the `suggest` function:

```rust
use crate::domain::ports::llm_enricher::EnrichmentInput;

pub async fn suggest(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Form(form): Form<SuggestForm>,
) -> axum::response::Response {
    let metadata = if form.url.trim().is_empty() {
        None
    } else {
        with_bookmarks!(&state.bookmarks, svc => svc.extract_metadata(&form.url).await).ok()
    };

    // Attempt LLM enrichment if user has it configured
    let enrichment = try_llm_enrich(&state, user.id, &form.url, &metadata).await;

    let suggest_tags = enrichment
        .as_ref()
        .map(|e| e.tags.join(", "))
        .unwrap_or_default();

    render(&SuggestFields {
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
        suggest_preview_image_url: metadata.and_then(|m| m.image_url),
        suggest_tags,
    })
}
```

Add a helper function:
```rust
use crate::domain::ports::llm_enricher::{EnrichmentInput, EnrichmentOutput, LlmEnricher};
use crate::domain::bookmark::UrlMetadata;

async fn try_llm_enrich(
    state: &AppState,
    user_id: Uuid,
    url: &str,
    metadata: &Option<UrlMetadata>,
) -> Option<EnrichmentOutput> {
    // Load user's LLM settings
    let settings = state.settings.load(user_id).await.ok()?;
    if !settings.enabled || !settings.has_anthropic_api_key {
        return None;
    }

    // We need the actual decrypted API key — load raw settings from repo
    let llm_settings = {
        // Access the settings service's underlying repo via a dedicated method
        // Actually, we need to decrypt the key. We have secret_box on state.
        // We need to load the raw LlmSettings to get the encrypted key.
        // The SettingsService only exposes SettingsView (no raw key).
        // Solution: query the LLM settings repo directly via a new method on SettingsService.
        None
    };

    // ... This approach needs a way to get the decrypted key.
    // See Step 4 for the solution.
    None
}
```

**Step 4: Add `get_decrypted_api_key` to `SettingsService`**

In `server/src/app/settings.rs`, add a method:
```rust
pub async fn get_decrypted_api_key(&self, user_id: Uuid) -> Result<Option<(String, String)>, DomainError> {
    let settings = self.repo.get(user_id).await?;
    match settings {
        Some(s) if s.enabled => {
            if let Some(encrypted) = &s.anthropic_api_key_encrypted {
                let decrypted = self.secret_box.decrypt(encrypted)
                    .map_err(DomainError::Internal)?;
                Ok(Some((decrypted, s.anthropic_model)))
            } else {
                Ok(None)
            }
        }
        _ => Ok(None),
    }
}
```

**Step 5: Complete the `try_llm_enrich` helper**

```rust
async fn try_llm_enrich(
    state: &AppState,
    user_id: Uuid,
    url: &str,
    metadata: &Option<UrlMetadata>,
) -> Option<EnrichmentOutput> {
    let (api_key, model) = state.settings.get_decrypted_api_key(user_id).await.ok()??;

    let input = EnrichmentInput {
        url: url.to_string(),
        scraped_title: metadata.as_ref().and_then(|m| m.title.clone()),
        scraped_description: metadata.as_ref().and_then(|m| m.description.clone()),
    };

    state.enricher.enrich(&api_key, &model, input).await.ok()
}
```

**Step 6: Verify it compiles**

Run: `cargo build -p boopmark-server`
Expected: Compiles without errors.

**Step 7: Run tests**

Run: `cargo test -p boopmark-server`
Expected: All tests pass.

**Step 8: Commit**

```bash
git add server/src/web/state.rs server/src/main.rs server/src/web/pages/bookmarks.rs server/src/app/settings.rs
git commit -m "feat: wire LLM enrichment into bookmark suggest handler"
```

---

### Task 4: Add unit tests for the enrichment flow

**Files:**
- Modify: `server/src/adapters/anthropic.rs` — add unit tests for prompt building and JSON parsing

**Step 1: Write tests for prompt building**

In `server/src/adapters/anthropic.rs`, add a `#[cfg(test)]` module:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ports::llm_enricher::EnrichmentInput;

    #[test]
    fn build_prompt_includes_url_and_scraped_metadata() {
        let input = EnrichmentInput {
            url: "https://example.com".to_string(),
            scraped_title: Some("Example Title".to_string()),
            scraped_description: Some("Example description".to_string()),
        };
        let prompt = AnthropicEnricher::build_prompt(&input);
        assert!(prompt.contains("https://example.com"));
        assert!(prompt.contains("Example Title"));
        assert!(prompt.contains("Example description"));
    }

    #[test]
    fn build_prompt_handles_missing_metadata() {
        let input = EnrichmentInput {
            url: "https://example.com".to_string(),
            scraped_title: None,
            scraped_description: None,
        };
        let prompt = AnthropicEnricher::build_prompt(&input);
        assert!(prompt.contains("(none)"));
    }

    #[test]
    fn parse_enrichment_json_from_clean_response() {
        let json = r#"{"title": "Better Title", "description": "Better desc", "tags": ["rust", "web"]}"#;
        let parsed: EnrichmentJson = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.title.as_deref(), Some("Better Title"));
        assert_eq!(parsed.tags.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn parse_enrichment_json_with_markdown_fences() {
        let text = "```json\n{\"title\": \"T\", \"description\": \"D\", \"tags\": [\"a\"]}\n```";
        let json_str = text
            .trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();
        let parsed: EnrichmentJson = serde_json::from_str(json_str).unwrap();
        assert_eq!(parsed.title.as_deref(), Some("T"));
    }
}
```

**Step 2: Run tests**

Run: `cargo test -p boopmark-server`
Expected: All tests pass including new ones.

**Step 3: Commit**

```bash
git add server/src/adapters/anthropic.rs
git commit -m "test: add unit tests for AnthropicEnricher prompt and parsing"
```

---

### Task 5: Add a `get_decrypted_api_key` unit test

**Files:**
- Modify: `server/src/app/settings.rs` — add test for `get_decrypted_api_key`

**Step 1: Write the test**

Add to the existing `#[cfg(test)] mod tests` in `server/src/app/settings.rs`:
```rust
#[tokio::test]
async fn get_decrypted_api_key_returns_key_and_model_when_enabled() {
    let repo = Arc::new(FakeLlmSettingsRepository::new());
    let secret_box = Arc::new(SecretBox::new(
        "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
    ));
    let service = SettingsService::new(repo.clone(), secret_box.clone());
    let user_id = Uuid::new_v4();

    // Save a key first
    service.save(user_id, SaveLlmSettingsInput {
        enabled: true,
        anthropic_api_key: Some("sk-ant-test-key".into()),
        clear_anthropic_api_key: false,
        anthropic_model: Some("claude-haiku-4-5-20251001".into()),
    }).await.expect("save");

    let result = service.get_decrypted_api_key(user_id).await.expect("get key");
    let (key, model) = result.expect("should have key");
    assert_eq!(key, "sk-ant-test-key");
    assert_eq!(model, "claude-haiku-4-5-20251001");
}

#[tokio::test]
async fn get_decrypted_api_key_returns_none_when_disabled() {
    let repo = Arc::new(FakeLlmSettingsRepository::new());
    let secret_box = Arc::new(SecretBox::new(
        "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
    ));
    let service = SettingsService::new(repo.clone(), secret_box.clone());
    let user_id = Uuid::new_v4();

    service.save(user_id, SaveLlmSettingsInput {
        enabled: false,
        anthropic_api_key: Some("sk-ant-test-key".into()),
        clear_anthropic_api_key: false,
        anthropic_model: Some("claude-haiku-4-5-20251001".into()),
    }).await.expect("save");

    let result = service.get_decrypted_api_key(user_id).await.expect("get key");
    assert!(result.is_none());
}
```

**Step 2: Run tests**

Run: `cargo test -p boopmark-server`
Expected: All tests pass.

**Step 3: Commit**

```bash
git add server/src/app/settings.rs
git commit -m "test: add unit tests for get_decrypted_api_key"
```

---

### Task 6: Copy .env, build, and verify with E2E testing

**Files:**
- No code changes — verification only.

**Step 1: Copy .env into the worktree**

```bash
cp /Users/chrisfenton/Code/personal/boopmark/.env /Users/chrisfenton/Code/personal/boopmark/.worktrees/fix-llm-bookmark-enrichment/.env
```

**Step 2: Run cargo test**

Run: `cargo test` from the worktree directory.
Expected: All tests pass.

**Step 3: Run cargo build**

Run: `cargo build` from the worktree directory.
Expected: Compiles without errors.

**Step 4: Start the local dev stack**

Run: `docker compose up -d` to start Postgres, then `cargo run -p boopmark-server` from the worktree.

**Step 5: Agent-browser E2E verification**

Using agent-browser / Playwright MCP:
1. Navigate to the running app (likely `http://localhost:4000`)
2. Sign in via E2E auth
3. Go to `/settings` and verify LLM is enabled with a valid Anthropic API key and model selected
4. If not already configured, enable LLM integration, enter a valid Anthropic API key, select a model, and save
5. Go to `/bookmarks` and click "Add Bookmark"
6. Paste `https://github.com/danshapiro/trycycle` into the URL field and tab out
7. Wait for the suggest fields to populate
8. **Take screenshot** — verify:
   - Title is LLM-enhanced (not just raw "GitHub - danshapiro/trycycle")
   - Description is LLM-enhanced (not just raw og:description)
   - Tags field is populated with comma-separated tags (not empty)
9. Submit the bookmark
10. **Take screenshot** — verify the bookmark card appears with correct data

**Step 6: Run existing E2E test suite**

Run: `npx playwright test tests/e2e/suggest.spec.js`
Expected: Existing tests still pass (they don't check LLM enrichment, just that title/description are non-empty).

**Step 7: Commit any test adjustments if needed**

---

### Summary of the bug

The LLM enrichment was never implemented. The codebase has:
- LLM settings management (save/load encrypted API key, model selection) — fully working
- HTML metadata scraping — fully working
- **Missing:** An adapter to call the Anthropic API with the scraped metadata
- **Missing:** Wiring in the suggest handler to call the LLM after scraping
- **Missing:** Tags field in `UrlMetadata` and the suggest template

This plan adds the missing LLM enrichment layer while keeping the architecture clean (port/adapter pattern, per-user credentials decrypted on demand).
