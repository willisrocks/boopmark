# Tag Consolidation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a one-shot, user-triggered "Consolidate Tags" button on the Settings page that uses the user's configured Anthropic key to merge variant/synonym tags across all their bookmarks and optionally add broader parent tags alongside narrow ones.

**Architecture:** Hexagonal — new `TagConsolidator` port (LLM-facing) plus three new methods on the existing `BookmarkRepository` (DB-facing). A new `TagConsolidationService` orchestrates: it loads tag samples, calls the LLM, computes per-bookmark new-tag lists in a pure helper, and writes the changes back in a single transaction. The HTTP endpoint runs synchronously inside the existing settings router.

**Tech Stack:** Rust 2024, Axum 0.8, sqlx 0.8 (Postgres), Askama 0.12, HTMX 2, Tailwind 4. Reuses the existing `AnthropicEnricher` HTTP/JSON pattern.

---

## File Structure

**New files:**
- `server/src/domain/ports/tag_consolidator.rs` — `TagConsolidator` trait + `ConsolidationInput` / `ConsolidationOutput` / `TagSample` types.
- `server/src/adapters/anthropic_tag_consolidator.rs` — `AnthropicTagConsolidator` implementing the port. Mirrors the JSON-extraction and HTTP plumbing from `adapters/anthropic.rs`.
- `server/src/app/tag_consolidation.rs` — `TagConsolidationService` plus the pure `compute_new_tags` helper.
- `templates/settings/tag_consolidation_result.html` — small HTMX response fragment (success or error message).

**Modified files:**
- `server/src/domain/ports/mod.rs` — register `tag_consolidator`.
- `server/src/domain/ports/bookmark_repo.rs` — add `tag_samples`, `list_id_tags`, `update_tags_bulk`.
- `server/src/adapters/postgres/bookmark_repo.rs` — implement the three new methods.
- `server/src/adapters/mod.rs` — register `anthropic_tag_consolidator`.
- `server/src/app/mod.rs` — register `tag_consolidation`.
- `server/src/app/bookmarks.rs` — add stubs for the three new repo methods on the two existing `MockRepo` blocks (lines ~527 and ~1262).
- `server/src/main.rs` — construct `AnthropicTagConsolidator` + `TagConsolidationService`, store on `AppState`.
- `server/src/web/state.rs` — add `tag_consolidation` field to `AppState`.
- `server/src/web/pages/settings.rs` — add `consolidate_tags_htmx` handler, the result fragment, route registration, and template-context flag for "library size".
- `templates/settings/index.html` — add a "Tag Library" section above "Image Repair".

---

## Task 1: Add the `TagConsolidator` port

**Files:**
- Create: `server/src/domain/ports/tag_consolidator.rs`
- Modify: `server/src/domain/ports/mod.rs`

- [ ] **Step 1: Register the new module**

Add to `server/src/domain/ports/mod.rs` (alphabetical order respected):
```rust
pub mod api_key_repo;
pub mod bookmark_repo;
pub mod invite_repo;
pub mod llm_enricher;
pub mod llm_settings_repo;
pub mod login_provider;
pub mod metadata;
pub mod screenshot;
pub mod session_repo;
pub mod storage;
pub mod tag_consolidator;
pub mod user_repo;
```

- [ ] **Step 2: Create the port file**

Create `server/src/domain/ports/tag_consolidator.rs`:
```rust
use crate::domain::error::DomainError;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

/// One tag in the user's library, with bookmark count and a few sample titles
/// for the LLM to disambiguate ambiguous names.
#[derive(Debug, Clone)]
pub struct TagSample {
    pub tag: String,
    pub count: i64,
    /// Up to 3 representative bookmark titles for this tag.
    pub sample_titles: Vec<String>,
}

pub struct ConsolidationInput {
    pub tags: Vec<TagSample>,
}

pub struct ConsolidationOutput {
    /// Maps each input tag (case-preserving) to the full list of tags a bookmark
    /// currently carrying that tag should end up with.
    pub mapping: HashMap<String, Vec<String>>,
}

pub trait TagConsolidator: Send + Sync {
    fn consolidate(
        &self,
        api_key: &str,
        model: &str,
        input: ConsolidationInput,
    ) -> Pin<Box<dyn Future<Output = Result<ConsolidationOutput, DomainError>> + Send + '_>>;
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p boopmark-server`
Expected: PASS (no errors, may have unused warnings — those will resolve as later tasks consume the port).

- [ ] **Step 4: Commit**

```bash
git add server/src/domain/ports/mod.rs server/src/domain/ports/tag_consolidator.rs
git commit -m "feat: add TagConsolidator port"
```

---

## Task 2: Add the pure `compute_new_tags` helper + tests

This is the testable core of the apply step. It takes a bookmark's current tags plus a mapping and returns the new tag list. Putting it in its own file lets us TDD it without DB or LLM.

**Files:**
- Create: `server/src/app/tag_consolidation.rs`
- Modify: `server/src/app/mod.rs`

- [ ] **Step 1: Register the new module**

Add to `server/src/app/mod.rs`:
```rust
pub mod auth;
pub mod bookmarks;
pub mod enrichment;
pub mod invite;
pub mod secrets;
pub mod settings;
pub mod tag_consolidation;
```

- [ ] **Step 2: Write the failing test file**

Create `server/src/app/tag_consolidation.rs` with just the helper signature stub and tests:
```rust
use std::collections::HashMap;

/// Compute the new tag list for a bookmark.
///
/// For each current tag:
/// - Look up its mapping. If absent or empty, treat as identity (`[tag]`).
/// - Collect every value from every mapping output for the bookmark's tags.
/// - Lowercase, dedupe (case-insensitive), sort.
pub fn compute_new_tags(
    current: &[String],
    mapping: &HashMap<String, Vec<String>>,
) -> Vec<String> {
    let _ = (current, mapping);
    todo!("implement in step 4")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &[&str])]) -> HashMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(k, vs)| ((*k).to_string(), vs.iter().map(|v| (*v).to_string()).collect()))
            .collect()
    }

    #[test]
    fn merges_variants_into_one_canonical() {
        let mapping = map(&[
            ("js", &["javascript"]),
            ("javascript", &["javascript"]),
            ("JavaScript", &["javascript"]),
        ]);
        let result =
            compute_new_tags(&["js".into(), "JavaScript".into()], &mapping);
        assert_eq!(result, vec!["javascript".to_string()]);
    }

    #[test]
    fn adds_parent_tag_alongside_narrow_tag() {
        let mapping = map(&[("react", &["react", "frontend"])]);
        let result = compute_new_tags(&["react".into()], &mapping);
        assert_eq!(result, vec!["frontend".to_string(), "react".to_string()]);
    }

    #[test]
    fn omitted_tag_is_treated_as_identity() {
        let mapping = map(&[("react", &["react", "frontend"])]);
        let result = compute_new_tags(&["react".into(), "rust".into()], &mapping);
        assert_eq!(
            result,
            vec!["frontend".to_string(), "react".to_string(), "rust".to_string()]
        );
    }

    #[test]
    fn empty_mapping_value_is_treated_as_identity() {
        let mapping = map(&[("react", &[])]);
        let result = compute_new_tags(&["react".into()], &mapping);
        assert_eq!(result, vec!["react".to_string()]);
    }

    #[test]
    fn outputs_are_lowercased() {
        let mapping = map(&[("react", &["React", "FRONTEND"])]);
        let result = compute_new_tags(&["react".into()], &mapping);
        assert_eq!(result, vec!["frontend".to_string(), "react".to_string()]);
    }

    #[test]
    fn deduplicates_case_insensitively() {
        let mapping = map(&[
            ("react", &["react", "frontend"]),
            ("vue", &["vue", "Frontend"]),
        ]);
        let result = compute_new_tags(&["react".into(), "vue".into()], &mapping);
        assert_eq!(
            result,
            vec!["frontend".to_string(), "react".to_string(), "vue".to_string()]
        );
    }

    #[test]
    fn empty_input_returns_empty() {
        let mapping = map(&[("react", &["react"])]);
        let result = compute_new_tags(&[], &mapping);
        assert!(result.is_empty());
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p boopmark-server tag_consolidation::tests`
Expected: FAIL (or panic) with `not yet implemented` from `todo!()`.

