# Tag Normalization and Alias Model — Implementation Plan

**Spec:** `docs/superpowers/specs/2026-10-31-tag-management-design.md` (branch `spec-tag-management`),
sections "1. Deterministic normalization", "2. Canonical tag model" and Rollout step 1.
**Branch:** `feat-tag-normalization`
**Goal:** every *new* tag written by any client resolves to an existing canonical tag; an
on-demand job writes merge *proposals*; a separate `apply_merge` applies them as one undoable
batch. Existing tags are never merged automatically.

## Auto-pilot decisions

The superpowers and auto-pilot skills are not installed on this host, so I ran the planning
questions myself and answered each from the spec, the code, or the task brief. No human gate.

| Question | Answer | Evidence |
|----------|--------|----------|
| Does the migration normalize existing tags, as the spec's "Migration" paragraph says? | No. It only creates tables. The L1–L3 job writes **proposals**; nothing is rewritten until `apply_merge`. | Task brief HARD RULE ("existing tags are NEVER merged automatically"); supersedes the spec paragraph |
| How does the write path find canonical tags when `tag_aliases` starts empty? | Index = alias rows ∪ distinct tags already on the user's bookmarks. | No backfill is allowed, so without the second source the first save after deploy would create `github` next to an existing `GitHub` |
| An edit re-submits the bookmark's existing tags. Resolve them? | No. Tags already on the bookmark are kept verbatim; only tags new to that bookmark resolve. | "Existing data is untouched": re-saving a title must not silently rewrite `github` → `GitHub` |
| A typed tag exists verbatim in the library, but an existing different spelling has the same key. | Alias row first, then the exact existing spelling, then the display rule over same-key spellings. | Keeps an existing tag stable; an alias row only exists after a merge or an earlier resolution, so it is the user's latest decision |
| Plural direction on write? | Both: a new plural resolves to an existing singular (spec L3), and a new singular resolves to an existing plural whose singular it is. | Both directions are the same L3 relation `group_tags` merges; without the reverse, `coding-agent` next to an existing `coding agents` makes a new duplicate |
| `.net` vs `net` under L2, which drops `.`? | Keep a `.` that starts a word; drop dots inside a word (`next.js` = `nextjs`, `asp.net` = `aspnet`, `.net` ≠ `net`). | Task: "keep the symbol-aware handling (c#, c++, .net, UI/UX)" |
| `-es` plural for `types`/`notes`? | `-ies`→`y`; `-es` only after `s`, `x`, `z`, `ch`, `sh`; otherwise `-s`. | The prototype tried `-es` first, so `notes` could fold into an existing `not` |
| Display-form rule "unless every use is cased" vs "mostly written"? | Prefer space spellings over `-`/`_`; lowercase wins unless cased spellings have more uses; then most used, shortest, A–Z. | Spec examples: `AWS`, `GitHub`, `macOS` keep their case "when that is how they are mostly written" |
| Control characters on write? | Dropped from the tag list, not a request error. | Today's form and API never reject tags; a 400 on save would be a new failure mode for every client |
| Import "restore" mode? | Left verbatim. Import mode (create and upsert) resolves. | Restore reproduces an export byte for byte (ids, timestamps); it is backup data, not a new tag |
| Undo of repointed aliases? | A `tag_change_alias_snapshots` table stores each alias row's before-image. | The spec's "delete the batch's aliases" alone cannot restore an alias that was repointed from A to B |
| Proposal status after its batch is undone? | Back to `pending`. | Undo restores the state before apply; the review screen can show it again or dismiss it |
| Re-running the proposal job? | Replaces the user's pending proposals of that source; skips groups the user dismissed (same member set). | Idempotent and never resurrects a dismissed proposal |
| Undo dependency? | Refused (`InvalidInput`) when a later, still-applied batch touched the same bookmarks or alias keys. | Spec: "Undo is not offered for a batch that a later, still-applied batch depends on" |
| Port shape? | New `TagAliasRepository` port (trait_variant, like the other repos) and a dyn-safe `TagResolver` port that `BookmarkService` holds as an `Option`, like `ImageGenerator`. | `BookmarkRepository` has three mock impls in tests; a new optional collaborator leaves them untouched and keeps `main.rs` edits additive |

## Steps

Each step ends with `cargo test -p boopmark-server` green and a commit.

1. **Normalizer** — `server/src/domain/tag_normalize.rs` from the prototype, hardened: word-leading
   dot, plural suffix rules, `choose_display`, `clean_tag`. Tests for every spec pattern and the
   screenshot list.
2. **Domain model** — `server/src/domain/tag_alias.rs`: `TagAlias`, `TagMergeProposal`,
   `TagChangeBatch`, enums for source/status/kind/confidence; pure functions `resolve_tag`
   (lookup order), `merge_mapping` (validate merges, follow chains, reject conflicts and cycles),
   `rewrite_tags`, `undo_tags`, `normalization_proposals`. Unit tests.
3. **Migration** — `migrations/015_tag_aliases.sql`: `tag_change_batches`, `tag_aliases`,
   `tag_change_snapshots`, `tag_change_alias_snapshots`, `tag_merge_proposals`. Additive only.
4. **Port + Postgres adapter** — `domain/ports/tag_alias_repo.rs` and
   `adapters/postgres/tag_alias_repo.rs`: alias list/insert-if-absent, tag counts, replace pending
   proposals, list/dismiss proposals, `apply_proposals`, `apply_merges`, `undo_batch`, list batches.
   Merge and undo each run in one transaction with the bookmark rows locked.
5. **Service** — `app/tag_alias.rs`: `TagAliasService` with `resolve_tags`, `propose_normalization`,
   `apply_merge(user, proposal_ids)`, `merge_tags`, `undo_batch`, list/dismiss. Implements the
   `TagResolver` port. Unit tests on a stub repository.
6. **Write path** — `BookmarkService::with_tag_resolver`; resolve in create, create_claimed,
   create_with_ai_image, update (keeping the bookmark's current tags), and import mode. Wire in
   `main.rs` and add `tag_aliases` to `AppState`.
7. **Postgres test** — ignored test in the adapter: seed a messy library, propose → apply → undo,
   assert untouched bookmarks are byte-identical throughout and old names resolve after apply.
   Run against a nix Postgres on a private port.

## API for later tasks

- Filter bar / `/tags` page: `TagAliasService::merge_tags(user, kind, merges)` for manual merge
  and rename, `list_batches` and `undo_batch(user, batch_id)` for history, `list_aliases` for
  "also: GitHub".
- Review screen: `TagAliasRepository::replace_pending_proposals(user, ProposalSource::Llm, ..)`
  to store LLM proposals, `list_proposals`, `apply_merge(user, ids)`, `dismiss_proposals`.
