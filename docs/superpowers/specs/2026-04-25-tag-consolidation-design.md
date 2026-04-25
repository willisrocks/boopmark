# Tag Consolidation — Design

**Status:** Draft
**Date:** 2026-04-25

## Summary

Add a one-shot, user-triggered "Consolidate Tags" action that uses the user's configured LLM to clean up their tag library across all bookmarks. The LLM merges variants/synonyms/typos into canonical forms and may add broader parent tags alongside narrow ones. There is no review step, no undo, and no preview — the user clicks once and the LLM rewrites tags across all of their bookmarks. Existing per-bookmark auto-tagging is unchanged; it already soft-prefers existing tags, and that behavior compounds naturally once the tag set is cleaner.

## Goals

- Let a user with a messy tag library (e.g. `js`, `javascript`, `JavaScript`, `JS`) clean it up in one click.
- Allow the LLM to *add* a broader parent tag alongside a narrow tag (e.g. add `frontend` to bookmarks tagged `react`) without replacing the narrow tag.
- Keep v1 architecturally minimal: synchronous request, single LLM call, single DB transaction.

## Non-Goals (v1)

- No undo / history of past consolidations.
- No preview / dry-run / diff before applying.
- No partial approval (per-merge toggles).
- No background / async execution.
- No automatic triggering (cron, threshold-based, etc.).
- No hierarchical tag relationships beyond a flat parent tag added alongside.
- No changes to the per-bookmark auto-tagging prompt — it already soft-prefers existing tags via `existing_tags`.

## UX

A new section on the **Settings page**, alongside the existing LLM/API key configuration, labeled **"Tag Library"**.

- Button: **"Consolidate Tags"**
- Subtext: *"Use AI to merge variants and add useful parent tags across your bookmarks."*
- Disabled when:
  - No LLM API key is configured (tooltip: *"Configure an API key first."*)
  - The user has fewer than 5 distinct tags (tooltip: *"Not enough tags to consolidate."*)

On click:
- Button enters a spinner state and is disabled.
- Request runs synchronously.
- On success: success toast (e.g. *"Consolidated N tags across M bookmarks."*) and the tag list / sidebar refreshes.
- On failure: error toast (*"Consolidation failed. Try again."*); no state change.

## LLM Contract

### Input

For each tag the user has, send:
- the tag name as stored
- the bookmark count
- up to 3 sample bookmark titles (most recent or arbitrary stable order)

### Prompt

The prompt instructs the LLM to:
- Decide, for each input tag, what tag(s) a bookmark currently carrying it should end up with.
- Merge variants, synonyms, and typos into one canonical form.
- Optionally add a broader parent tag alongside a narrow tag (do not replace the narrow tag).
- Not invent tags unrelated to the input set.
- Use lowercase and prefer the most common / idiomatic form.
- Return strict JSON — keys are input tag names, values are arrays of output tag names.

### Output

```json
{
  "js":         ["javascript"],
  "javascript": ["javascript"],
  "JavaScript": ["javascript"],
  "react":      ["react", "frontend"],
  "vue":        ["vue", "frontend"],
  "rust":       ["rust"]
}
```

The mapping value is the **full target tag list** for a bookmark with that key tag — not a delta. This makes the apply step a clean per-bookmark union.

## Apply Algorithm

For each bookmark belonging to the user, in a single Postgres transaction:

1. Take the bookmark's current tags.
2. For each tag, look up its mapping; if absent, treat as identity (`"foo" → ["foo"]`).
3. Take the union of all output lists; dedupe case-insensitively; sort.
4. Write back if different from current.

The whole consolidation runs in one DB transaction so a mid-run failure leaves the user's tag state intact.

## Architecture

The codebase uses hexagonal (ports-and-adapters) architecture; this feature follows the same pattern.

### New port

`server/src/domain/ports/tag_consolidator.rs`

```rust
pub trait TagConsolidator {
    fn consolidate(
        &self,
        api_key: &str,
        model: &str,
        input: ConsolidationInput,
    ) -> Pin<Box<dyn Future<Output = Result<ConsolidationOutput, DomainError>> + Send + '_>>;
}

pub struct TagSample {
    pub tag: String,
    pub count: i64,
    pub sample_titles: Vec<String>, // up to 3
}

pub struct ConsolidationInput {
    pub tags: Vec<TagSample>,
}

pub struct ConsolidationOutput {
    pub mapping: HashMap<String, Vec<String>>,
}
```