- [ ] **Step 4: Implement `compute_new_tags`**

Replace the body of `compute_new_tags` in `server/src/app/tag_consolidation.rs`:
```rust
pub fn compute_new_tags(
    current: &[String],
    mapping: &HashMap<String, Vec<String>>,
) -> Vec<String> {
    use std::collections::BTreeSet;

    let mut acc: BTreeSet<String> = BTreeSet::new();
    for tag in current {
        let outputs = match mapping.get(tag) {
            Some(values) if !values.is_empty() => values.clone(),
            _ => vec![tag.clone()],
        };
        for out in outputs {
            let normalized = out.trim().to_lowercase();
            if !normalized.is_empty() {
                acc.insert(normalized);
            }
        }
    }
    acc.into_iter().collect()
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p boopmark-server tag_consolidation::tests`
Expected: PASS — all 7 tests green.

- [ ] **Step 6: Commit**

```bash
git add server/src/app/mod.rs server/src/app/tag_consolidation.rs
git commit -m "feat: add compute_new_tags helper for tag consolidation"
```

---

## Task 3: Add three new methods to `BookmarkRepository`

This task only updates the trait definition and the two existing `MockRepo` stub blocks. The Postgres implementation comes in Task 4. Splitting it this way keeps each commit small.

**Files:**
- Modify: `server/src/domain/ports/bookmark_repo.rs`
- Modify: `server/src/app/bookmarks.rs` (two `MockRepo` blocks at lines ~527 and ~1262)

- [ ] **Step 1: Add the new methods to the trait**

Edit `server/src/domain/ports/bookmark_repo.rs`. Replace the file contents with:
```rust
use crate::domain::bookmark::{Bookmark, BookmarkFilter, CreateBookmark, UpdateBookmark};
use crate::domain::error::DomainError;
use crate::domain::ports::tag_consolidator::TagSample;
use uuid::Uuid;

#[trait_variant::make(Send)]
pub trait BookmarkRepository: Send + Sync {
    async fn create(&self, user_id: Uuid, input: CreateBookmark) -> Result<Bookmark, DomainError>;
    async fn get(&self, id: Uuid, user_id: Uuid) -> Result<Bookmark, DomainError>;
    async fn list(
        &self,
        user_id: Uuid,
        filter: BookmarkFilter,
    ) -> Result<Vec<Bookmark>, DomainError>;
    async fn update(
        &self,
        id: Uuid,
        user_id: Uuid,
        input: UpdateBookmark,
    ) -> Result<Bookmark, DomainError>;
    async fn delete(&self, id: Uuid, user_id: Uuid) -> Result<(), DomainError>;
    async fn all_tags(&self, user_id: Uuid) -> Result<Vec<String>, DomainError>;
    async fn tags_with_counts(&self, user_id: Uuid) -> Result<Vec<(String, i64)>, DomainError>;
    async fn export_all(&self, user_id: Uuid) -> Result<Vec<Bookmark>, DomainError>;
    async fn find_by_url(&self, user_id: Uuid, url: &str) -> Result<Option<Bookmark>, DomainError>;
    async fn insert_with_id(&self, bookmark: Bookmark) -> Result<Bookmark, DomainError>;
    async fn upsert_full(&self, bookmark: Bookmark) -> Result<Bookmark, DomainError>;
    async fn update_image_url(
        &self,
        id: Uuid,
        user_id: Uuid,
        image_url: &str,
    ) -> Result<(), DomainError>;
    /// Returns each distinct tag with its bookmark count and up to 3 sample titles.
    async fn tag_samples(&self, user_id: Uuid) -> Result<Vec<TagSample>, DomainError>;
    /// Returns (id, tags) for every bookmark belonging to this user.
    async fn list_id_tags(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<(Uuid, Vec<String>)>, DomainError>;
    /// Replaces tags on the given bookmarks (must all belong to user_id) in a single
    /// transaction. Returns the count of rows actually written.
    async fn update_tags_bulk(
        &self,
        user_id: Uuid,
        updates: &[(Uuid, Vec<String>)],
    ) -> Result<u64, DomainError>;
}
```

- [ ] **Step 2: Add stubs to the first `MockRepo` block**

In `server/src/app/bookmarks.rs`, find the `impl BookmarkRepository for MockRepo` block that starts around line 527. Add these methods (alongside the other stub methods like `all_tags`):
```rust
async fn tag_samples(
    &self,
    _user_id: Uuid,
) -> Result<Vec<crate::domain::ports::tag_consolidator::TagSample>, DomainError> {
    Ok(vec![])
}
async fn list_id_tags(
    &self,
    user_id: Uuid,
) -> Result<Vec<(Uuid, Vec<String>)>, DomainError> {
    Ok(self
        .bookmarks
        .lock()
        .unwrap()
        .iter()
        .filter(|b| b.user_id == user_id)
        .map(|b| (b.id, b.tags.clone()))
        .collect())
}
async fn update_tags_bulk(
    &self,
    user_id: Uuid,
    updates: &[(Uuid, Vec<String>)],
) -> Result<u64, DomainError> {
    let mut bookmarks = self.bookmarks.lock().unwrap();
    let mut rows = 0u64;
    for (id, new_tags) in updates {
        if let Some(b) = bookmarks
            .iter_mut()
            .find(|b| b.id == *id && b.user_id == user_id)
        {
            b.tags = new_tags.clone();
            rows += 1;
        }
    }
    Ok(rows)
}
```

- [ ] **Step 3: Add identical stubs to the second `MockRepo` block**

The second `MockRepo` is around line 1262. Add the same three methods to it. (Repeat the exact code from Step 2 — the engineer may be reading tasks out of order; do not reference Step 2.)

