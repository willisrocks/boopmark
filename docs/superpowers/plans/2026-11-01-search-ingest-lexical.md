# Article Ingest and Lexical Hybrid Search Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build step 2 of the hybrid search spec
(`docs/superpowers/specs/2026-10-31-hybrid-article-search-design.md`, branch
`spec-hybrid-search`): fetch and extract every bookmark's article in the
background, index it with a weighted tsvector, and replace today's boolean
search with the measured lexical-only hybrid (full text AND, else OR, plus a
`pg_trgm` leg over title + description, fused with RRF). With
`EMBEDDING_BACKEND=disabled` this is the spec's 0.791 nDCG@10 row.

**Architecture:** Hexagonal, like the rest of the server.

- Domain: `domain/search.rs` (ingest status, retry schedule, search hits,
  snippet segments) and two ports, `ArticleFetcher` and `SearchRepository`.
- Adapters: `adapters/net.rs` (the URL guard shared with the metadata
  adapters, plus a DNS resolver that refuses non-public addresses),
  `adapters/article/readability.rs` (`HtmlArticleFetcher`, reqwest +
  `dom_smoothie`), `adapters/postgres/search_repo.rs`, and
  `adapters/postgres/ingest_listener.rs` (`LISTEN bookmark_ingest`).
- App: `app/search.rs` (`SearchService`) and `app/ingest.rs` (`IngestWorker`
  with a per-host throttle).
- Web: the page and API list handlers route non-empty searches to
  `SearchService`; cards render a snippet; the sort select gains Relevance.

**Tech Stack:** Rust 2024, Axum 0.8, SQLx 0.8, Askama 0.12, HTMX 2, Postgres 16
with `pg_trgm` (contrib, ships in `postgres:16`), `dom_smoothie` 0.19 (MIT).

---

## Decisions (auto-pilot; answered from the spec, the code and the task)

1. **Migration number is 016, not the spec's 015.** 015 belongs to the tag
   worker. No `vector` extension and no chunks: those are 017's. The
   plain `embedded_model` column is in 016, as the final spec has it.
2. **The queue lives in `bookmark_contents`** (`status = 'pending'`,
   `next_attempt_at`, partial index `idx_bookmark_contents_queue`), exactly as
   the spec's Schema section. The task's "ingest queue table" is this table;
   a second table would duplicate the per-bookmark state the spec keeps in one
   row.
3. **A trigger on `bookmarks` maintains `bookmark_contents`, not each
   repository write.** The spec asks that `search`/`meta_text` be rebuilt "in
   the same transaction as the bookmark update"; a row trigger does exactly
   that, for every write path: `create`, `create_claimed`, `insert_with_id`,
   `upsert_full` (import, the only path that can change a URL),
   `update`, and `update_tags_bulk`. The last one is tag code this task must
   not edit, and the tag worker is changing tag writes in parallel. The
   trigger also sends `NOTIFY bookmark_ingest` when it enqueues.
   The tsvector is built by one SQL function,
   `bookmark_search_document(title, description, url, tags, body)`, which the
   trigger, the backfill and the repository all call, so the weights cannot
   drift apart.
4. **`ArticleFetcher` returns its own error type**, not `DomainError`: the
   worker must tell transient (retry after 1 min, 1 h, 24 h), permanent
   (`failed` now) and skipped (non-HTML) apart, and `DomainError` has no such
   distinction. Futures use the explicit `Pin<Box<dyn Future>>` like
   `MetadataExtractor`; the shared `BoxFuture` alias belongs to the ports
   task.
5. **SSRF guard.** `validate_public_url` moves to `adapters/net.rs` (the spec
   says so) and becomes `UrlGuard::check`. The article client:
   - resolves DNS through `PublicOnlyResolver`, which fails the lookup if
     *any* resolved address is not public; this covers every redirect hop and
     DNS rebinding, because the connection uses the vetted addresses;
   - checks every redirect hop with a custom `redirect::Policy` (scheme,
     credentials, local names, literal IPs; at most 5 hops);
   - disables proxies (`no_proxy()`), because a proxy would resolve names
     itself and bypass the resolver;
   - allows only `http`/`https`.
   The metadata adapters keep their behaviour; they only call the moved
   function.
