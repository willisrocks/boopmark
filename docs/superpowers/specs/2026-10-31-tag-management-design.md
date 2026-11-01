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
   (`js`→`javascript`, `k8s`→`kubernetes`, `wfh`→`remote work`). Candidate merges come from two
   sources: a whole-list structured-output call, and cheap string candidates. Every candidate pair
   is then *checked one pair at a time*, the user reviews the result, applies it, and can undo the
   batch. In the lab, a 4B local model given the whole list proposed topic clusters, not synonyms
   (**6% precision**). The same model checking one pair at a time had **100% precision** (27/27,
   all hand-checked) at 26% recall. The fully local pipeline reached **600 → 303 → 276 tags with 0
   wrong merges**. A hosted mid-tier model is needed for recall; that comparison is pending API
   keys (see [Open items](#open-items)).
3. **A compact filter bar and a tag library page**: top tags by use, a searchable "More tags"
   popover that also matches aliases, multi-select, and a management page for rename, merge, delete
   and undo.

Save-time suggestions change to choose from the canonical list (an enum in the output schema), allow
at most one new tag, and resolve every suggestion through the alias table before storing it.
Measured on 20 seeded bookmarks with the local model:

- The current pipeline stored a **new distinct tag for 16% of suggested tags** (3% a new spelling
  of an existing tag, 13% a brand-new tag). The proposed pipeline stored **0%**, reusing an
  existing canonical tag every time.
- On-topic rates were similar (38% vs 35% against the synthetic labels).
- The prompt shrank from 3,050 to 1,980 input tokens per save.

**Recommendation: Proposal A.**

- Normalization and aliases first.
- Then consolidation with the user's own configured provider:
  - propose: a whole-list structured-output call plus string candidates
  - check: every pair, in one batched structured-output call
  - review: a review screen, with undo
- Use a mid-tier model for consolidation (Sonnet 5 / gpt-5.6-terra, about $0.09–0.10 per run on
  paper). Keep the user's default model for suggestions.

Decision APIs (OpenAI Decisions, TypeSafe Jev) are real and very cheap. They do exactly one part of
this well, the pairwise check, and they cannot propose canonical names or new tags. They are the
optional Proposal B upgrade for that one step. See [Model and API choice](#model-and-api-choice).

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
| 6 | The output must contain an entry for every tag | prompt rule 5 | Output grows with the library (~10 tokens per tag, so 6–8k for 600 tags), against a 60 s HTTP client timeout (both consolidator clients) inside a synchronous HTMX request |
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

## Brainstorming decisions (auto-pilot)

The superpowers brainstorming skill is not installed on this host. I ran its question loop myself
and answered each question from code or measurements; there was no human gate.

| Question | Answer | Evidence |
|----------|--------|----------|
| Is the problem storage or display? | Both. Verbatim storage creates the duplicates; the flat chip list amplifies them. | `all_tags` → `filters_inner.html`; the current suggestion pipeline created a new tag for 16% of suggested tags |
| Normalize on read or on write? | On write, through an alias table, plus a one-time migration. | Read-time folding would leave the API, export and CLI with dirty data |
| Aliases or a tag table? | Aliases next to `TEXT[]` (M1). | Every query and client stays unchanged; libraries are small |
| Is plural folding safe? | Only when the singular already exists. | 0 wrong among 297 deterministic merges, including the `ios`/`news`/`kubernetes` traps |
| Can an LLM apply merges unreviewed? | No. | 165 wrong merges when the local model's proposals were applied directly |
| Self-reported confidence for review defaults? | No; use a pairwise check. | 41 of 45 `high`-confidence proposals were wrong; the pairwise check had 0 wrong |
| Decision API at the core? | No; it is an optional pair-check backend. | Neither can propose names or new tags (primary docs); it is the one step it does well |
| Which model? | Mid tier for consolidation, the default for suggestions. | Cents per run (paper costs); small models fail at the whole-list task |

## Lab: seed data, prototype, measurements

Everything here is on this branch under `scripts/tag-lab/` and is reproducible.

- `concepts.py`: the ground truth. 212 concepts, each listing the raw spellings a messy library
  accumulates (600 spellings), plus 52 concept pairs where merging is a judgment call (`UI/UX` vs
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
- `verify.py`: the pairwise check. One yes/no question per pair; P(yes) is read from the answer
  token's log-probabilities, mimicking a decision-API predicate. It also generates the string
  candidates and unions accepted pairs into groups.
- `suggest.py`: save-time suggestions, the current prompt against the proposed one.
- `measure.py`: scores a grouping against the ground truth, overall and for the LLM's proposals
  alone.
- `out/`: every input and output of the runs, including the two hand checks
  (`handcheck-deterministic.txt`, `handcheck-llm.txt`) and `spend.jsonl`.

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

| Stage | Tags | Merges | Wrong | Precision | Recall (of 388) |
|-------|------|--------|-------|-----------|-----------------|
| Raw | **600** | — | — | — | — |
| L1 case/separator | 377 | 223 | 0 | 1.00 | 0.57 |
| L2 compact (`/`, `.`, joined words) | 341 | 259 | 0 | 1.00 | 0.67 |
| L3 plural, existence-checked | **303** | 297 | 0 | 1.00 | 0.77 |
| L3 + Qwen3-4B whole-list proposals, applied unreviewed | 219 | 381 | **165** | 0.56 | 0.54 |
| L3 + string candidates + Qwen3-4B pairwise check (p ≥ 0.5) | **276** | 324 | **0** | 1.00 | 0.83 |
| Perfect grouping (ground truth) | 212 | 388 | 0 | 1.00 | 1.00 |

**LLM step on its own** (proposals beyond L3, scored per folded tag):

| LLM step | Proposals | Correct | Ambiguous | Wrong | Precision |
|----------|-----------|---------|-----------|-------|-----------|
| Qwen3-4B, whole list in one call (consolidate.py) | 84 | 5 | 3 | 76 | **0.06** |
| … same proposals, then Qwen3-4B pairwise check, p ≥ 0.5 | 1 | 1 | 0 | 0 | 1.00 |
| String candidates (287 pairs; 72 of the 102 residual same-concept pairs) + Qwen3-4B pairwise check, p ≥ 0.5 | 28 | 27 | 1 | 0 | **1.00** (recall 0.26 of residual) |
| … at p ≥ 0.9 | 20 | 20 | 0 | 0 | 1.00 (recall 0.20) |

What the numbers say:

- **Deterministic rules get the bulk safely.** They make 297 of the 388 needed merges (77%) with
  zero errors, and a hand check of 40 random merges found 40/40 correct
  (`scripts/tag-lab/out/handcheck-deterministic.txt`).
- **A small model must not see the whole list and decide alone.** Qwen3-4B (Q4_K_M, llama.cpp,
  strict JSON schema) grouped by *topic*: `android`, `swift` and `macos` into `ios`; `figma` and
  `ux` into `react`; `claude` and `machine learning` into `python`. It labelled 45 of those
  proposals `high` confidence, and 41 of the 45 were wrong. Self-reported confidence is no use for
  review defaults.
- **Checking one pair at a time is precise even on a small model.** The same model, asked "do A
  and B mean the same thing?", accepted 28 pairs. All 27 non-ambiguous ones were correct (hand
  check: `out/handcheck-llm.txt`). It rejected 83 of its own 84 whole-list proposals.
- **Recall is limited by how strong the model is, not by the pipeline.** The small model missed
  `ts`/`typescript`, `ML`/`machine learning`, `o11y`/`observability` and `rust`/`rust-lang`. A
  hosted mid-tier model is expected to catch these, but that is not measured yet (pending keys).
  String candidates alone also cap recall at 72 of the 102 residual pairs: pure synonyms such as
  `wfh`/`remote work` and `infosec`/`security` need the whole-list call.
- **The review screen is still required.** It catches judgment calls (`UI/UX` ~ `ux` scored 0.68)
  and whatever the checker gets wrong.

Local run cost: $0. On this 4-core EPYC (AVX2, native llama.cpp build) the whole-list call took
573 s for 6,440 input + 1,791 output tokens. The pairwise check averaged ~2.2 s and ~120 input
tokens per pair. The nixpkgs llama.cpp binary has no AVX2 kernels and ran ~10× slower.

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

**Canonical display form.** Case is preserved and the user can rename the canonical tag. The new
pipeline never forces lowercase. The lab's naive rule, the most-used spelling, picked odd display
forms for some groups (`Coding Agents`, `Startup`, `AI-art`). The default is therefore chosen in
this order:

1. the most-used spelling written with spaces rather than hyphens or underscores
2. among those, the all-lowercase spelling, unless every use is cased (acronyms and brands such as
   `AWS`, `GitHub` and `macOS` keep their case when that is how they are mostly written)
3. on a tie, the shortest spelling

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
data, the library page shows a banner: "We merged 600 spellings into 303 tags · Review · Undo".

### 3. Reviewed LLM consolidation

Triggered from the library page ("Find duplicates with AI"). It runs as a background job, not inside
the HTMX request. The page polls the job, so the 60 s client timeout no longer matters.

**Input.** The canonical tags after normalization (not raw spellings), each with its bookmark count
and one sample title, delimited as untrusted data, as today. With 303 tags this measured 6,440
input tokens (Qwen tokenizer; expect ±20% on Claude/OpenAI). The current consolidation sends up to
500 raw tags with three titles each, about 22k input tokens.

**Two proposal sources, merged:**

1. *Whole-list call*: the strict schema below. It catches pure synonyms that share no letters
   (`wfh`/`remote work`).
2. *String candidates* (`verify.py candidate_reason`): abbreviation subsequence (`js`/`javascript`),
   initials (`mcp`), numeronyms (`k8s`, `a11y`), substrings (`web dev`/`web development`),
   edit distance ≤ 2 (`kubernets`) and shared words. They are free and deterministic. In the lab
   they found 72 of the 102 residual same-concept pairs in 287 candidate pairs.

**Pairwise check (new).** Every candidate pair from either source goes into one batched call to the
same provider: "do A and B mean the same thing?". The schema is
`{"pairs":[{"id":int,"same":bool,"confidence":"high|medium|low"}]}`. For 300 pairs that is about
9k input + 3k output tokens. A structured boolean is used instead of the log-probabilities the
lab read, because Anthropic's Messages API does not return token log-probabilities. Pairs judged
`same` become merge groups (union-find), and the canonical tag follows the display-form rule
above. Only pairs judged `same` with `high` confidence are
pre-checked on the review screen. This step is what turns a 6%-precision proposal list into a
usable one in the lab.

**Whole-list call output, strict schema.** The model returns only groups to merge, not an entry per tag, so output
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

**Measured** (`suggest.py`, Qwen3-4B local, 20 seeded bookmarks sampled with a fixed seed; full
rows in `out/suggest-qwen3-4b.json`):

| Pipeline | Tags stored | Reused an existing tag exactly | New spelling of an existing tag (duplicate) | Brand-new tag | On-topic vs labels | Input tokens / save | Output tokens / save |
|----------|-------------|-------------------------------|---------------------------------------------|---------------|--------------------|---------------------|----------------------|
| Current prompt, stored verbatim | 100 | 84% | 3% | 13% | 33/87 (38%) | 3,050 | 59 |
| Proposed (canonical enum + resolver) | 77 | **100%** | **0%** | **0%** | 27/77 (35%) | 1,980 | 27 |

Reading the numbers:

- **The current pipeline grows the library on almost every save.** 16 of 100 suggested tags would
  have become new chips: `database` next to `databases`, `tutorial` next to `tutorials`, and new
  tags such as `research`, `data` and `configuration management`.
- **The proposed pipeline cannot grow the library by accident.**
- *On-topic* is the share of judged tags whose concept is one of the bookmark's labeled concepts.
  The seeded titles name only one or two concepts, so the low absolute numbers say more about the
  seed than the prompt. The two pipelines are within noise of each other.
- **Two weaknesses of the small model** under the proposed schema:
  - It never used `new`. Going from 13% new tags to 0% may over-suppress a legitimately new topic.
  - It sometimes padded with an off-topic popular tag (`mobile` on unrelated pages).

  Both should be rechecked on a hosted model. If padding persists, change "2 to 5" to "1 to 5",
  and add a server-side cap that drops any existing tag the model marks `low`.
- **Synonyms leak until consolidation runs.** One answer stored `terminal`, `cli` and
  `command line` together, because `cli` and `command line` are separate canonical tags until
  consolidation merges them. Suggestions improve after consolidation.

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

**A. Deterministic + structured-output LLM with a pairwise check + review (recommended).**

- Consolidation makes two calls to the user's configured provider: whole-list proposals, then a
  batched pairwise check. Both use strict schemas, and string candidates are added for free.
  Suggestions use the enum schema.
- It uses the keys and settings users already have (Anthropic or OpenAI) and adds no vendor.
- Measured on the local 4B model: precision 1.00 after the pairwise check. Recall needs a
  stronger model.
- Paper cost per consolidation run: about $0.09 (Sonnet 5), $0.10 (gpt-5.6-terra) or $0.04
  (Haiku 4.5). See the table below.

**B. A, with the pairwise check done by a decision API.** Instead of A's batched generative check,
send each candidate pair as one predicate/Noul question: "A bookmark tagged '{a}' is about the same
topic as one tagged '{b}'". This is the step the lab emulated with log-probabilities.

- All pairs go in one request: OpenAI allows 400 questions per request; Jev has no question limit
  within its context.
- The calibrated probability drives the review defaults: ≥ 0.9 pre-checked, below that under
  "Check these". The model's self-reported confidence is not used for this.
- Cost is negligible: about 300 pairs × ~120 tokens (measured locally, with sample titles) = 36k
  tokens → $0.004 on OpenAI Decisions, $0.0015 on Jev.
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

**Measured:** only the local Qwen3-4B (no hosted keys were available; see [Open items](#open-items)).
It shows the failure mode of a small model on the whole-list task (6% precision) and the strength of
the pairwise check (100%).

**Recommended defaults:**

- *Consolidation* is a rare, whole-vocabulary reasoning task, and quality matters more than cost.
  Use the provider the user selected, at least at the mid tier: `claude-sonnet-5` or
  `gpt-5.6-terra`, even if the user's default is Haiku or luna. Show the estimated cost on the
  button ("~$0.09").
- *Suggestions* run on every save, and the enum schema constrains the output, so the user's
  default model (Haiku 4.5 / gpt-5.6-luna) is enough.
- *Model options.* The settings list lacks Claude Haiku 5.5 and Sonnet 5.5, which are cheaper or
  better than the listed Haiku 4.5 and Sonnet 5. Adding them is a separate small change.

**Paper cost estimates** (list prices on the providers' pricing pages, 2026-10-31; token counts from
the lab, rounded):

| Model | $/M in, out | Consolidation, proposed (whole-list 6.5k in / 2.5k out + pair check 9k in / 3k out) | Consolidation, current (~22k in / ~5.5k out) | Per save, proposed (measured 2.0k in / 27 out) | Per save, current (measured 3.05k in / 59 out) |
|-------|-------------|-----|-----|-----|-----|
| claude-haiku-4-5 (app default) | 1 / 5 | $0.043 | $0.050 | $0.0021 | $0.0033 |
| claude-haiku-5-5 | 0.10 / 0.50 | $0.004 | $0.005 | $0.0002 | $0.0003 |
| claude-sonnet-5 | 2 / 10 | $0.086 | $0.099 | $0.0042 | $0.0067 |
| claude-opus-5-5 | 4 / 20 | $0.17 | $0.20 | $0.0085 | $0.013 |
| gpt-5.6-luna (app default) | 0.20 / 1.20 | $0.010 | $0.011 | $0.0004 | $0.0007 |
| gpt-5.6-terra | 2 / 12 | $0.097 | $0.11 | $0.0043 | $0.0068 |
| gpt-5.6-sol | 4 / 20 | $0.17 | $0.20 | $0.0085 | $0.013 |
| OpenAI Decisions (gpt-6-luna), pair check only (36k in) | 0.10 / free | +$0.004 | — | Proposal C: ~$0.0007 | — |
| TypeSafe Jev, pair check only (36k in) | 0.042 / free | +$0.0015 | — | Proposal C: ~$0.0003 | — |

Every option costs cents per consolidation run and fractions of a cent per save, so cost does not
decide between them; quality does. The proposed consolidation costs about the same as today's
while doing more (a pair check plus review). Suggestions get cheaper because the canonical list is
half the size of the raw list (measured 1,980 vs 3,050 input tokens per save).

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

| Run | Provider / model | Calls | Input tokens | Output tokens | $ |
|-----|------------------|-------|--------------|---------------|---|
| Whole-list consolidation | local llama.cpp, Qwen3-4B-Instruct-2507 Q4_K_M | 1 | 6,440 | 1,791 | 0 |
| Pair check on its 84 proposals | local | 84 | 10,082 | 84 | 0 |
| Pair check on 287 string candidates | local | 287 | 34,200 | 287 | 0 |
| Save-time suggestions, current prompt | local | 20 | 61,000 | 1,180 | 0 |
| Save-time suggestions, proposed prompt | local | 20 | 39,600 | 540 | 0 |
| Hosted models, decision APIs | — | 0 | 0 | 0 | **0** (no keys available; $10 budget unused) |

## Open items

- **Hosted comparison (blocked on keys).** The $10 budget is approved, but no Anthropic, OpenAI or
  TypeSafe key was available to this run. Once keys are in the environment, the same harness
  measures them unchanged:
  `python3 scripts/tag-lab/consolidate.py --provider anthropic --model claude-sonnet-5 --run sonnet5`,
  then `measure.py out/sonnet5.groups.json`, and the same for `claude-haiku-4-5-20251001`,
  `gpt-5.6-luna` and `gpt-5.6-terra`. `verify.py` and `suggest.py` support only llama.cpp so
  far; their hosted variants (a batched structured pair check, and the enum suggestion schema) are
  a small addition. Estimated total for 4 models × (consolidation + pair check + 20 suggestions) is
  under $1.
- **Decision API access.** The OpenAI Decisions guide says GA, but secondary reports from October
  describe per-account enablement. Check with one request before building Proposal B on it.
- **Real library check.** Run the normalizer over Chris's production tag list (read-only
  `tags_with_counts` export) and hand-check its merges before the migration ships.
