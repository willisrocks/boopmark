# Better Tag Management — Design

**Status:** Draft (exploration spec; nothing here is implemented except the lab prototype)
**Date:** 2026-10-31
**Branch:** `spec-tag-management`
**Supersedes in part:** `2026-04-25-tag-consolidation-design.md` (the one-click consolidation)

## Summary

Boopmark's tag library degrades as it grows. Tags are stored verbatim, so every save can add a new
spelling of an existing tag (`GitHub`/`github`, `coding-agent`/`coding agents`). The filter bar then
renders every distinct spelling as one flat wall of chips. On a seeded library of 600 bookmarks the
wall fills the whole first screen and no bookmark is visible
([before](assets/2026-10-31-tag-management/before-filter-bar.png)).

This spec fixes the problem in three layers. Each layer is useful without the next:

1. **Deterministic normalization and an alias table.** No LLM. Every tag written by any client
   (web, API, CLI, extension, iOS, import) resolves to one canonical tag. Case, hyphen, spacing,
   slash/dot and plural variants collapse. On the lab library this takes **600 → 303 tags with 0
   wrong merges**. Aliases are remembered, so a duplicate does not come back on the next save.
2. **Reviewed LLM consolidation** for what rules cannot catch: synonyms, abbreviations and typos
   (`js`→`javascript`, `k8s`→`kubernetes`, `wfh`→`remote work`). The model *proposes* merge
   groups through structured output. The user reviews them on a review screen, applies them, and
   can undo the batch. ⟨TBD consolidation numbers⟩
3. **A compact filter bar and a tag library page**: top tags by use, a searchable "More tags"
   popover that also matches aliases, multi-select, and a management page for rename, merge, delete
   and undo.

Save-time suggestions change to choose from the canonical list (an enum in the output schema), allow
at most one new tag, and resolve every suggestion through the alias table before storing it.
⟨TBD suggestion numbers⟩