6. **Search goes through a separate `SearchService`/`SearchRepository`**, so
   `BookmarkRepository` and its three mocks (one is in tag code) stay
   untouched. An empty search keeps today's `list`.
7. **Query shape.** Each leg is one CTE (`lex_and`, `lex_or`, `lex`, `trg`),
   fused in `fused`. 017 adds `nn`/`vec` CTEs and one more `UNION ALL` arm;
   the repository builds the leg list in Rust so that is one push. Each leg
   applies the user and tag filters itself, so the GIN indexes stay usable.
   The OR query is the eval's
   `replace(plainto_tsquery(..)::text, ' & ', ' | ')`, so the measured
   number reproduces.
8. **No `unaccent`** (the task says pg_trgm only). The eval's trigram leg
   used `unaccent`; that affects one typo query of 48, and the eval run
   reports the difference.
9. **Snippets** come from `ts_headline` on the body, only for lexical hits of
   fetched articles, and only when the headline highlights something. The
   SQL marks highlights with control characters (`\x02`, `\x03`); Rust splits
   them into segments, and Askama escapes each one, so page text can never
   inject HTML. The API returns the plain text as `snippet`.
10. **Sorting.** With a non-empty search the default sort is `relevance`. The
    search box no longer sends the sort select, so each new query starts in
    relevance order; choosing Newest/Oldest/Title/Domain re-sorts the 50 fused
    results. At most 50 results; the footer says so when the cap is hit.
11. **Worker connections.** The worker gets its own pool (`max_connections(1)`)
    plus the listener's dedicated connection, so it cannot starve the request
    pool. `ARTICLE_INGEST_ENABLED=0` does not spawn it (metadata-only search,
    still ranked). `ARTICLE_INGEST_CONCURRENCY` defaults to 4.
12. **Backfill** is the migration's `INSERT … SELECT` (every bookmark pending,
    already searchable by metadata); the worker drains it at the configured
    concurrency with at most one request per host per second.
13. **In scope from the spec's UI section:** relevance sort, cap, snippet,
    API `snippet`, CLI `--sort` optional while searching, privacy text.
    **Out of scope:** Settings → Search index panel, "Refetch article", the
    "semantic match" label (needs the vector leg), embeddings and chunks.

## File Structure

New:
- `migrations/016_bookmark_content_search.sql`
- `server/src/domain/search.rs`
- `server/src/domain/ports/article_fetcher.rs`, `server/src/domain/ports/search_repo.rs`
- `server/src/adapters/net.rs`
- `server/src/adapters/article/mod.rs`, `server/src/adapters/article/readability.rs`
- `server/src/adapters/postgres/search_repo.rs`, `server/src/adapters/postgres/ingest_listener.rs`
- `server/src/app/search.rs`, `server/src/app/ingest.rs`
- `scripts/search-eval/app_eval.py`

Modified (small, additive): `domain/mod.rs`, `domain/ports/mod.rs`,
`domain/bookmark.rs` (`BookmarkSort::Relevance`), `adapters/mod.rs`,
`adapters/metadata/{mod,html,iframely,opengraph_io}.rs` (use `net`),
`adapters/postgres/{mod,bookmark_repo}.rs` (Relevance arm), `app/mod.rs`,
`config.rs`, `main.rs`, `web/state.rs`, `web/pages/bookmarks.rs`,
`web/api/bookmarks.rs`, `templates/bookmarks/card.html`,
`templates/bookmarks/list.html`, `templates/components/{header,filters_inner}.html`,
`templates/legal/privacy.html`, `cli/src/main.rs`, `server/Cargo.toml`.

## Tasks

### Task 1: Migration 016
- [ ] `pg_trgm`; `bookmark_search_document()` (IMMUTABLE, A title, B
      description, C url + tags, D body); `bookmark_contents` with the spec's
      columns (`embedded_model` included, unused until 017); GIN on `search`, GIN trigram on
      `meta_text`, the queue partial index, the user index.
