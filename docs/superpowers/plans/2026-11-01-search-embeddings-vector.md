# Embeddings, Chunks and Three-Leg RRF Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build steps 3 and 4 of the hybrid search spec
(`docs/superpowers/specs/2026-10-31-hybrid-article-search-design.md`:
Schema, Ports and adapters, Search query, UI, Backfill, Cost). Wire the
`Embedder` port (#40) into ingestion and search on top of the lexical search
and ingest worker (#42): migration 018 (renumbered from the spec's 017, which the tag consolidation took) with `bookmark_chunks`, chunking and
the metadata fallback chunk, the vector leg as the third RRF leg, the
Settings → Search index panel, and the resumable embedding backfill. Also fix
the four follow-ups from the PR 42 review, (d) being a security fix.

**Architecture:** Hexagonal, like the rest of the server.

- Domain: `domain/chunking.rs` (pure chunker, a port of
  `scripts/search-eval/chunking.py`), `domain/search.rs` (query vector,
  embed jobs, index status, embed retry schedule), a new port
  `domain/ports/embedding_repo.rs` (`EmbeddingRepository`, the embed queue)
  and `SearchIndexRepository` next to `SearchRepository`.
- Adapters: `adapters/postgres/embedding_repo.rs` (embed queue, chunks,
  index status, retry/refetch, extension versions); the vector CTEs in
  `search_repo.rs`.
- App: `app/embedding.rs` (`EmbedWorker`: claim, chunk, embed, store; this
  is also the backfill), `app/search.rs` (query embedding with a 1.5 s
  timeout, falling back to the lexical legs).
- Web: `web/pages/search_index.rs` (the Settings panel fragment, retry,
  refetch), a "semantic match" label and chunk snippets on cards, and
  `/uploads` security headers.

**Tech Stack:** Rust 2024, Axum 0.8, SQLx 0.8, Askama 0.12, HTMX 2, Postgres 16
with `pg_trgm` and pgvector ≥ 0.8 (`halfvec`, `hnsw.iterative_scan`).

---

## Decisions (auto-pilot; answered from the spec, the code and the task)

1. **Embedding is its own worker, not a step inside the fetch job.** The spec
   lists `store_article(…, chunks, model_id)` in one transaction, but it also
   needs the same code for four other cases: an embedder outage after a fetch
   ("keep the text, `embedded_model = NULL`, retry later"), the metadata
   chunk of failed/skipped rows, the backfill of existing rows, and a model
   change. One queue covers all five: a row needs embedding when
   `status <> 'pending'` and `embedded_model IS DISTINCT FROM
   Embedder::model_id()`. `store_article` resets `embedded_model` only when
   `content_hash` changed, so a refetch of unchanged text costs nothing. The
   ingest worker wakes the embed worker in-process after each result, so a
   new bookmark is searchable by meaning within seconds. Evidence: the
   spec's Backfill section already describes the backfill as "the worker
   re-embeds from the stored text … `embedded_model = NULL`".
2. **The embed queue lives in `bookmark_contents`**, with three new columns
   in 018 (`embed_attempts`, `embed_next_attempt_at`, `embed_error`), the
   same way the fetch queue does. Claims use `FOR UPDATE SKIP LOCKED` and a
   lease. A write is accepted only while the row's `updated_at` is the one
   claimed, so an edit during an embed call drops the stale result (the next
   claim redoes it). That makes the backfill resumable (all state is in the
   DB), idempotent (chunks are replaced per bookmark) and safe against edits.
3. **Embed failures.** `Internal` (outage, 429, timeout) is retried after
   1 min, 10 min, 1 h, then every 6 h, never given up: a provider outage
   must not leave bookmarks without vectors. `InvalidInput` (the vendor will
   never accept it) records the error and marks the row done for this
   `model_id` with no chunks, so a model change retries it.
4. **Rate limit.** One bookmark per embed call (the spec's contract: at most
   64 chunks), and at least 200 ms between calls (≤ 5 calls/s, about 50
   chunks/s), well inside OpenAI's limits, and so a backfill of the 334
   production bookmarks takes about a minute. Spend is logged per bookmark
   and as a running total: `usage_tokens` only, never text.
5. **Chunk text vs metadata chunk.** Text of at least 300 characters is
   chunked (≈200-word windows, 30-word overlap, at most 64, the title as a
   header and the description on chunk 0). Anything else (failed, skipped,
   short text) gets one metadata chunk, `title\ndescription\nurl\ntags`. A
   chunk window is also capped at 1,600 characters, the spec's "cap by
   tokens" without a tokenizer in the server: normal prose (≈6 characters a
   word) keeps the prototype's 200-word windows; dense code is cut well
   below BGE's 512 tokens.
6. **The trigger keeps chunks in step with edits.** 018 replaces
   `bookmarks_sync_content()`: a URL change deletes the bookmark's chunks;
   a title or description change sets `embedded_model = NULL` (both are in
   chunk headers); a tag change does so only for metadata-chunk rows (tags
   are only in that chunk). No tag code is edited.
7. **Vector leg.** As the spec's SQL: `nn` takes the 200 nearest chunks with
   `model = Embedder::model_id()` *and* `vector_dims(embedding) = <dims>`
   (dims inlined so the partial HNSW index matches), `vec` ranks bookmarks
   by their best chunk. It runs in a transaction with
   `SET LOCAL hnsw.iterative_scan = relaxed_order` and
   `hnsw.ef_search = 100`. With a query vector the lexical leg is AND only
   (the measured 0.925 / 0.960 SQL); without one it is AND, else OR (0.791).
   Tags, quoted phrases and `-exclusions` filter inside `nn` and again on the
   fused union. The query vector is bound as `real[]` and cast to `halfvec`.
8. **Query timeout in the service, for every backend.** `SearchService`
   wraps `embed(Query)` in `tokio::time::timeout(1.5 s)`; on a timeout, an
   error or a disabled embedder it runs the lexical legs and warns at most
   once a minute. The fastembed adapter also applies the query timeout
   itself (its blocking thread cannot be cancelled, so it only stops
   waiting), and takes the model lock per 16 inputs, so a query waits for
   at most 16 document chunks during a backfill.
9. **Snippets.** A lexical hit keeps `ts_headline`. A hit whose headline is
   empty and that has an article chunk shows that chunk's text cut to about
   160 characters at a word boundary. A bookmark found only by the vector
   leg gets a "semantic match" label. Metadata chunks are never shown.
10. **Settings → Search index panel is its own fragment**
    (`GET /settings/search-index`, loaded with `hx-trigger="load"` from one
    new `<section>` in `settings/index.html`), so `settings.rs`, which the tag
    workers also touch, is unchanged. It shows indexed / pending / couldn't
    be fetched counts, the top failing domains, embedded counts when
    embeddings are on, "Retry all" (failed and skipped back to pending) and,
    for the owner role only, one line: `model_id · vector <v> · pg_trgm <v>`
    from `pg_extension`. "Refetch article" is a button on the card
    (`POST /bookmarks/{id}/refetch`), as the spec's UI section says.
11. **Follow-ups from the PR 42 review.**
    - (a) The extraction permit (`ARTICLE_INGEST_CONCURRENCY` slots) moves
      into the `spawn_blocking` closure, so a timed-out extraction keeps its
      slot until its thread ends, and hostile pages cannot exceed the limit.
    - (b) A spaced minus (`coffee - tea`) is an exclusion in Postgres's
      `websearch_to_tsquery` (checked: `'coffe' & !'tea'`). The constraint
      extraction now treats it the same, so every leg excludes `tea`.
    - (c) The claim ranks each user's due rows by host turn *before* it
      takes that user's share, so an old single-host backlog no longer
      delays the same user's links to other hosts.
    - (d) Fetched card images refuse `image/svg+xml` (and SVG never gets an
      `.svg` key), and `/uploads` is served with
      `Content-Security-Policy: default-src 'none'; sandbox` and
      `X-Content-Type-Options: nosniff`.
12. **Production config needs no repo change.** `EMBEDDING_BACKEND=openai`
    and `OPENAI_EMBEDDING_API_KEY` are set on prod; te3-large@1024 is already
    the default model and size; self-hosters default to `disabled`. The
    local-embeddings Docker image stays out of scope (a docs note).
13. **CI.** The new ignored DB tests live in
    `adapters::postgres::embedding_repo::embedding_tests` and share the
    search tests' serial lock; the "Test search and ingest DB" step's filter
    gains that module. Nothing else in CI changes.

## Tasks

- [ ] **1. Migration 018** `migrations/018_bookmark_chunks_vector.sql`:
  `CREATE EXTENSION IF NOT EXISTS vector`, `bookmark_chunks` (halfvec, no
  fixed size, `model`), user index, partial HNSW indexes for 1024 and 384,
  the embed queue columns and index, and the replaced trigger function.
- [ ] **2. Follow-ups (a)-(d)** with tests: extraction permit, spaced minus,
  host-ranked claim, SVG refusal and `/uploads` headers.
- [ ] **3. Chunker** `domain/chunking.rs` with unit tests: windows, overlap,
  header, chunk cap, character cap, short-text and metadata chunk.
- [ ] **4. Embed queue port and Postgres adapter**: claim, store chunks,
  record failure; `store_article` resets `embedded_model` on a changed
  hash.
- [ ] **5. `EmbedWorker`** with a stub embedder: chunks per bookmark,
  metadata chunk, retry schedule, permanent failure, rate limit, spend log,
  disabled embedder spawns nothing. Wire it in `main.rs`.
- [ ] **6. Vector leg** in `search_sql`, query embedding with timeout in
  `SearchService`, semantic label and chunk snippet; unit tests with a stub
  embedder (timeout, error, disabled).
- [ ] **7. Settings → Search index panel**, retry all, refetch, owner line.
- [ ] **8. DB tests** (pgvector): chunks stored and replaced, three-leg
  query, model filter, user isolation, tag filter and exclusion on the
  vector leg, cascade, embed claim and stale write; timeout fallback.
  Extend the CI step's filter.
- [ ] **9. Eval**: `app_eval.py` waits for embeddings; run
  `--features local-embeddings` (bge-small) on the local DB and record
  nDCG@10 against the spec's 0.925. Docs: README, `.env.example`, the
  search-eval README, the local-embeddings image follow-up note.