**Recommendation:** Proposal A. Normalization and aliases, then one structured-output call to the
user's own configured model with a review screen. Decision APIs (OpenAI Decisions, TypeSafe Jev) are
real and cheap, but they cannot propose canonical names or new tags. They are an optional second
pass that verifies proposed merges with calibrated probabilities (Proposal B). They are not the
core. See [Model and API choice](#model-and-api-choice).

## Why the existing consolidation is not enough

`server/src/app/tag_consolidation.rs` and the two consolidator adapters implement the April spec:
one button in Settings → Tag Library, one LLM call, applied immediately. Reading the code shows these
gaps:

| # | Gap | Where | Effect |
|---|-----|-------|--------|
| 1 | No preview, review or undo (explicit non-goals in the April spec) | `TagConsolidationService::consolidate` writes with `update_tags_bulk` straight after the LLM returns | One bad mapping silently rewrites every affected bookmark, and there is no way back |
| 2 | Duplicates come back | Enrichment passes every raw tag (`tags_with_counts`, unbounded) to the model and stores whatever it returns verbatim. There is no alias memory. | The next save can re-create `GitHub` right after a cleanup merged it into `github` |
| 3 | Every output tag is forced to lowercase | `compute_new_tags`, `validate_tag_text` | `GitHub`, `macOS`, `iOS` and `UI/UX` lose their casing |
| 4 | It may add broader parent tags (`react` → `react`, `frontend`) | prompt rule 2 | The tag count can *grow*, and bookmarks get tags the user never chose |
| 5 | Only the 500 most-used tags are sent | `input.tags.iter().take(500)` in both adapters | The long tail, where typos and one-off spellings live, is never considered |
| 6 | The output must contain an entry for every tag | prompt rule 5 | Output grows with the library (~10 tokens per tag, so 6–8k for 600 tags), against a 60 s HTTP timeout (`DEFAULT_TIMEOUT_SECS`, Anthropic client) inside a synchronous HTMX request |
| 7 | Anthropic output is parsed from free text | `extract_json_object` takes first `{` to last `}` | One malformed token fails the whole run. The OpenAI path uses JSON-object mode, not a strict schema. |
| 8 | No deterministic pre-pass | — | Most merges are trivial case/hyphen/plural folds (297 of the 388 needed in the lab). The model spends tokens on them and can still miss some. |
| 9 | The filter bar shows every distinct tag, single-select | `all_tags` → `filters_inner.html` | The wall of chips. Clicking a chip replaces the filter instead of adding to it. |

The consolidator's validation (`validate_consolidation_mapping`) is good and stays: keys must be
submitted tags, values are bounded, and control characters are rejected.

## Goals

- A clean library stays clean: no new duplicate spellings from any client.
- One-time cleanup of an existing messy library, with review and undo.
- A filter bar that fits in one or two rows at any library size.
- Better save-time suggestions that reuse canonical tags.
- Keep the bring-your-own-key model. No new required vendor.

## Non-goals

- Tag hierarchies or parent tags. They are dropped from consolidation (gap 4) and could be a later
  feature.
- Shared tags across users. Everything stays per-user.
- Replacing `bookmarks.tags TEXT[]` with a join table (see [Data model](#canonical-tag-model)).
- Automatic, unreviewed LLM rewrites.

## Lab: seed data, prototype, measurements

Everything here is on this branch under `scripts/tag-lab/` and is reproducible.

- `concepts.py`: the ground truth. 212 concepts, each listing the raw spellings a messy library
  accumulates (600 spellings), plus 53 concept pairs where merging is a judgment call (`UI/UX` vs
  `ux`, `vim` vs `neovim`, `local ai` vs `local llm`). It deliberately includes traps:
  - plural lookalikes: `ios`, `news`, `physics`, `kubernetes`, `aws`, `analytics`
  - near misses: `java`/`javascript`, `go`/`google`, `react`/`react native`, `claude`/`claude code`,
    `management`/`engineering management`, `ai agents`/`coding agents`
  - slash tags: `UI/UX`, `CI/CD`, `GNU/Linux`
  - typos: `kubernets`, `javscript`, `pyhton`
- `seed.py`: writes `seed.sql`, 600 bookmarks for one user in 13 topic groups, with Zipf-weighted
  spellings and every spelling used at least once. It also writes `ground_truth.json`.
- `server/src/domain/tag_normalize.rs` (with unit tests) and `server/examples/tag_lab.rs`: the
  deterministic normalizer.
- `consolidate.py`: the proposed LLM step. Providers are local llama.cpp, OpenAI Responses with a
  strict `json_schema`, and Anthropic Messages with `output_config.format`.
- `suggest.py`: save-time suggestions, the current prompt against the proposed one.
- `measure.py`: scores a grouping against the ground truth.

Run: Postgres 16 and the server ran locally from a nix shell on ports 56440 and 4210 (the host has no
Docker). The lab user was seeded with `psql -f scripts/tag-lab/seed.sql`. Tags and counts were read
back from the database.

**Metric definitions.** A *merge* folds one raw tag into its group's canonical tag. It is *correct*
when both tags belong to the same concept, *ambiguous* when the concept pair is a listed judgment
call, and *wrong* otherwise. Precision = correct / (correct + wrong); ambiguous merges are reported
separately. Recall = correct merges / 388, the number of merges a perfect grouping makes (600 raw
tags − 212 concepts).

**Caveat.** I wrote both the seed and the normalizer, so the deterministic precision is optimistic by
construction. The traps exist to counter that, but a real library will have surprises. That is why
the alias table records every merge and the library page can split an alias back out.

### Results

⟨TBD results table⟩

## Design

### 1. Deterministic normalization

Three cumulative levels, all in `tag_normalize.rs`:

| Level | Rule | Lab: tags after | Wrong merges |
|-------|------|-----------------|--------------|
| raw | — | 600 | — |
| L1 case | lowercase, trim, collapse `-`/`_`/whitespace to one space, no spaces around `/` | 377 | 0 |
| L2 compact | L1, then drop spaces, `/` and `.` (`self hosted`=`selfhosted`, `next.js`=`nextjs`, `CI/CD`=`cicd`) | 341 | 0 |
| L3 plural | the last word's plural (`-s`, `-es`, `-ies`) folds into its singular **only if that singular already exists in the library**. Words of three letters or fewer and `-ss` words never fold. | 303 | 0 |

The existence check is what makes plural folding safe. `kubernetes`, `news`, `physics`, `ios` and
`analytics` never fold, because `kubernete`, `new`, `physic`, `io` and `analytic` are not tags. A
stemming dictionary is not needed.

Slash tags are not split. `UI/UX` is one tag and only folds with `ui-ux`/`ui ux`/`ui/ux`. Whether it
equals `ux` is a judgment call for the review step.

**Canonical display form.** The group's most-used spelling is the default canonical tag, with case
preserved, and the user can rename it. The new pipeline never forces lowercase.

### 2. Canonical tag model

**Recommended (M1): keep `bookmarks.tags TEXT[]` storing canonical names, and add an alias table
plus a change log.**

```sql
-- One row per known spelling, keyed by its compact key.
CREATE TABLE tag_aliases (
    user_id   UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    alias_key TEXT NOT NULL,          -- compact_key(spelling)
    alias     TEXT NOT NULL,          -- a spelling seen, for display ("also: GitHub")
    canonical TEXT NOT NULL,          -- the tag stored on bookmarks
    source    TEXT NOT NULL,          -- 'normalize' | 'llm' | 'manual'
    batch_id  UUID REFERENCES tag_change_batches(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, alias_key)
);

-- One row per rename/merge/delete/cleanup the user applies; the unit of undo.
CREATE TABLE tag_change_batches (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL,         -- 'normalize' | 'llm_merge' | 'rename' | 'merge' | 'delete'
    summary    JSONB NOT NULL,        -- the merges applied, for the history list
    undone_at  TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Before-images of the bookmarks a batch touched.
CREATE TABLE tag_change_snapshots (
    batch_id    UUID NOT NULL REFERENCES tag_change_batches(id) ON DELETE CASCADE,
    bookmark_id UUID NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE,
    tags_before TEXT[] NOT NULL,
    tags_after  TEXT[] NOT NULL,
    PRIMARY KEY (batch_id, bookmark_id)
);
```

**Write path.** One function, `TagResolver::resolve(user_id, raw) -> String`, runs on every tag
write: bookmark create and update, import, and the API used by the CLI, extension and iOS app.

1. Trim, then reject empty or control-character tags (as today).
2. Look up `compact_key(raw)` in `tag_aliases`. If found, return its `canonical`.
3. Otherwise try the L3 plural candidates against `tag_aliases`. If found, return that canonical and
   insert the new spelling as an alias.
4. Otherwise it is a new tag. Store it as typed and insert `alias_key → itself`.

Bookmarks keep exact canonical strings, so filtering (`tags && $1`), export and the API stay
unchanged. Clients never need to know aliases exist.

**Merge semantics.** Merging A into B:

- rewrites every bookmark array, replacing A with B and deduplicating
- repoints every alias whose canonical is A to B
- adds `key(A) → B`
- records the batch and snapshots

Rename is a merge into a new name. Delete removes the tag from bookmarks and drops its aliases. All
three run in one transaction.

**Undo.** Undo is per batch, from the library page's history list (kept 30 days).

- For each snapshot whose bookmark still has `tags_after`, restore `tags_before`.
- If a bookmark changed since, apply the inverse as a diff instead: re-add the removed tags and
  remove the added canonical, only if it was not on the bookmark before.
- Delete the batch's aliases, then mark the batch `undone_at`.

Undo is not offered for a batch that a later, still-applied batch depends on. The UI greys it out
with "undo the newer change first".

**Splitting an alias.** On the library page, any alias can be "split out" into its own tag. This is
the escape hatch for a wrong deterministic fold.

**Rejected alternative (M2): a `tags` table plus a `bookmark_tags` join table.** It makes renames a
one-row update and counts cheap, but it touches every bookmark query, import/export, the API and the
filter SQL. Libraries here have hundreds of tags and thousands of bookmarks, so array rewrites in one
transaction are milliseconds. M1 gets the same behaviour with additive migrations.

**Migration.** Migration `015_tag_aliases.sql` creates the tables. A one-time job runs L1–L3 per user
as a `normalize` batch: it rewrites tags, seeds `tag_aliases`, and can be undone like any other batch.
It runs at deploy for every user, or lazily on first visit to the library page. Before it touches
data, the library page shows a banner: "We merged 297 spellings into 303 tags · Review · Undo".

### 3. Reviewed LLM consolidation

Triggered from the library page ("Find duplicates with AI"). It runs as a background job, not inside
the HTMX request. The page polls the job, so the 60 s client timeout no longer matters.

**Input.** The canonical tags after normalization (not raw spellings), each with its bookmark count
and one or two sample titles, delimited as untrusted data, as today. With 303 tags this is
⟨TBD tokens⟩.

**Output, strict schema.** The model returns only groups to merge, not an entry per tag, so output
scales with the number of duplicates instead of the library size:

```json
{"merges": [
  {"canonical": "javascript", "members": ["javascript", "js", "java-script", "javscript"], "confidence": "high"}
]}
```

The schema is the same for all three providers: OpenAI Responses `text.format` `json_schema`
`strict: true`; Anthropic `output_config.format` (GA, no beta header, supported on every Claude model
Boopmark offers); llama.cpp `response_format` for local runs.

**Prompt** (`scripts/tag-lab/consolidate.py`, `INSTRUCTIONS`):

- Merge synonyms, abbreviations, alternate spellings, typos and a product's other names.
- Do not merge broader/narrower pairs or related-but-distinct topics.
- Use only listed tags. The canonical tag is a member. Each tag is in at most one group.
- Confidence is `high`, `medium` or `low`.

The examples in the prompt (`tf`/`terraform`, `aws lambda`/`serverless`) are deliberately outside
the lab's concept list, so they do not leak the answers.

**Validation**, applied before anything is shown:

- Drop unknown tags and tags already claimed by an earlier group.
- Drop groups whose canonical is not a member, or that have fewer than two members.
- Keep the existing length and control-character checks.

Invalid entries are dropped, never guessed.

**Review screen** ([mockup](assets/2026-10-31-tag-management/mockup-review.png)):

- One row per proposed group: member chips with counts, the canonical tag (editable), confidence,
  bookmarks affected, and a sample title.
- `high` rows are pre-checked; `medium` and `low` rows are unchecked under "Check these".
- "Apply N merges" creates one `llm_merge` batch, which can be undone as a unit.

### 4. UI

**Filter bar** ([mockup](assets/2026-10-31-tag-management/mockup-filter.png)):

- Active tags first, as removable chips. Multi-select: chips *add* to the `tags` filter, and the
  repository already matches with `&&`.
- Then the top N canonical tags by bookmark count, with counts. N defaults to 10 on desktop and 4 on
  mobile, and the bar is one row with no wrap.
- A "More tags (241)" button opens a popover with a search box. It matches canonical names *and*
  aliases (typing `k8s` finds `kubernetes`), is keyboard navigable, and links to an A–Z browse list.
- Server-side, `all_tags` is replaced by `top_tags(user, n)` plus `GET /tags/search?q=`, which
  returns an HTMX fragment. The popover loads lazily.

**Tag library page** ([mockup](assets/2026-10-31-tag-management/mockup-library.png)). This replaces
Settings → Tag Library and moves to `/tags`, linked from the filter popover and from Settings:

- table of canonical tags: aliases, count, and Rename · Merge into… · Delete
- filter box, and sort by most used, A–Z or unused
- bulk select → "Merge selected into…"
- the "Find duplicates with AI" button (enabled when a provider key is configured)
- the batch history with Undo

### 5. Save-time suggestions

Current behaviour (`AnthropicEnricher::build_prompt`, `OpenAiEnricher`): the prompt lists every raw
tag with its count, the reply is free-form JSON, and tags are stored as returned.

Proposed:

1. Send canonical tags only, most used first (303 → about 2k tokens in the lab, shrinking after
   cleanup). Cap the list at the 300 most used. Above that, add a lexical prefilter on the URL,
   title and description.
2. Use a strict schema: `{"existing": [enum of canonical tags], "new": [string, maxItems 1]}`.
   OpenAI's limits allow 1000 enum values (15,000 characters if more than 250). Anthropic documents
   no enum count limit but warns that enum casing is not guaranteed, so resolution compares
   case-insensitively.
3. Prompt rules: 2–5 tags, prefer the most specific existing tag, never a tag together with its
   synonym, at most one new tag and only for a central uncovered topic, short lowercase noun phrase.
4. Every suggested tag goes through `TagResolver::resolve` before it is shown, so even a new tag
   that is a spelling of an existing one maps to the canonical tag.

⟨TBD suggestion results⟩

## Model and API choice

### What the "decision APIs" actually are (verified from primary sources, 2026-10-31)

**OpenAI Decisions API: it exists.** Source:
[developers.openai.com/api/docs/guides/decisions](https://developers.openai.com/api/docs/guides/decisions)
and the [API reference](https://developers.openai.com/api/reference/resources/decisions/methods/create).

- Endpoint `POST /v1/decisions`. Model `gpt-6-luna`, "the only currently supported model".
- Request: `model`, `input` (text, or an array of messages and tool calls), and `questions` (1–400
  per request).
- Question types:
  - `predicate`: returns `probability`
  - `choice`: one of your values, each with an optional description; returns `probabilities` and
    `confidence`
  - `score`: 2–10 levels
- Answers are typed. It does not generate text.
- The page opens "The Decisions API is now generally available". It claims to be "about 10x faster
  than the Responses API".
- Pricing on the guide: uncached input $0.10 per 1M tokens, output tokens not charged.
- The guide states no maximum number of choices.
- Several secondary articles from October describe it as a limited preview, and one reports a 403
  "not enabled" error on a standard key. The primary doc now says GA. Whether Chris's key has
  access is unverified because no key was available to this run.

**TypeSafe Jev: it exists.** Sources: [docs.typesafe.ai](https://docs.typesafe.ai/),
[models](https://docs.typesafe.ai/go/models.md), [Choice](https://docs.typesafe.ai/primitives/choice.md),
and the official agent skill
[typesafe-ai/skills SKILL.md](https://github.com/typesafe-ai/skills/blob/main/skills/typesafe-ai/SKILL.md).

- A "System One" decision model. Endpoint `POST https://api.typesafe.ai/v1/systemone`, with a
  `state` (text or JSON) and a map of typed questions.
- Question types:
  - `Choice`: up to 255 options, each described; returns `probabilities` and `confidence`
  - `Score`
  - `Noul`: the probability that a statement is true
- Questions are evaluated in parallel and independently over the same state. It never generates
  text.
- Models `jev-1.14.0` and `jev-1.13.0` (`jev-latest`). Price $0.042 per 1M input tokens, output free.
  Context 64k per request and 32k per question.
- It needs a separate TypeSafe account and key from console.typesafe.ai.
- No `jev` skill is installed on this host. I evaluated Jev from the vendor docs and the official
  skill text instead. The skill's own guidance ("select instead of generate", ask about the null
  case separately, keep options distinct) shaped the Proposal B design below.

**What both share, and why it matters here.** Both are *classifiers with calibrated probabilities*.
They answer "is A the same tag as B?" or "which of these tags fits?" very cheaply. Neither can
*propose* a canonical name, invent a needed new tag, or look at 300 tags and find the groups. Each
question is evaluated in isolation, so the model never sees the whole vocabulary at once, and the
whole vocabulary is exactly what consolidation needs.

### Proposals

**A. Deterministic + structured-output LLM + review (recommended).**

- Consolidation is one call to the user's configured model, with the strict schema above.
  Suggestions use the enum schema.
- It uses the keys and settings users already have (Anthropic or OpenAI) and adds no vendor.
- The review screen is the safety net. Measured quality and cost: ⟨TBD⟩.

**B. A + decision-API verifier.** After A proposes groups, send each proposed (member, canonical)
pair as one predicate/Noul question: "A bookmark tagged '{member}' is about the same topic as one
tagged '{canonical}'".

- All pairs go in one request: OpenAI allows 400 questions per request; Jev has no question limit
  within its context.
- The calibrated probability drives the review defaults: ≥ 0.9 pre-checked, below that under
  "Check these". The model's self-reported confidence is not used for this.
- Cost is negligible: about 100 pairs × ~40 tokens = 4k tokens → $0.0004 on OpenAI Decisions,
  $0.0002 on Jev.
- It is worth adding only if the review load from A is too high, that is, if users have to inspect
  many unchecked rows. Jev needs a second key; Decisions needs OpenAI access.

**C. Decision API as the main engine.** For suggestions, one Noul/predicate per canonical tag (300
questions, about 6–7k tokens; $0.0003 on Jev, $0.0007 on Decisions) and take the top 2–5 above a
threshold. For consolidation, per-tag `Choice` among candidate canonicals.

- It cannot create a needed new tag, so suggestions still need a generative fallback.
- Consolidation needs a candidate shortlist per tag (Jev's 255-option cap is below a 300-tag
  library), so it needs candidate generation on top.
- It is rejected as the core: more moving parts, a new vendor, and the expensive part (seeing the
  whole vocabulary) is the part it cannot do.

**D. Deterministic only, no LLM.** Layer 1 plus the library page's manual merge.

- It gets 600 → 303 with zero cost and is the baseline everyone gets, including users without a
  key.
- It leaves the synonym and abbreviation tail (91 merges in the lab) to manual work.

### Model choice

⟨TBD model comparison and cost table⟩

## Testing

- Unit: `tag_normalize` (shipped on this branch: 7 tests). Also resolver lookup order,
  merge/rename/delete/undo on a stub repository, and consolidation validation for unknown tags,
  claimed tags, a non-member canonical, and singleton groups.
- Repository: alias upsert and lookup, batch apply and undo in one transaction, and undo with
  intervening edits (Postgres integration tests like the existing ones).
- E2E (Playwright, the committed harness): seed a messy library, check the filter bar shows at most
  N chips plus "More", search `k8s` finds `kubernetes`, a merge on the library page can be undone,
  and the review screen applies only checked rows.
- Lab regression: `scripts/tag-lab` can be rerun when prompts change. Zero wrong deterministic
  merges is a hard gate; LLM precision is tracked.

## Rollout

1. Normalizer, resolver, alias tables and migration, write-path resolution. No UI change except the
   banner. Biggest win, no LLM.
2. Filter bar (top N + More + multi-select) and the `/tags` library page with manual merge and undo.
3. Suggestion prompt and schema change.
4. LLM consolidation as a background job with the review screen. Remove the old one-click button.
5. Optional: Proposal B verifier, if review load warrants it.

## Spend log

⟨TBD spend⟩