- [ ] Trigger `bookmarks_sync_content` (AFTER INSERT OR UPDATE OF title,
      description, url, tags): upsert the row; a URL change resets it to a
      fresh pending row; other edits rebuild `search` with the stored body.
      `pg_notify('bookmark_ingest', '')` on enqueue.
- [ ] Backfill `INSERT … SELECT` from `bookmarks`.
- [ ] Apply on a nix Postgres 16 on port 55433 together with 001–015 as
      found on main. Commit.

### Task 2: Domain and ports
- [ ] `domain/search.rs`: `IngestStatus`, `retry_delay(attempts)`
      (1 min, 1 h, 24 h, then none), `SearchHit`, `Snippet` segments parsed
      from the sentinel string, `MAX_SEARCH_RESULTS = 50`. Unit tests.
- [ ] `ArticleFetcher` port: `FetchedArticle { text, final_url, html_bytes }`
      and `ArticleFetchError { Transient, Permanent, Skipped }`.
- [ ] `SearchRepository` port: `search`, `claim_ingest_batch`,
      `store_article`, `record_ingest_failure`. Commit.

### Task 3: URL guard and article fetcher
- [ ] `adapters/net.rs`: move `validate_public_url` and its tests; add
      `is_public_ip`, `UrlGuard`, `PublicOnlyResolver` over a `Lookup`
      trait, `guarded_redirect_policy`. Tests: DNS name resolving to 10.x,
      mixed public/private answers, IPv4-mapped IPv6, every redirect hop.
- [ ] `HtmlArticleFetcher`: 15 s timeout, 5 s connect, 5 MB streamed cap,
      `text/html`/`application/xhtml+xml` only, Cloudflare detection shared
      with `html.rs`, `dom_smoothie` on `spawn_blocking` with an element cap,
      200k-char text cap. Status mapping: 408/425/429/5xx transient; other 4xx
      permanent; non-HTML and oversize skipped.
- [ ] Fixture-server tests (axum on 127.0.0.1 behind a test resolver that maps
      `fixture.test`): extraction, content type, size cap, 404/503, CF
      challenge, redirect to `10.0.0.1` and to a name resolving to it. Commit.

### Task 4: Postgres search repository
- [ ] `search`: the CTE query, sort, cap, snippet. `claim_ingest_batch`:
      one `UPDATE … FROM (SELECT … FOR UPDATE SKIP LOCKED)` moving
      `next_attempt_at` forward as a 10-minute lease and counting the attempt.
      `store_article` and `record_ingest_failure` only touch the row while
      the bookmark still has the claimed URL; 0 rows is "done".
- [ ] Ignored Postgres test: ranking (title beats body), AND→OR fallback,
      prefix and typo via trigram, tag filter, user isolation, sort re-order,
      snippet escaping, trigger on URL/title change, cascade delete,
      concurrent claimers. Commit.

### Task 5: Ingest worker
- [ ] `HostThrottle` (one request per host per second), `IngestWorker::run`
      (claim, fetch with throttle concurrently, store or record failure,
      sleep 2 s or wake on notify). Unit tests with fakes and paused time.
- [ ] `ingest_listener.rs`: `PgListener` → `Notify`, reconnecting.
- [ ] Config `ARTICLE_INGEST_ENABLED`, `ARTICLE_INGEST_CONCURRENCY`; spawn in
      `main.rs`. Commit.

### Task 6: Search service, web, API, CLI
- [ ] `SearchService`; `AppState.search`; page and API handlers; card snippet,
      cap footer, Relevance option, header include change; API `snippet`;
      CLI `--sort` optional; privacy text. Commit.

### Task 7: Prove it
- [ ] `cargo fmt`, `cargo clippy --workspace --all-targets -D warnings`,
      `cargo test --workspace`, the ignored Postgres tests.
- [ ] `scripts/search-eval/app_eval.py`: seed the pinned corpus into the app's
      tables (the 72 failed links stay metadata-only), query the running
      server's `/api/v1/bookmarks?search=`, report nDCG@10 per type against
      0.791. Optionally `--ingest` to let the worker fetch with dom_smoothie.
- [ ] Push, PR, report.