### New adapter

Extend the existing `AnthropicEnricher` (or add a sibling adapter that shares the HTTP client) to implement `TagConsolidator`. The HTTP plumbing, JSON-extraction helper, and error mapping are identical to the existing enricher; only the prompt and response shape differ.

### New repository methods

On `BookmarkRepository`:

- `tag_samples(user_id: Uuid) -> Result<Vec<TagSample>, DomainError>`
  - Returns each distinct tag the user has, with its bookmark count and up to 3 sample titles. Ordering of samples is stable but not otherwise meaningful.
- `apply_tag_mapping(user_id: Uuid, mapping: &HashMap<String, Vec<String>>) -> Result<ApplyStats, DomainError>`
  - Applies the mapping to every bookmark for `user_id` in a single transaction. Returns counts (e.g. bookmarks updated, tags before, tags after) for the success toast.

### New service

`server/src/app/tag_consolidation.rs` — `TagConsolidationService`:

- `consolidate(user_id: Uuid) -> Result<ApplyStats, DomainError>`
  1. Load user's decrypted API key + model via the existing `SettingsService`. If none, return a recognizable error mapped to 400 by the web layer.
  2. Load `tag_samples` from the bookmark repository.
  3. Call `TagConsolidator::consolidate`.
  4. Sanitize the returned mapping (see "Errors & Edge Cases" below).
  5. Call `apply_tag_mapping` and return the stats.

### New endpoint

`POST /api/settings/consolidate-tags`

- Auth: existing session/auth middleware.
- HTMX response: replaces the button with a "Consolidating…" indicator on submit, then with the result message (success or error) on completion. Issues a swap event so the tag list / sidebar can refresh.

### No changes to enrichment

The existing per-bookmark enrichment flow continues as-is. It already passes `existing_tags` and instructs the LLM to prefer them. After consolidation, the existing-tag set it sees is cleaner, so future auto-tagging tends toward the consolidated vocabulary without any prompt change.

## Errors & Edge Cases

- **No API key configured** — endpoint returns 400; UI button is also disabled client-side.
- **Fewer than 5 tags** — button disabled; endpoint also returns 400 if called.
- **LLM HTTP failure / non-2xx** — propagate as internal error; user sees failure toast; no DB writes.
- **LLM returns malformed JSON** — same as above; no DB writes.
- **LLM returns mapping keys that aren't real tags** — ignore those entries.
- **LLM omits some real tags from the mapping** — treat as identity (`"foo" → ["foo"]`); never silently delete.
- **LLM returns an empty value list for a tag** — treat as identity; never let the LLM nuke a tag.
- **All tags on a bookmark map to the same canonical tag** — result is that one tag (after dedupe).
- **Mapping output contains uppercase / mixed-case** — lowercase before applying (the prompt asks for lowercase, but we don't trust it).

## Testing

- **Adapter unit tests** — mirror the existing `AnthropicEnricher` tests:
  - Prompt includes URL/title context (sample titles).
  - JSON parsing handles markdown fences and leading text.
  - Malformed JSON surfaces as a `DomainError`.
- **Service unit tests** (with a stub `TagConsolidator`):
  - Identity for omitted tags.
  - Identity for empty-list values.
  - Per-bookmark union + dedupe across multiple input tags.
  - Lowercase normalization of mapping outputs.
  - "No API key" path returns the recognizable error.
- **Repository / integration test** (real Postgres):
  - Seed a user with messy tags across multiple bookmarks, apply a hand-written mapping, assert the resulting tag arrays match expectation, in one transaction.
- **No new E2E** — the feature is a single sync settings-page button; existing manual testing pattern is sufficient.

## Build Sequence (suggested)

1. Domain port + types.
2. Adapter implementing the port (extend the existing Anthropic adapter).
3. Repository methods (`tag_samples`, `apply_tag_mapping`) with integration tests.
4. Service with unit tests.
5. Web endpoint + HTMX wiring.
6. Settings-page UI section with disabled-state logic.
7. Manual end-to-end smoke test against a seeded local user.

## Open Questions

None at design time. Any new ones surface as implementation notes in the plan.