```rust
async fn tag_samples(
    &self,
    _user_id: Uuid,
) -> Result<Vec<crate::domain::ports::tag_consolidator::TagSample>, DomainError> {
    Ok(vec![])
}
async fn list_id_tags(
    &self,
    user_id: Uuid,
) -> Result<Vec<(Uuid, Vec<String>)>, DomainError> {
    Ok(self
        .bookmarks
        .lock()
        .unwrap()
        .iter()
        .filter(|b| b.user_id == user_id)
        .map(|b| (b.id, b.tags.clone()))
        .collect())
}
async fn update_tags_bulk(
    &self,
    user_id: Uuid,
    updates: &[(Uuid, Vec<String>)],
) -> Result<u64, DomainError> {
    let mut bookmarks = self.bookmarks.lock().unwrap();
    let mut rows = 0u64;
    for (id, new_tags) in updates {
        if let Some(b) = bookmarks
            .iter_mut()
            .find(|b| b.id == *id && b.user_id == user_id)
        {
            b.tags = new_tags.clone();
            rows += 1;
        }
    }
    Ok(rows)
}
```

If the second `MockRepo` uses field names different from the first (re-read the surrounding struct definition before editing), match its conventions; the algorithm is identical.

- [ ] **Step 4: Verify the workspace still compiles (Postgres impl will fail; that's expected next)**

Run: `cargo check -p boopmark-server`
Expected: FAIL — `BookmarkRepository for PostgresPool` is missing the three new methods. This proves the trait change has bite.

- [ ] **Step 5: Add temporary `unimplemented!()` stubs to `PostgresPool`**

We need the workspace to build between trait change and Postgres impl. Add to `server/src/adapters/postgres/bookmark_repo.rs`, at the very end of the `impl BookmarkRepository for PostgresPool` block (just before the closing `}`):
```rust
async fn tag_samples(
    &self,
    _user_id: Uuid,
) -> Result<Vec<crate::domain::ports::tag_consolidator::TagSample>, DomainError> {
    unimplemented!("implemented in next task")
}

async fn list_id_tags(
    &self,
    _user_id: Uuid,
) -> Result<Vec<(Uuid, Vec<String>)>, DomainError> {
    unimplemented!("implemented in next task")
}

async fn update_tags_bulk(
    &self,
    _user_id: Uuid,
    _updates: &[(Uuid, Vec<String>)],
) -> Result<u64, DomainError> {
    unimplemented!("implemented in next task")
}
```

- [ ] **Step 6: Verify build + existing tests pass**

Run: `cargo test -p boopmark-server`
Expected: PASS — all existing tests still green. New stub methods are unused so no warnings about that.

- [ ] **Step 7: Commit**

```bash
git add server/src/domain/ports/bookmark_repo.rs server/src/app/bookmarks.rs server/src/adapters/postgres/bookmark_repo.rs
git commit -m "feat: add tag-consolidation methods to BookmarkRepository trait"
```

---

## Task 4: Implement Postgres methods

**Files:**
- Modify: `server/src/adapters/postgres/bookmark_repo.rs`

These are SQL-only changes. Existing pattern uses `sqlx::query_as` / `sqlx::query` against the pool; we follow it exactly. There are no integration tests in this codebase for the Postgres impl — we verify via `cargo build` + the manual smoke test in Task 9.

- [ ] **Step 1: Replace `tag_samples` stub with real query**

In `server/src/adapters/postgres/bookmark_repo.rs`, replace the `tag_samples` body:
```rust
async fn tag_samples(
    &self,
    user_id: Uuid,
) -> Result<Vec<crate::domain::ports::tag_consolidator::TagSample>, DomainError> {
    use crate::domain::ports::tag_consolidator::TagSample;

    let rows: Vec<(String, i64, Vec<String>)> = sqlx::query_as(
        "WITH expanded AS (
             SELECT id, title, created_at, unnest(tags) AS tag
             FROM bookmarks
             WHERE user_id = $1
         ),
         counts AS (
             SELECT tag, COUNT(*) AS count
             FROM expanded
             GROUP BY tag
         ),
         ranked AS (
             SELECT
                 tag,
                 title,
                 ROW_NUMBER() OVER (PARTITION BY tag ORDER BY created_at DESC, id DESC) AS rn
             FROM expanded
             WHERE title IS NOT NULL AND title <> ''
         )
         SELECT
             c.tag,
             c.count,
             COALESCE(
                 ARRAY_AGG(r.title ORDER BY r.rn) FILTER (WHERE r.rn IS NOT NULL),
                 ARRAY[]::TEXT[]
             ) AS sample_titles
         FROM counts c
         LEFT JOIN ranked r ON r.tag = c.tag AND r.rn <= 3
         GROUP BY c.tag, c.count
         ORDER BY c.count DESC, c.tag ASC",
    )
    .bind(user_id)
    .fetch_all(&self.pool)
    .await
    .map_err(|e| DomainError::Internal(e.to_string()))?;

    Ok(rows
        .into_iter()
        .map(|(tag, count, sample_titles)| TagSample {
            tag,
            count,
            sample_titles,
        })
        .collect())
}
```

- [ ] **Step 2: Replace `list_id_tags` stub**

In the same file:
```rust
async fn list_id_tags(
    &self,
    user_id: Uuid,
) -> Result<Vec<(Uuid, Vec<String>)>, DomainError> {
    let rows: Vec<(Uuid, Vec<String>)> = sqlx::query_as(
        "SELECT id, tags FROM bookmarks WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_all(&self.pool)
    .await
    .map_err(|e| DomainError::Internal(e.to_string()))?;
    Ok(rows)
}
```

- [ ] **Step 3: Replace `update_tags_bulk` stub with a transaction**

In the same file:
```rust
async fn update_tags_bulk(
    &self,
    user_id: Uuid,
    updates: &[(Uuid, Vec<String>)],
) -> Result<u64, DomainError> {
    if updates.is_empty() {
        return Ok(0);
    }

    let mut tx = self
        .pool
        .begin()
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?;

    let mut rows: u64 = 0;
    for (id, new_tags) in updates {
        let r = sqlx::query(
            "UPDATE bookmarks
             SET tags = $1, updated_at = now()
             WHERE id = $2 AND user_id = $3",
        )
        .bind(new_tags)
        .bind(id)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?;
        rows += r.rows_affected();
    }

    tx.commit()
        .await
        .map_err(|e| DomainError::Internal(e.to_string()))?;
    Ok(rows)
}
```

- [ ] **Step 4: Verify build + tests**

Run: `cargo test -p boopmark-server`
Expected: PASS — all existing tests green; no panic from stubs anymore (though no test exercises these directly yet, that comes in Task 6).

- [ ] **Step 5: Commit**

```bash
git add server/src/adapters/postgres/bookmark_repo.rs
git commit -m "feat: implement Postgres tag_samples, list_id_tags, update_tags_bulk"
```

---

## Task 5: Implement `AnthropicTagConsolidator`

Mirrors `adapters/anthropic.rs` (HTTP plumbing, JSON-extraction). Adds prompt builder + response parser for the consolidator shape.

**Files:**
- Create: `server/src/adapters/anthropic_tag_consolidator.rs`
- Modify: `server/src/adapters/mod.rs`

- [ ] **Step 1: Register the new module**

Add to `server/src/adapters/mod.rs`:
```rust
pub mod anthropic;
pub mod anthropic_tag_consolidator;
pub mod login;
pub mod metadata;
pub mod postgres;
pub mod screenshot;
pub mod storage;
```

- [ ] **Step 2: Write the prompt-builder tests (failing)**

Create `server/src/adapters/anthropic_tag_consolidator.rs` with a stub and tests:
```rust
use crate::domain::error::DomainError;
use crate::domain::ports::tag_consolidator::{
    ConsolidationInput, ConsolidationOutput, TagConsolidator, TagSample,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

#[derive(Clone)]
pub struct AnthropicTagConsolidator {
    client: reqwest::Client,
}

impl AnthropicTagConsolidator {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    fn build_prompt(input: &ConsolidationInput) -> String {
        let _ = input;
        todo!("implement in step 4")
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

/// Extract the first JSON object from a text response by finding the first `{`
/// and last `}`. Handles markdown fences or stray text around the JSON.
fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end >= start {
        Some(&text[start..=end])
    } else {
        None
    }
}

impl TagConsolidator for AnthropicTagConsolidator {
    fn consolidate(
        &self,
        api_key: &str,
        model: &str,
        input: ConsolidationInput,
    ) -> Pin<Box<dyn Future<Output = Result<ConsolidationOutput, DomainError>> + Send + '_>> {
        let api_key = api_key.to_string();
        let model = model.to_string();
        Box::pin(async move { self.do_consolidate(&api_key, &model, input).await })
    }
}

impl AnthropicTagConsolidator {
    async fn do_consolidate(
        &self,
        api_key: &str,
        model: &str,
        input: ConsolidationInput,
    ) -> Result<ConsolidationOutput, DomainError> {
        let prompt = Self::build_prompt(&input);

        let request_body = AnthropicRequest {
            model: model.to_string(),
            max_tokens: 4096,
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

        let json_str = extract_json_object(&text).ok_or_else(|| {
            DomainError::Internal("LLM response contained no JSON object".to_string())
        })?;

        let mapping: HashMap<String, Vec<String>> = serde_json::from_str(json_str)
            .map_err(|e| DomainError::Internal(format!("LLM JSON parse error: {e}")))?;

        Ok(ConsolidationOutput { mapping })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_input() -> ConsolidationInput {
        ConsolidationInput {
            tags: vec![
                TagSample {
                    tag: "js".to_string(),
                    count: 12,
                    sample_titles: vec![
                        "Promise.all guide".to_string(),
                        "Async iterators".to_string(),
                    ],
                },
                TagSample {
                    tag: "javascript".to_string(),
                    count: 8,
                    sample_titles: vec!["ECMAScript 2024 features".to_string()],
                },
            ],
        }
    }

    #[test]
    fn prompt_includes_each_tag_with_count_and_samples() {
        let prompt = AnthropicTagConsolidator::build_prompt(&sample_input());
        assert!(prompt.contains("\"js\""), "missing js: {prompt}");
        assert!(prompt.contains("(12)"), "missing count: {prompt}");
        assert!(prompt.contains("Promise.all guide"), "missing sample: {prompt}");
        assert!(prompt.contains("\"javascript\""), "missing javascript: {prompt}");
    }

    #[test]
    fn prompt_instructs_lowercase_and_json_only() {
        let prompt = AnthropicTagConsolidator::build_prompt(&sample_input());
        assert!(prompt.to_lowercase().contains("lowercase"));
        assert!(prompt.to_lowercase().contains("json"));
    }

    #[test]
    fn prompt_describes_parent_tag_rule() {
        let prompt = AnthropicTagConsolidator::build_prompt(&sample_input());
        let lower = prompt.to_lowercase();
        assert!(lower.contains("parent"));
        assert!(lower.contains("not replace") || lower.contains("do not replace"));
    }

    #[test]
    fn extract_json_handles_markdown_fences() {
        let text = "```json\n{\"js\": [\"javascript\"]}\n```";
        let json = extract_json_object(text).expect("json");
        let parsed: HashMap<String, Vec<String>> = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.get("js"), Some(&vec!["javascript".to_string()]));
    }

    #[test]
    fn extract_json_handles_leading_text() {
        let text = "Here you go:\n{\"js\": [\"javascript\"]}\n";
        let json = extract_json_object(text).expect("json");
        let parsed: HashMap<String, Vec<String>> = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.get("js"), Some(&vec!["javascript".to_string()]));
    }

    #[test]
    fn extract_json_returns_none_for_no_braces() {
        assert!(extract_json_object("nothing here").is_none());
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p boopmark-server anthropic_tag_consolidator::tests`
Expected: prompt-builder tests panic with `not yet implemented` from `todo!()`. The `extract_json_*` tests should already pass (no `todo!` in that path).

- [ ] **Step 4: Implement `build_prompt`**

Replace the body of `build_prompt` in `server/src/adapters/anthropic_tag_consolidator.rs`:
```rust
fn build_prompt(input: &ConsolidationInput) -> String {
    let mut tag_lines = String::new();
    for sample in &input.tags {
        let titles = if sample.sample_titles.is_empty() {
            "(no sample titles)".to_string()
        } else {
            sample
                .sample_titles
                .iter()
                .map(|t| format!("\"{}\"", t.replace('"', "\\\"")))
                .collect::<Vec<_>>()
                .join(", ")
        };
        tag_lines.push_str(&format!(
            "- \"{}\" ({}): {}\n",
            sample.tag.replace('"', "\\\""),
            sample.count,
            titles
        ));
    }

    format!(
        "You are a bookmark tag organizer. The user has the following tags on their bookmarks. \
         For each tag, decide what tag(s) a bookmark currently carrying it should end up with.\n\n\
         Rules:\n\
         1. Merge variants, synonyms, and typos into a single canonical form. \
         Example: \"js\", \"javascript\", \"JavaScript\" should all map to [\"javascript\"].\n\
         2. You MAY add a broader parent tag alongside a narrow tag. Do NOT replace the narrow tag. \
         Example: \"react\" might map to [\"react\", \"frontend\"].\n\
         3. Do not invent tags unrelated to the input set or the user's apparent topics.\n\
         4. Use lowercase. Prefer the most common, idiomatic form.\n\
         5. Every input tag MUST be a key in your output. If no change, return the tag itself: \"rust\" -> [\"rust\"].\n\n\
         Tags (with bookmark count and up to 3 sample titles per tag):\n\
         {tag_lines}\n\
         Respond with ONLY valid JSON, no other text. The format is an object whose keys are the input tags \
         (exact case as given) and whose values are arrays of output tag strings:\n\
         {{\"input_tag_1\": [\"output_a\", \"output_b\"], \"input_tag_2\": [\"output_c\"]}}"
    )
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p boopmark-server anthropic_tag_consolidator::tests`
Expected: PASS — all 6 tests green.

- [ ] **Step 6: Commit**

```bash
git add server/src/adapters/mod.rs server/src/adapters/anthropic_tag_consolidator.rs
git commit -m "feat: add AnthropicTagConsolidator adapter"
```

---

## Task 6: Implement `TagConsolidationService`

Wires the port + repo + settings together. Has a stubbed-port unit test that exercises the full happy path and the key edge cases.

**Files:**
- Modify: `server/src/app/tag_consolidation.rs`

- [ ] **Step 1: Add the service skeleton (not yet implemented) + tests**

Edit `server/src/app/tag_consolidation.rs`. Keep the existing `compute_new_tags` + its tests. Append:
```rust
use crate::app::settings::SettingsService;
use crate::domain::error::DomainError;
use crate::domain::ports::bookmark_repo::BookmarkRepository;
use crate::domain::ports::llm_settings_repo::LlmSettingsRepository;
use crate::domain::ports::tag_consolidator::{ConsolidationInput, TagConsolidator};
use std::sync::Arc;
use uuid::Uuid;

pub const MIN_TAGS_FOR_CONSOLIDATION: usize = 5;

#[derive(Debug, Clone, serde::Serialize)]
pub struct ConsolidationStats {
    pub bookmarks_changed: u64,
    pub tags_before: usize,
    pub tags_after: usize,
}

pub struct TagConsolidationService<B, R> {
    bookmarks: Arc<B>,
    consolidator: Arc<dyn TagConsolidator>,
    settings: Arc<SettingsService<R>>,
}

impl<B, R> TagConsolidationService<B, R>
where
    B: BookmarkRepository + Send + Sync,
    R: LlmSettingsRepository + Send + Sync,
{
    pub fn new(
        bookmarks: Arc<B>,
        consolidator: Arc<dyn TagConsolidator>,
        settings: Arc<SettingsService<R>>,
    ) -> Self {
        Self {
            bookmarks,
            consolidator,
            settings,
        }
    }

    pub async fn consolidate(
        &self,
        user_id: Uuid,
    ) -> Result<ConsolidationStats, DomainError> {
        let _ = user_id;
        todo!("implement in step 3")
    }
}

#[cfg(test)]
mod service_tests {
    use super::*;
    use crate::app::secrets::SecretBox;
    use crate::app::settings::SaveLlmSettingsInput;
    use crate::domain::bookmark::{
        Bookmark, BookmarkFilter, CreateBookmark, UpdateBookmark,
    };
    use crate::domain::llm_settings::LlmSettings;
    use crate::domain::ports::llm_settings_repo::LlmSettingsRepository;
    use crate::domain::ports::tag_consolidator::{
        ConsolidationInput, ConsolidationOutput, TagConsolidator, TagSample,
    };
    use chrono::Utc;
    use std::collections::HashMap;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;

    // ---- Stub LLM settings repo (saves and returns one record) ----
    struct StubLlmSettingsRepo {
        stored: Mutex<Option<LlmSettings>>,
    }
    impl StubLlmSettingsRepo {
        fn new() -> Self {
            Self {
                stored: Mutex::new(None),
            }
        }
    }
    impl LlmSettingsRepository for StubLlmSettingsRepo {
        async fn get(&self, _user_id: Uuid) -> Result<Option<LlmSettings>, DomainError> {
            Ok(self.stored.lock().unwrap().clone())
        }
        async fn upsert(
            &self,
            user_id: Uuid,
            enabled: bool,
            replace: Option<&[u8]>,
            clear: bool,
            model: &str,
        ) -> Result<LlmSettings, DomainError> {
            let existing = self.stored.lock().unwrap().clone();
            let encrypted = if clear {
                None
            } else {
                replace
                    .map(|v| v.to_vec())
                    .or_else(|| existing.as_ref().and_then(|s| s.anthropic_api_key_encrypted.clone()))
            };
            let saved = LlmSettings {
                user_id,
                enabled,
                anthropic_api_key_encrypted: encrypted,
                anthropic_model: model.to_string(),
                created_at: existing.map(|s| s.created_at).unwrap_or_else(Utc::now),
                updated_at: Utc::now(),
            };
            *self.stored.lock().unwrap() = Some(saved.clone());
            Ok(saved)
        }
    }

    // ---- Stub bookmark repo (only the methods used by the service matter) ----
    struct StubBookmarkRepo {
        bookmarks: Mutex<Vec<Bookmark>>,
        samples: Vec<TagSample>,
    }
    impl StubBookmarkRepo {
        fn new(bookmarks: Vec<Bookmark>, samples: Vec<TagSample>) -> Self {
            Self {
                bookmarks: Mutex::new(bookmarks),
                samples,
            }
        }
    }
    impl BookmarkRepository for StubBookmarkRepo {
        async fn create(&self, _: Uuid, _: CreateBookmark) -> Result<Bookmark, DomainError> {
            unimplemented!()
        }
        async fn get(&self, _: Uuid, _: Uuid) -> Result<Bookmark, DomainError> {
            unimplemented!()
        }
        async fn list(&self, _: Uuid, _: BookmarkFilter) -> Result<Vec<Bookmark>, DomainError> {
            unimplemented!()
        }
        async fn update(
            &self,
            _: Uuid,
            _: Uuid,
            _: UpdateBookmark,
        ) -> Result<Bookmark, DomainError> {
            unimplemented!()
        }
        async fn delete(&self, _: Uuid, _: Uuid) -> Result<(), DomainError> {
            unimplemented!()
        }
        async fn all_tags(&self, _: Uuid) -> Result<Vec<String>, DomainError> {
            unimplemented!()
        }
        async fn tags_with_counts(&self, _: Uuid) -> Result<Vec<(String, i64)>, DomainError> {
            unimplemented!()
        }
        async fn export_all(&self, _: Uuid) -> Result<Vec<Bookmark>, DomainError> {
            unimplemented!()
        }
        async fn find_by_url(&self, _: Uuid, _: &str) -> Result<Option<Bookmark>, DomainError> {
            unimplemented!()
        }
        async fn insert_with_id(&self, _: Bookmark) -> Result<Bookmark, DomainError> {
            unimplemented!()
        }
        async fn upsert_full(&self, _: Bookmark) -> Result<Bookmark, DomainError> {
            unimplemented!()
        }
        async fn update_image_url(&self, _: Uuid, _: Uuid, _: &str) -> Result<(), DomainError> {
            unimplemented!()
        }
        async fn tag_samples(&self, _: Uuid) -> Result<Vec<TagSample>, DomainError> {
            Ok(self.samples.clone())
        }
        async fn list_id_tags(
            &self,
            user_id: Uuid,
        ) -> Result<Vec<(Uuid, Vec<String>)>, DomainError> {
            Ok(self
                .bookmarks
                .lock()
                .unwrap()
                .iter()
                .filter(|b| b.user_id == user_id)
                .map(|b| (b.id, b.tags.clone()))
                .collect())
        }
        async fn update_tags_bulk(
            &self,
            user_id: Uuid,
            updates: &[(Uuid, Vec<String>)],
        ) -> Result<u64, DomainError> {
            let mut bookmarks = self.bookmarks.lock().unwrap();
            let mut rows = 0u64;
            for (id, new_tags) in updates {
                if let Some(b) = bookmarks
                    .iter_mut()
                    .find(|b| b.id == *id && b.user_id == user_id)
                {
                    b.tags = new_tags.clone();
                    rows += 1;
                }
            }
            Ok(rows)
        }
    }

    // ---- Stub TagConsolidator that returns a canned mapping ----
    struct StubConsolidator {
        mapping: HashMap<String, Vec<String>>,
    }
    impl StubConsolidator {
        fn new(pairs: &[(&str, &[&str])]) -> Self {
            Self {
                mapping: pairs
                    .iter()
                    .map(|(k, vs)| {
                        ((*k).to_string(), vs.iter().map(|v| (*v).to_string()).collect())
                    })
                    .collect(),
            }
        }
    }
    impl TagConsolidator for StubConsolidator {
        fn consolidate(
            &self,
            _api_key: &str,
            _model: &str,
            _input: ConsolidationInput,
        ) -> Pin<Box<dyn Future<Output = Result<ConsolidationOutput, DomainError>> + Send + '_>>
        {
            let mapping = self.mapping.clone();
            Box::pin(async move { Ok(ConsolidationOutput { mapping }) })
        }
    }

    // ---- Helpers ----
    fn bookmark(user_id: Uuid, tags: &[&str]) -> Bookmark {
        Bookmark {
            id: Uuid::new_v4(),
            user_id,
            url: "https://example.com".into(),
            title: Some("t".into()),
            description: None,
            image_url: None,
            domain: None,
            tags: tags.iter().map(|t| (*t).to_string()).collect(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn samples(tags: &[(&str, i64)]) -> Vec<TagSample> {
        tags.iter()
            .map(|(t, c)| TagSample {
                tag: (*t).to_string(),
                count: *c,
                sample_titles: vec![],
            })
            .collect()
    }

    async fn make_settings(
        api_key: Option<&str>,
    ) -> Arc<SettingsService<StubLlmSettingsRepo>> {
        let repo = Arc::new(StubLlmSettingsRepo::new());
        let secret_box =
            Arc::new(SecretBox::new("MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY="));
        let svc = Arc::new(SettingsService::new(repo, secret_box));
        if let Some(key) = api_key {
            svc.save(
                Uuid::new_v4(), // not the user under test, but the stub stores globally — we'll save for our user below
                SaveLlmSettingsInput {
                    enabled: true,
                    anthropic_api_key: Some(key.to_string()),
                    clear_anthropic_api_key: false,
                    anthropic_model: Some("claude-haiku-4-5-20251001".into()),
                },
            )
            .await
            .expect("save");
        }
        svc
    }

    #[tokio::test]
    async fn returns_invalid_input_when_no_api_key() {
        let user_id = Uuid::new_v4();
        let bookmark_repo = Arc::new(StubBookmarkRepo::new(vec![], vec![]));
        let consolidator = Arc::new(StubConsolidator::new(&[]));
        // No api key saved
        let settings = make_settings(None).await;
        let service =
            TagConsolidationService::new(bookmark_repo, consolidator, settings);

        let err = service.consolidate(user_id).await.err().expect("should err");
        assert!(matches!(err, DomainError::InvalidInput(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn returns_invalid_input_when_too_few_tags() {
        let user_id = Uuid::new_v4();
        let bookmark_repo = Arc::new(StubBookmarkRepo::new(
            vec![bookmark(user_id, &["a"])],
            samples(&[("a", 1)]), // 1 < MIN_TAGS_FOR_CONSOLIDATION (5)
        ));
        let consolidator = Arc::new(StubConsolidator::new(&[]));
        // Save a key so we get past the API-key gate, by stubbing settings
        // directly (the make_settings helper saves for a different uuid).
        let llm_repo = Arc::new(StubLlmSettingsRepo::new());
        let secret_box =
            Arc::new(SecretBox::new("MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY="));
        let settings = Arc::new(SettingsService::new(llm_repo, secret_box));
        settings
            .save(
                user_id,
                SaveLlmSettingsInput {
                    enabled: true,
                    anthropic_api_key: Some("sk-ant-x".into()),
                    clear_anthropic_api_key: false,
                    anthropic_model: Some("claude-haiku-4-5-20251001".into()),
                },
            )
            .await
            .expect("save");

        let service = TagConsolidationService::new(bookmark_repo, consolidator, settings);
        let err = service.consolidate(user_id).await.err().expect("should err");
        assert!(matches!(err, DomainError::InvalidInput(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn applies_mapping_and_returns_stats() {
        let user_id = Uuid::new_v4();
        let bookmarks = vec![
            bookmark(user_id, &["js", "react"]),
            bookmark(user_id, &["JavaScript", "vue"]),
            bookmark(user_id, &["rust"]),
            bookmark(user_id, &["javascript"]),
            bookmark(user_id, &["react"]),
        ];
        let bookmark_repo = Arc::new(StubBookmarkRepo::new(
            bookmarks,
            samples(&[
                ("js", 1),
                ("JavaScript", 1),
                ("javascript", 1),
                ("react", 2),
                ("vue", 1),
                ("rust", 1),
            ]),
        ));
        let consolidator = Arc::new(StubConsolidator::new(&[
            ("js", &["javascript"]),
            ("JavaScript", &["javascript"]),
            ("javascript", &["javascript"]),
            ("react", &["react", "frontend"]),
            ("vue", &["vue", "frontend"]),
            ("rust", &["rust"]),
        ]));

        let llm_repo = Arc::new(StubLlmSettingsRepo::new());
        let secret_box =
            Arc::new(SecretBox::new("MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY="));
        let settings = Arc::new(SettingsService::new(llm_repo, secret_box));
        settings
            .save(
                user_id,
                SaveLlmSettingsInput {
                    enabled: true,
                    anthropic_api_key: Some("sk-ant-x".into()),
                    clear_anthropic_api_key: false,
                    anthropic_model: Some("claude-haiku-4-5-20251001".into()),
                },
            )
            .await
            .expect("save");

        let service =
            TagConsolidationService::new(bookmark_repo.clone(), consolidator, settings);
        let stats = service.consolidate(user_id).await.expect("ok");

        // 6 distinct input tags. After applying the mapping, the unique output
        // tags across all bookmarks are: javascript, react, frontend, vue, rust.
        assert_eq!(stats.tags_before, 6);
        assert_eq!(stats.tags_after, 5);
        // 5 bookmarks; only the lone "rust" and lone "javascript" bookmarks
        // are unchanged (the mapping is identity for them after sort/dedupe).
        assert_eq!(stats.bookmarks_changed, 3);

        // Verify some bookmark states.
        let bookmarks = bookmark_repo.bookmarks.lock().unwrap();
        // "rust" bookmark unchanged.
        assert!(
            bookmarks.iter().any(|b| b.tags == vec!["rust".to_string()]),
            "expected an unchanged rust bookmark"
        );
        // The lone "javascript" bookmark unchanged.
        assert!(
            bookmarks.iter().any(|b| b.tags == vec!["javascript".to_string()]),
            "expected an unchanged javascript bookmark"
        );
        // A "js"+"react" bookmark should now be ["frontend", "javascript", "react"].
        assert!(
            bookmarks.iter().any(|b| b.tags
                == vec![
                    "frontend".to_string(),
                    "javascript".to_string(),
                    "react".to_string()
                ]),
            "expected merged js+react bookmark"
        );
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p boopmark-server tag_consolidation::service_tests`
Expected: FAIL — all three async tests panic with `not yet implemented` from `todo!()`.

- [ ] **Step 3: Implement `consolidate`**

Replace the body of `TagConsolidationService::consolidate` in `server/src/app/tag_consolidation.rs`:
```rust
pub async fn consolidate(
    &self,
    user_id: Uuid,
) -> Result<ConsolidationStats, DomainError> {
    // 1. Get API key + model.
    let (api_key, model) = self
        .settings
        .get_decrypted_api_key(user_id)
        .await?
        .ok_or_else(|| {
            DomainError::InvalidInput(
                "No Anthropic API key configured. Save one in Settings first.".to_string(),
            )
        })?;

    // 2. Load tag samples.
    let samples = self.bookmarks.tag_samples(user_id).await?;
    if samples.len() < MIN_TAGS_FOR_CONSOLIDATION {
        return Err(DomainError::InvalidInput(format!(
            "Need at least {MIN_TAGS_FOR_CONSOLIDATION} tags to consolidate; you have {}.",
            samples.len()
        )));
    }
    let tags_before = samples.len();

    // 3. Ask the LLM.
    let output = self
        .consolidator
        .consolidate(&api_key, &model, ConsolidationInput { tags: samples })
        .await?;

    // 4. Compute per-bookmark new tags.
    let id_tags = self.bookmarks.list_id_tags(user_id).await?;
    let mut updates: Vec<(Uuid, Vec<String>)> = Vec::new();
    let mut after_set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (id, current) in id_tags {
        let new_tags = compute_new_tags(&current, &output.mapping);
        for t in &new_tags {
            after_set.insert(t.clone());
        }
        // Compare ignoring order; current may not be sorted but compute_new_tags returns sorted.
        let mut current_sorted = current.clone();
        current_sorted.sort();
        current_sorted.dedup();
        let mut new_sorted = new_tags.clone();
        new_sorted.sort();
        new_sorted.dedup();
        if current_sorted != new_sorted {
            updates.push((id, new_tags));
        }
    }

    // 5. Apply.
    let bookmarks_changed = self
        .bookmarks
        .update_tags_bulk(user_id, &updates)
        .await?;

    Ok(ConsolidationStats {
        bookmarks_changed,
        tags_before,
        tags_after: after_set.len(),
    })
}
```

- [ ] **Step 4: Run tests to verify all pass**

Run: `cargo test -p boopmark-server tag_consolidation`
Expected: PASS — both `tests` (compute_new_tags) and `service_tests` modules green.

- [ ] **Step 5: Commit**

```bash
git add server/src/app/tag_consolidation.rs
git commit -m "feat: add TagConsolidationService with stubbed unit tests"
```

---

## Task 7: Wire the service into `AppState` and `main.rs`

**Files:**
- Modify: `server/src/web/state.rs`
- Modify: `server/src/main.rs`

- [ ] **Step 1: Add the service field to `AppState`**

In `server/src/web/state.rs`, update the imports + the struct. Replace:
```rust
use crate::app::auth::AuthService;
use crate::app::bookmarks::BookmarkService;
use crate::app::enrichment::EnrichmentService;
use crate::app::invite::InviteService;
use crate::app::settings::SettingsService;
```
with:
```rust
use crate::app::auth::AuthService;
use crate::app::bookmarks::BookmarkService;
use crate::app::enrichment::EnrichmentService;
use crate::app::invite::InviteService;
use crate::app::settings::SettingsService;
use crate::app::tag_consolidation::TagConsolidationService;
```

In the same file, find `pub struct AppState { ... }`. Add a new field after `enrichment`:
```rust
pub tag_consolidation: Arc<TagConsolidationService<PostgresPool, PostgresPool>>,
```

- [ ] **Step 2: Construct and register in `main.rs`**

In `server/src/main.rs`, near the other adapter and service constructions (search for `enricher: Arc<dyn LlmEnricher>` — around line 156), add right after the `enrichment_service` block:
```rust
let tag_consolidator: Arc<dyn domain::ports::tag_consolidator::TagConsolidator> =
    Arc::new(adapters::anthropic_tag_consolidator::AnthropicTagConsolidator::new());
let tag_consolidation_service = Arc::new(
    app::tag_consolidation::TagConsolidationService::new(
        db.clone(),
        tag_consolidator,
        settings_service.clone(),
    ),
);
```

Also add to the `AppState { ... }` literal (search for `enrichment: enrichment_service,` and add right after):
```rust
tag_consolidation: tag_consolidation_service,
```

- [ ] **Step 3: Verify build**

Run: `cargo build -p boopmark-server`
Expected: PASS — compiles cleanly. Existing tests still pass (`cargo test -p boopmark-server`).

- [ ] **Step 4: Commit**

```bash
git add server/src/web/state.rs server/src/main.rs
git commit -m "feat: wire TagConsolidationService into AppState"
```

---

## Task 8: Add the HTMX endpoint and result fragment

**Files:**
- Create: `templates/settings/tag_consolidation_result.html`
- Modify: `server/src/web/pages/settings.rs`

- [ ] **Step 1: Create the result fragment template**

Create `templates/settings/tag_consolidation_result.html`:
```html
{% if let Some(message) = success_message %}
<div
    data-testid="tag-consolidation-result"
    class="rounded-lg border border-emerald-500/40 bg-emerald-500/10 px-4 py-3 text-sm text-emerald-200"
>
    {{ message }}
</div>
{% endif %}
{% if let Some(message) = error_message %}
<div
    data-testid="tag-consolidation-error"
    class="rounded-lg border border-red-500/40 bg-red-500/10 px-4 py-3 text-sm text-red-200"
>
    {{ message }}
</div>
{% endif %}
```

- [ ] **Step 2: Add the handler and result template type to `settings.rs`**

Edit `server/src/web/pages/settings.rs`. Just below the existing `ApiKeysListFragment` template definition (look for `#[derive(Template)]` around line 38), add:
```rust
#[derive(Template)]
#[template(path = "settings/tag_consolidation_result.html")]
struct TagConsolidationResultFragment {
    success_message: Option<String>,
    error_message: Option<String>,
}
```

Then, just before `pub fn routes() -> Router<AppState> {`, add the handler:
```rust
async fn consolidate_tags_htmx(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> axum::response::Response {
    match state.tag_consolidation.consolidate(user.id).await {
        Ok(stats) => render(&TagConsolidationResultFragment {
            success_message: Some(format!(
                "Consolidated {} tag{tplural} into {} across {} bookmark{bplural}.",
                stats.tags_before,
                stats.tags_after,
                stats.bookmarks_changed,
                tplural = if stats.tags_before == 1 { "" } else { "s" },
                bplural = if stats.bookmarks_changed == 1 { "" } else { "s" },
            )),
            error_message: None,
        }),
        Err(DomainError::InvalidInput(msg)) => render(&TagConsolidationResultFragment {
            success_message: None,
            error_message: Some(msg),
        }),
        Err(_) => render(&TagConsolidationResultFragment {
            success_message: None,
            error_message: Some(
                "Consolidation failed. Try again.".to_string(),
            ),
        }),
    }
}
```

- [ ] **Step 3: Register the route**

In the same file, in the existing `pub fn routes() -> Router<AppState> { ... }`, add a new `.route(...)` call to the chain (place it next to `/settings/fix-images/stream`):
```rust
.route(
    "/settings/consolidate-tags",
    axum::routing::post(consolidate_tags_htmx),
)
```

The full `routes()` after the change should be:
```rust
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/settings",
            axum::routing::get(settings_page).post(save_settings),
        )
        .route(
            "/settings/api-keys",
            axum::routing::post(create_api_key_htmx),
        )
        .route(
            "/settings/api-keys/{id}",
            axum::routing::delete(delete_api_key_htmx),
        )
        .route(
            "/settings/fix-images/stream",
            axum::routing::get(fix_images_stream),
        )
        .route(
            "/settings/consolidate-tags",
            axum::routing::post(consolidate_tags_htmx),
        )
}
```

- [ ] **Step 4: Verify build**

Run: `cargo build -p boopmark-server`
Expected: PASS — compiles. Note: the template file is loaded at compile time by Askama, so a missing or malformed template will surface here.

Run: `cargo test -p boopmark-server`
Expected: PASS — existing tests still green.

- [ ] **Step 5: Commit**

```bash
git add templates/settings/tag_consolidation_result.html server/src/web/pages/settings.rs
git commit -m "feat: add /settings/consolidate-tags HTMX endpoint"
```

---

## Task 9: Add the "Tag Library" section to the settings page

The button is HTMX-driven: clicking it issues `POST /settings/consolidate-tags`, swaps the result fragment into a target div, and disables the button while the request is in flight. Disabled-when-no-key is rendered server-side using the existing `has_anthropic_api_key` flag already on the page context.

**Files:**
- Modify: `templates/settings/index.html`

- [ ] **Step 1: Insert the section above "Image Repair"**

In `templates/settings/index.html`, find the section `<section class="space-y-5">` whose `<h2>` is "Image Repair" (around line 184). Add this new section IMMEDIATELY BEFORE it:
```html
<section class="space-y-5" data-testid="tag-library-section">
    <div>
        <h2 class="text-lg font-semibold">Tag Library</h2>
        <p class="text-sm text-gray-400">Use AI to merge variants and add useful parent tags across your bookmarks.</p>
    </div>

    <div class="space-y-3">
        <button
            id="consolidate-tags-btn"
            type="button"
            data-testid="consolidate-tags-button"
            hx-post="/settings/consolidate-tags"
            hx-target="#consolidate-tags-result"
            hx-swap="innerHTML"
            hx-disabled-elt="this"
            hx-indicator="#consolidate-tags-spinner"
            class="px-4 py-2 bg-blue-600 hover:bg-blue-700 rounded-lg text-sm font-medium disabled:opacity-50 disabled:cursor-not-allowed"
            {% if !has_anthropic_api_key %}disabled title="Configure an Anthropic API key first."{% endif %}
        >
            <span class="inline-flex items-center gap-2">
                <span>Consolidate Tags</span>
                <span id="consolidate-tags-spinner" class="htmx-indicator">…</span>
            </span>
        </button>
        <div id="consolidate-tags-result"></div>
    </div>
</section>
```

- [ ] **Step 2: Build CSS / verify rendering**

Run: `cargo build -p boopmark-server`
Expected: PASS — Askama template change is compile-checked.

If the project uses Tailwind via a build step, also run the Tailwind build (the project ships with the macOS arm64 binary):
Run: `./tailwindcss-macos-arm64 -i ./static/css/input.css -o ./static/css/output.css`
(Skip this step if `static/css` does not have these files; the existing layout already contains the classes used here so it is generally a no-op.)

- [ ] **Step 3: Commit**

```bash
git add templates/settings/index.html
git commit -m "feat: add Tag Library settings section with consolidate button"
```

---

## Task 10: Manual smoke test against the local stack

This validates the wired-up feature end-to-end. There is no automated E2E for this feature (consistent with the project's existing settings-page features like Image Repair).

- [ ] **Step 1: Bring up the local stack**

Run (from the repo root): `devproxy up`
Expected: devproxy reports the proxy URL (e.g. `https://<slug>-boopmark.mysite.dev`).

- [ ] **Step 2: Sign in and seed messy tags**

Either via the UI or `just add-user`, sign in. Add ~6 bookmarks with deliberately messy tags:
- A bookmark tagged `js`
- A bookmark tagged `JavaScript`
- A bookmark tagged `javascript`
- A bookmark tagged `react`
- A bookmark tagged `vue`
- A bookmark tagged `rust`

Then go to **Settings**, save an Anthropic API key, and enable LLM integration.

- [ ] **Step 3: Click "Consolidate Tags"**

Visit `/settings`. Find the "Tag Library" section. Click **Consolidate Tags**.

Expected:
- Button disables and shows the spinner indicator.
- After ~5–15s, a green success banner appears below the button: e.g. *"Consolidated 6 tags into 3 across 5 bookmarks."* (Exact numbers depend on the LLM, but `tags_after` should be smaller than `tags_before`.)

- [ ] **Step 4: Verify the bookmarks**

Navigate to `/bookmarks`. Confirm:
- The variants `js` / `JavaScript` / `javascript` have collapsed to a single canonical tag (typically `javascript`).
- React/Vue bookmarks may have gained a `frontend` parent tag.
- The `rust` bookmark is unchanged.

- [ ] **Step 5: Click "Consolidate Tags" with too few tags**

Delete bookmarks until you have fewer than 5 distinct tags. Click the button again.

Expected: red banner with a message like *"Need at least 5 tags to consolidate; you have 2."*

- [ ] **Step 6: Click with no API key**

Go to Settings, delete the saved Anthropic key. Reload the page.

Expected: the **Consolidate Tags** button is rendered disabled with a hover tooltip *"Configure an Anthropic API key first."*

- [ ] **Step 7: If anything fails, open an issue or fix and re-test**

If a step fails, capture the error from the toast and the server logs (`docker compose logs server` or `devproxy logs`), then iterate. Otherwise, the feature is complete.

- [ ] **Step 8: Final commit (only if any docs/cleanup were needed)**

If smoke testing required no code changes, no commit is needed. Otherwise:
```bash
git add <files>
git commit -m "fix: <what you changed>"
```

---

## Self-Review Notes (already addressed)

- Spec coverage: every section in the spec has a task — UX (Task 9), LLM contract (Tasks 1, 5), apply algorithm (Tasks 2, 4, 6), architecture (Tasks 1–7), errors & edge cases (Tasks 2, 6, 9), testing (Tasks 2, 5, 6).
- No placeholders in code blocks; every step shows the actual code or command.
- Type/method names are consistent across tasks: `TagConsolidator`, `ConsolidationInput`, `ConsolidationOutput`, `TagSample`, `tag_samples`, `list_id_tags`, `update_tags_bulk`, `compute_new_tags`, `TagConsolidationService::consolidate`, `ConsolidationStats { bookmarks_changed, tags_before, tags_after }`, `MIN_TAGS_FOR_CONSOLIDATION`.
