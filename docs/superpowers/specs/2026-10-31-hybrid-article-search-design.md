# Hybrid Full-Article Search — Design

**Status:** Draft (exploration; prototype on branch `spec-hybrid-search`)
**Date:** 2026-10-31
**Prototype + harness:** `scripts/search-eval/` (corpus fetcher, seed, chunker, labelled queries, eval)

## Summary

Search the **full article text** of every bookmark, not only its title,
description and URL, with **hybrid retrieval**: a weighted Postgres full-text
leg and a pgvector embedding leg over article chunks, fused with Reciprocal
Rank Fusion (RRF) in one SQL statement. Article text is fetched and extracted
by a background ingestion worker after the bookmark is saved, so saving stays
as fast as today.

On a 490-article corpus (342 Wikipedia pages + 148 extracted Hacker News
articles) with 24 hand-labelled queries, the recommended hybrid scores
**nDCG@10 0.936 / MRR@10 0.979 / recall@10 0.960**. Today's search scores
**nDCG@10 0.106** and returns nothing for 83% of the queries.

**Recommendation: Option A.** Postgres-native hybrid search: pgvector (HNSW) +
weighted `tsvector` (`websearch_to_tsquery`, ranked by `ts_rank`), fused with RRF. Embeddings come through a
new `Embedder` port. An operator-configured hosted model is the production
default (`text-embedding-3-small`, about **$0.05 per 1,000 bookmarks** to
ingest). A local fastembed adapter covers self-hosters, and a `disabled`
setting degrades to full-text-only search. This is the only option that runs
on Fly Managed Postgres, which supports pgvector but not ParadeDB or any other
BM25 extension.

## Problem

Today's search (`bookmark_repo.rs::list`) is a boolean filter over
`to_tsvector(title || description || url)` (index from migration 004) with
`plainto_tsquery`. Results come back in the user's chosen sort order
(newest first by default), not by relevance. Three things follow:

1. **Nothing below the description is searchable.** When you remember a
   bookmark by something it *said*, the search misses it.
2. **Every term must appear in the metadata** (`plainto_tsquery` ANDs every
   word), so natural-language queries almost always return zero results. Of
   the 24 evaluation queries, 20 returned nothing.
3. **No ranking.** When a query does match, the best result can sit anywhere
   in the newest-first list.

The page fetch already exists, but the article body is thrown away.
`HtmlMetadataExtractor` downloads the whole HTML page to read the `og:*` tags
and discards everything else.

## Goals

- Find a bookmark by any passage of its article, by keyword *or* by meaning.
- Rank by relevance, and show why each result matched (a snippet).
- Fit the current stack: Postgres on Fly MPG, SQLx 0.8, hexagonal ports and
  adapters, HTMX. No new always-on service.
- Keep saving a bookmark as fast as it is today; ingestion is asynchronous.
- Degrade gracefully: if the embedder is unavailable, search falls back to
  full-text; if the article fetch fails, metadata stays searchable.
- Self-hosting stays possible without any paid API.

## Non-Goals (v1)

- PDFs, YouTube transcripts, and JS-rendered SPAs (v1 ingests HTML only; these
  show up as `skipped` or `failed`).
- Question answering / RAG over bookmarks (this design is its foundation,
  though).
- Cross-user search, and offline search in the mobile app or the browser
  extension.
- Re-crawling bookmarks to pick up changed articles (the text is a snapshot
  taken at save time; manual "refetch" only).

---

## Evidence: what was measured

### Environment

Docker is not available on the shared host, so `devproxy` could not run.
The prototype instead runs Postgres 16.15 with pgvector 0.8.2 and ParadeDB
pg_search 0.15 from nixpkgs, on its own port (55432). The real server
(`boopmark-server`, migrations 001–014) runs on port 4100 against that
database, with one seeded user. The app shows the seeded bookmarks, and its
live search reproduces the "today" numbers below. For example, `/bookmarks?search=how do bees tell other bees where the flowers are`
returns 0 cards, while `search=bees` returns 15.

### Corpus (`fetch_corpus.py`; regenerated, not committed)

| source | docs | article chars median / p90 / max | notes |
|---|---|---|---|
| Wikipedia, 20 categories × ≤18 pages | 342 | 5.6k / 22k / 92k | Neighbouring pages in a category serve as hard negatives (sorting algorithms, hash functions, Icelandic volcanoes, coffee, …). |
| Hacker News stories with >900 points, extracted with trafilatura | 148 of 220 | 7.2k / 32k / 99k | Real-world ingestion: blogs, news, vendor pages, X posts. |

Totals: **490 bookmarks, 5,052 chunks**, 1.14M article tokens (2,334 per
bookmark, cl100k), 1.32M embedding-input tokens (2,696 per bookmark, which
includes chunk overlap and headers).

**Ingestion failure rate on real links (HN, n=220): 33% failed.**

| outcome | count |
|---|---|
| extracted (≥800 chars) | 148 (67%) |
| extraction too short (JS apps, x.com, homepages) | 30 |
| HTTP 403 (bot walls, Cloudflare) | 27 |
| HTTP 401 / 404 / 429 / 520 | 11 |
| timeouts / not HTML / other | 4 |

- HTML size: median 87 KB, p90 437 KB, max 1.9 MB.
- Fetch + extract time: median 0.77 s, p90 2.35 s.
- Wikipedia (API text): 100% extracted.

### Labelled queries (`queries.jsonl`)

There are 24 queries in three types:

- **keyword** (7): exact terms, e.g. `sscanf strlen`, `Laki 1783 eruption`.
- **natural** (9): paraphrases that share few words with the target, e.g.
  *"why the first price I hear skews my judgement"* → Anchoring effect.
- **deep** (8): the answer sits in the article body, not in the title or
  description, e.g. *"why a game's online mode took minutes to load and how
  parsing JSON was fixed"* → "How I cut GTA Online loading times by 70%".

Relevance labels were written first. They were then completed by judging the
**pool**, the union of every method's top 10 (TREC-style), so no method was
scored against labels that only another method could find. The pool was judged twice: once after the main methods ran, and again after
the embedding variants (metadata-only, MiniLM, bge-base) added new hits. The
second pass added no relevant documents, so the variant rows are not biased by
labels frozen before they ran. Each query has 1–8 relevant bookmarks.

### Results (exact KNN; `eval.py --bm25`)

| method | R@10 | MRR@10 | nDCG@10 | zero-result | nDCG keyword | nDCG natural | nDCG deep | p50 ms |
|---|---|---|---|---|---|---|---|---|
| **today** (metadata filter, newest first) | 0.090 | 0.167 | 0.106 | 83% | 0.222 | 0.111 | 0.000 | 0.2 |
| metadata `ts_rank` | 0.090 | 0.167 | 0.106 | 83% | 0.222 | 0.111 | 0.000 | 17 |
| full-text AND (`websearch_to_tsquery`, `ts_rank_cd`) | 0.625 | 0.708 | 0.635 | 25% | 0.976 | 0.490 | 0.500 | 0.3 |
| full-text OR (`ts_rank`) | 0.840 | 0.686 | 0.722 | 0% | 0.923 | 0.546 | 0.746 | 2.2 |
| BM25 (ParadeDB pg_search) — *not on Fly MPG* | 0.924 | 0.833 | 0.848 | 0% | 0.984 | 0.663 | 0.938 | 1.9 |
| vector (bge-small-en-v1.5, 384-d, chunks) | 0.960 | 0.948 | 0.918 | 0% | 0.960 | 0.814 | 1.000 | 12.6 |
| **hybrid RRF: full-text AND + vector** ⭐ | **0.960** | **0.979** | **0.936** | 0% | **0.990** | **0.838** | 1.000 | 12.9 |
| hybrid RRF: full-text OR + vector | 0.950 | 0.844 | 0.858 | 0% | 0.965 | 0.753 | 0.883 | 15.4 |
| hybrid RRF: BM25 + vector — *not on Fly MPG* | 0.939 | 0.943 | 0.912 | 0% | 0.981 | 0.780 | 1.000 | 15.3 |

**Embedding variants** (`embed_variant.py`; the same labels, the same
full-text AND lexical leg in the hybrid; exact KNN):

| variant | dims | embed speed (4 shared vCPU) | vector nDCG@10 | hybrid nDCG@10 | hybrid MRR@10 | vector nDCG keyword / natural / deep |
|---|---|---|---|---|---|---|
| **bge-small-en-v1.5, article chunks** (prototype default) | 384 | 9.6 chunks/s | 0.918 | 0.936 | 0.979 | 0.960 / 0.814 / 1.000 |
| bge-small-en-v1.5, **metadata only** (title + description + URL; 1 vector per bookmark) | 384 | 39 docs/s | 0.813 | 0.915 | 0.946 | 0.713 / 0.765 / 0.954 |
| all-MiniLM-L6-v2, article chunks | 384 | 35 chunks/s | 0.919 | 0.926 | 0.979 | 0.961 / 0.855 / 0.954 |
| bge-base-en-v1.5, article chunks | 768 | 3.2 chunks/s | 0.905 | **0.941** | 0.979 | 0.942 / 0.859 / 0.923 |
| OpenAI text-embedding-3-small, article chunks | 1536 | — | pending | pending | — | not run: no API key was available (see Open questions) |

The variants show three things:

- **Chunk embeddings of the article add +0.105 vector nDCG** over embedding
  only the metadata (0.813 → 0.918). Most of that is on keyword queries
  (0.713 → 0.960).
- **Inside the hybrid, the article's lexical leg recovers much of the
  difference** (0.915 vs 0.936). A cheap "one vector per bookmark" variant is
  a credible fallback if storage or embedding cost ever matters (see Option
  A, variant A2).
- **The three local models are within noise of each other** (0.926–0.941
  hybrid). Model choice can rest on cost, speed and RAM rather than quality.


Latency is the SQL time only (warm, local, 490 bookmarks / 5k chunks). Query
embedding is extra: **6.5 ms p50 / 10.3 ms p95** locally with bge-small (warm, one query at a time, printed by `eval.py`; a standalone run measured 4.1 / 7.0 ms; the process RSS with ONNX Runtime and the model loaded was 274 MB in Python), or one HTTPS
round trip (~100–300 ms) for a hosted model. With
`hnsw.iterative_scan = relaxed_order` and `ef_search = 100`, the HNSW index
gave identical rankings and latency at this size. Per-user corpora are small
enough that the planner's exact scan over the user's chunks is fine; HNSW
matters once the *table* holds millions of chunks across users.

### What the numbers say

1. **Full article text is the biggest single win.** Indexing the body with
   the same Postgres FTS takes nDCG from 0.106 to 0.635–0.722 and cuts
   zero-result queries from 83% to 0–25%. That gain needs no embeddings, so
   the `disabled` setting is still a big upgrade.
2. **Embeddings carry the natural-language and deep queries** (natural nDCG
   0.814 vs 0.490 for the full-text AND query). These are the "I remember what
   it was about" searches a bookmark app is for.
3. **Hybrid beats either leg alone**, mainly on keyword queries (0.990 vs
   0.960 for vector alone). Exact identifiers (`sscanf`, `SHA-1`, `Laki`) and
   rare names favour the lexical leg; vectors blur them.
4. **The lexical leg must be precise, not high-recall.** The OR query and
   BM25 have better recall on their own, but they flood the fusion with weak
   matches and *lower* the hybrid score (0.858 and 0.912 vs 0.936). The AND
   leg contributes only when every term matches, which is exactly when it is
   trustworthy.
5. **BM25 vs `ts_rank`:** pg_search BM25 is clearly better than `ts_rank` as
   a standalone ranker (0.848 vs 0.635/0.722). Once fused with vectors, that
   advantage disappears on this set. Together with its unavailability on MPG
   (below), this rules out Option B for v1.
6. **Caveats.** 24 queries is small: one query moves nDCG by about 0.04, so
   0.936 vs 0.918 is a direction, not proof. The corpus favours vectors,
   because its topics are distinct; a collection of very similar items (50
   Rust blog posts) leans more on the lexical leg. Wikipedia text is cleaner
   than extracted HTML. The labels are hand-made by one judge.

---

## Platform constraint: what Fly Managed Postgres allows

These facts were checked on 2026-11-01:

- MPG supports exactly two third-party extensions: **`vector` (pgvector) and
  PostGIS** ([docs.fly.io/mpg/extensions](https://docs.fly.io/mpg/extensions)).
- They are enabled from the dashboard's Extensions page. The app role is not
  a superuser, so `CREATE EXTENSION vector` fails until the toggle is on
  ([community thread, 2026-03](https://community.fly.io/t/enabling-pgvector-extension-on-managed-postgres-cluster/27387)).
  Clusters created before 2025-07-23 need a role-system upgrade first
  ([announcement](https://community.fly.io/t/mpg-extensions-management-is-here/25551)).
- `pg_trgm`, `unaccent`, `btree_gin` and `fuzzystrmatch` are available as
  built-in contrib extensions.
- **None of the BM25 extensions are listed:** ParadeDB pg_search, Tiger's
  pg_textsearch, and vchord_bm25. All of them need
  `shared_preload_libraries`, which MPG does not expose.
- Plans: Basic, 1 GB RAM, $38/mo; Starter, 2 GB, $72/mo. Storage is
  $0.28/GB-month.

---

## Proposals

### Option A — Postgres-native hybrid (pgvector + tsvector + RRF) ⭐ recommended

- **Lexical leg:** a weighted `tsvector` over title (A), description (B),
  URL (C) and article body (D), stored in a new `bookmark_contents` table with
  a GIN index. The query is `websearch_to_tsquery('english', $q)`, ranked by
  `ts_rank(search, q, 1)` (this is what the measured hybrid used; the
  standalone "full-text AND" row used `ts_rank_cd`). Users also get quotes and `-exclusion` for
  free.
- **Vector leg:** `bookmark_chunks(embedding vector(N))` with an HNSW cosine
  index. Each bookmark scores its best chunk (`min(distance)`).
- **Fusion:** RRF with k=60 over the top 50 of each leg, in one SQL
  statement (below). There are no score weights to tune.
- **Pros:**
  - Runs on MPG today.
  - One datastore, transactional with the bookmark row, so delete is a
    cascade.
  - SQLx-friendly; no new service.
  - Measured best.
- **Cons:**
  - `ts_rank` is not BM25. Measured, it doesn't matter once fused.
  - The vector index adds RAM pressure on the 1 GB MPG plan at scale (see
    Cost).
- **Variant A2: one vector per bookmark.** Embed only title + description +
  URL, and keep the full article in the lexical leg. Measured hybrid nDCG is
  0.915 vs 0.936, with about 10× fewer vectors and no chunk table. Keep it in
  reserve for very large libraries; it is not the default, because the
  article chunks also supply the snippets for semantic hits.

### Option B — Postgres + ParadeDB pg_search (BM25) + pgvector

- Real BM25 (Tantivy) in Postgres, best for lexical-only search.
- **Not available on Fly MPG.** It would need one of:
  - self-managed Postgres on a Fly Machine with a volume, giving up MPG's
    HA, backups and pooling;
  - ParadeDB Cloud as a logical-replication read target (eventually
    consistent, an extra bill, and data leaves Fly);
  - or a move off MPG.
- The licence is AGPL-3.0. That is fine for this AGPL-compatible open-source
  app, but it is a consideration for anyone redistributing modified builds.
- Measured: fused with vectors, it is *not* better than Option A (0.912 vs
  0.936).
- **Verdict:** reject for v1. If a future MPG release adds pg_search, swap in
  the lexical leg behind the same repository method, with no other changes.

### Option C — External search index (Meilisearch / Typesense / Qdrant / Elasticsearch)

- Built-in hybrid ranking and typo tolerance; scales independently.
- Costs:
  - another always-on Fly Machine plus a volume (about $5–30/mo);
  - dual writes with an outbox to keep it consistent;
  - deletions and user scoping enforced in two places;
  - backups and an upgrade path for a second stateful service;
  - self-hosters must run it too.
- Not measured. At this app's scale (thousands, not hundreds of millions, of
  chunks) it brings operational cost with no measured quality gain.
- **Verdict:** reject. Revisit only if per-query latency at production scale
  exceeds about 200 ms on MPG.

### Embedding model — sub-options

| option | quality (this eval) | ingest cost / 1k bookmarks | query cost | ops | verdict |
|---|---|---|---|---|---|
| **E1. Operator-paid hosted, `text-embedding-3-small`** | not measured (no key, see Open questions) | 2.7M tok × $0.02/M = **$0.054** | ~12 tok/query: $0.24 per 1M queries | API key in Fly secrets; network call per query | ⭐ **production default** |
| E1'. Hosted, Voyage `voyage-4-lite` | not measured | $0.054 (200M free tokens) | same | same | Equivalent alternative; 32k context |
| **E2. Local fastembed (`bge-small-en-v1.5`, ONNX)** | vector 0.918 / hybrid 0.936 | $0 API cost; measured **9.6 chunks/s** (≈1 bookmark/s) on 4 shared vCPUs | ≈5 ms CPU (measured) | about 130 MB model + ONNX runtime; does **not** fit next to the app in the current 512 MB `shared-cpu-1x` VM; needs ≥1 GB | ⭐ **self-host option** |
| E3. Per-user BYO key (like `user_llm_settings`) | — | user pays | user pays; needs the user's key on every search | Anthropic, the default text provider, has no embeddings API, so most users get nothing. Index coverage differs per user. A key rotation or removal breaks query embedding, and a model switch means a full re-embed per user. | reject for v1 |
| E4. `disabled` (full-text only) | full-text AND 0.635 | $0 | $0 | none | ⭐ fallback / default when unconfigured |

Why **operator-paid** over BYO, given that BYO matches today's LLM settings:

- **Embeddings cost very little.** Embedding tokens are priced at $0.02/M,
  against about $1/M input for the cheapest enrichment chat models, and an
  embedding call has no output tokens (not measured end to end). Ingesting 1k
  bookmarks costs about $0.05, and a heavy user's searches cost
  fractions of a cent per month.
- **The index must be one consistent vector space.** A per-user key breaks
  that: users without an OpenAI key (the default provider is Anthropic, which
  has no embeddings) would silently get a worse search, and query embedding
  would depend on a user secret at read time.

Self-hosters choose the backend the same way they choose `STORAGE_BACKEND`,
`SCREENSHOT_BACKEND` or `METADATA_FALLBACK_BACKEND`.

---

## Design (Option A + E1/E2/E4)

### Configuration

```
EMBEDDING_BACKEND=disabled | openai | fastembed     # default: disabled
EMBEDDING_MODEL=text-embedding-3-small              # or BAAI/bge-small-en-v1.5
EMBEDDING_DIMENSIONS=512                            # must match the migration's column (see Storage)
OPENAI_EMBEDDING_API_KEY=...                        # operator key; separate from users' BYO keys
ARTICLE_INGEST_CONCURRENCY=4
ARTICLE_INGEST_ENABLED=1                            # 0 = metadata-only (today's behaviour + ranking)
```

`fastembed` sits behind a cargo feature, `local-embeddings`, so the default
build and binary don't carry ONNX Runtime. The `ort` crate is still at
2.0.0-rc.

### Schema (migration `015_bookmark_content_search.sql`)

The prototype version is in `scripts/search-eval/sql/prototype_schema.sql`.

```sql
CREATE EXTENSION IF NOT EXISTS vector;  -- on MPG: enable in the dashboard first

CREATE TABLE bookmark_contents (
    bookmark_id UUID PRIMARY KEY REFERENCES bookmarks(id) ON DELETE CASCADE,
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    status      TEXT NOT NULL DEFAULT 'pending',  -- pending|fetched|failed|skipped
    attempts    INT  NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    error       TEXT,
    text        TEXT,              -- extracted article text, ≤ 200k chars
    text_chars  INT NOT NULL DEFAULT 0,
    content_hash TEXT,             -- sha256(text): skip re-embedding unchanged text
    search      tsvector,          -- A title, B description, C url, D body
    embedded_model TEXT,           -- model the chunks were built with
    fetched_at  TIMESTAMPTZ,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_bookmark_contents_user ON bookmark_contents(user_id);
CREATE INDEX idx_bookmark_contents_search ON bookmark_contents USING GIN(search);
CREATE INDEX idx_bookmark_contents_queue ON bookmark_contents(next_attempt_at)
    WHERE status = 'pending';

CREATE TABLE bookmark_chunks (
    bookmark_id UUID NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE,
    user_id     UUID NOT NULL,
    ord         INT  NOT NULL,
    text        TEXT NOT NULL,     -- the passage, shown as a snippet
    embedding   halfvec(512) NOT NULL,
    PRIMARY KEY (bookmark_id, ord)
);
CREATE INDEX idx_bookmark_chunks_user ON bookmark_chunks(user_id);
CREATE INDEX idx_bookmark_chunks_hnsw ON bookmark_chunks
    USING hnsw (embedding halfvec_cosine_ops);

-- Backfill queue: every existing bookmark starts pending.
INSERT INTO bookmark_contents (bookmark_id, user_id, search)
SELECT id, user_id,
       setweight(to_tsvector('english', coalesce(title, '')), 'A') ||
       setweight(to_tsvector('english', coalesce(description, '')), 'B') ||
       setweight(to_tsvector('english', url), 'C')
FROM bookmarks;
```

- **Why a separate table.** The `bookmarks` row stays narrow; the list page
  never reads the article text. Delete cascades. A failed fetch still has a
  metadata-only `search` vector, so the bookmark stays findable.
- **Why `search` is maintained by the app** and not a generated column: it
  combines `bookmarks.title/description/url` with the body. A generated
  column can't read another table. The repository rebuilds it whenever the
  title, description, URL or text changes, in the same transaction as the
  bookmark update.
- **Why `halfvec(512)`.** `text-embedding-3-small` supports shortened
  Matryoshka output (`dimensions: 512`). Combined with half-precision storage
  (pgvector ≥ 0.7), that is 1 KB per chunk instead of 6 KB at 1536-d
  float32, which keeps the HNSW index in RAM on the 1 GB plan. The local
  `bge-small` (384-d) uses `halfvec(384)`. Because the dimension lives in the
  migration, **changing models is a migration plus a re-embed backfill**. The
  `embedded_model` column lets the worker find stale rows. (Not measured:
  quality loss from 512-d truncation; published Matryoshka results put it at
  about 1–2% nDCG.)
- **Self-hosting / dev / CI** need a Postgres with pgvector:
  - `docker-compose.yml`: `postgres:16-alpine` → `pgvector/pgvector:pg16`;
  - the CI `postgres:16` service → `pgvector/pgvector:pg16`;
  - (`scripts/e2e/start-server.sh` reuses the compose `db`, so the compose change covers E2E);
  - the self-hosting docs.
  (A CI change is an implementation step for the owner, not part of this
  spec PR.)

### Ports and adapters

```rust
// domain/ports/article_fetcher.rs
pub struct FetchedArticle { pub text: String, pub final_url: String, pub html_bytes: usize }
pub trait ArticleFetcher: Send + Sync {
    fn fetch(&self, url: &str) -> BoxFuture<'_, Result<FetchedArticle, DomainError>>;
}

// domain/ports/embedder.rs
pub trait Embedder: Send + Sync {
    fn model_id(&self) -> &str;               // stored in embedded_model
    fn dimensions(&self) -> usize;
    fn embed_documents(&self, texts: &[String]) -> BoxFuture<'_, Result<Vec<Vec<f32>>, DomainError>>;
    fn embed_query(&self, text: &str) -> BoxFuture<'_, Result<Vec<f32>, DomainError>>;
}
```

The query and document methods are separate because models such as BGE,
Voyage and Nomic use different prefixes or `input_type` for each.

**Adapters:**

- `adapters/article/readability.rs`: `HtmlArticleFetcher`, using reqwest and
  `dom_smoothie` (a Readability.js port; MIT; active, 0.19 in 2026-10).
- `adapters/embedding/openai.rs`: batches of up to 96 inputs, `dimensions`
  set from config, timeout 20 s for documents and 1.5 s for queries.
- `adapters/embedding/fastembed.rs`: behind the `local-embeddings` feature;
  runs on the blocking pool (`spawn_blocking`).
- `adapters/embedding/disabled.rs`: returns `Err(Unavailable)`, and the
  search service skips the vector leg.

**Repository** (`BookmarkRepository` grows, or a new `SearchRepository` port
does; prefer the new port to keep the trait from growing):

- `search(user_id, query, query_vec: Option<Vec<f32>>, tags, limit, offset)
  -> Vec<SearchHit { bookmark, snippet, matched: Lexical|Semantic|Both }>`
- `claim_ingest_batch(n) -> Vec<(bookmark_id, url)>`, using `FOR UPDATE SKIP
  LOCKED` and moving `next_attempt_at` forward as a lease
- `store_article(bookmark_id, text, hash, chunks: Vec<(String, Vec<f32>)>, model)`
  in one transaction: replace the chunks, rebuild `search`, set
  `status='fetched'`
- `mark_ingest_failed(bookmark_id, error, retry_at: Option<DateTime>)`

### Ingestion

The existing pieces are:

- `EnrichmentService::suggest`, called by `/api/v1/bookmarks/suggest` (the
  extension) and by the page route `/bookmarks/suggest` (the add modal);
- `BookmarksService::prepare_create`, called on save when `needs_metadata()`
  is true.

Both call `MetadataExtractor::extract`, which fetches the page and keeps only
the metadata: the `og:*` tags, falling back to `<title>` and
`meta name=description`, plus a resolved image URL. Neither should get slower. So:

1. **Enqueue on save.** `create` / `create_claimed` / `import_batch` insert the
   `bookmark_contents` row (`status='pending'`, metadata-only `search`) in the
   same transaction as the bookmark. A URL change on `update` resets it to
   `pending`; a title or description edit only rebuilds `search`.
2. **Worker.** `IngestWorker` is a long-lived tokio task spawned in
   `main.rs`. It would be the first one there; the image-fix and
   tag-consolidation jobs are spawned from request handlers. It runs with its own one-connection budget
   so it can't starve the request pool (`max_connections(5)` today). Each
   loop:
   - claims up to `ARTICLE_INGEST_CONCURRENCY` rows;
   - fetches with **at most one request per host per second** (imports are
     often many links to one site);
   - extracts, chunks and embeds, then stores;
   - sleeps 2 s when the queue is empty. It wakes immediately on a
     `NOTIFY bookmark_ingest`, so a freshly saved bookmark is searchable by
     its body within seconds.
3. **Fetch.**
   - Reuse the URL safety check `adapters/metadata/mod.rs::validate_public_url`.
     It is `pub(super)` today, so move it to a shared `adapters/net.rs`.
   - **Fix two SSRF gaps that full-body fetching makes more important:**
     re-validate every redirect hop (a custom `reqwest::redirect::Policy`;
     today reqwest follows up to 10 redirects without checking), and reject
     hostnames that *resolve* to private addresses (today only literal IPs
     and `localhost`/`.local`/`.internal` names are rejected).
   - Same User-Agent and Cloudflare-challenge detection as `html.rs`.
4. **Limits.**
   - Response body: ≤ 5 MB, read as a stream and aborted past the cap
     (measured p90 437 KB, max 1.9 MB).
   - `Content-Type` must be `text/html` or `application/xhtml+xml`.
     Anything else is `skipped`, with the reason recorded.
   - Timeout: 15 s total.
   - Extracted text: ≤ 200,000 chars. The measured max was 99k; p90 was 22k
     for Wikipedia and 32k for HN.
   - Text under 300 chars is stored, but not chunked or embedded; the
     lexical leg still sees it.
   - At most 64 chunks embedded per bookmark (6 of the 490 hit the cap). The
     whole capped text stays in the lexical index.
5. **Failures.**
   - Transient errors (timeout, 429, 5xx, connection errors) are retried
     after 1 min, then 1 h, then 24 h, then marked `failed`.
   - Permanent errors (401/403/404/410, non-HTML, a Cloudflare challenge)
     are marked `failed` straight away.
   - If the embedder fails (provider outage), keep the text, set
     `status='fetched'` with `embedded_model = NULL`, and retry embedding
     later. Full-text search works immediately.
   - A failed bookmark is still searchable by its metadata. Expect **about a
     third of real-world links to fail**, as measured. They are listed in
     Settings → Search index with a "Retry" button.
6. **Chunking** (prototype: `scripts/search-eval/chunking.py`; the Rust
   version mirrors it):
   - Paragraph-aware windows of about 200 words with a 30-word overlap,
     giving about 10 chunks per bookmark.
   - Each chunk is embedded with a header made of the title, plus the
     description on chunk 0, so a deep passage keeps its article's topic.
   - The chunk's text is stored without the header and shown as the snippet.
   - Production should cap windows by **tokens**, not words: 512 for BGE,
     about 1k for hosted models. One sampled chunk in code-heavy text
     reached 962 tokens, which BGE silently truncates.

### Search query (`SQL_HYBRID_AND` in `eval.py`, minus its eval-only `model` filter)

```sql
WITH lex AS (
    SELECT bookmark_id, row_number() OVER (ORDER BY ts_rank(search, q, 1) DESC) AS r
    FROM bookmark_contents, websearch_to_tsquery('english', $2) q
    WHERE user_id = $1 AND search @@ q
    ORDER BY ts_rank(search, q, 1) DESC LIMIT 50
),
nn AS (
    SELECT bookmark_id, embedding <=> $3 AS dist
    FROM bookmark_chunks
    WHERE user_id = $1
    ORDER BY embedding <=> $3 LIMIT 200
),
vec AS (
    SELECT bookmark_id, row_number() OVER (ORDER BY min(dist)) AS r
    FROM nn GROUP BY bookmark_id ORDER BY min(dist) LIMIT 50
)
SELECT bookmark_id, sum(1.0 / (60 + r)) AS score
FROM (SELECT * FROM lex UNION ALL SELECT * FROM vec) u
GROUP BY bookmark_id
ORDER BY score DESC
LIMIT $4 OFFSET $5;
```

- **Tags filter.** Join `bookmarks` in both legs with `AND b.tags && $tags`.
  This is not measured, but it is cheap: GIN on `tags`, applied after the
  user scope.
- **No query embedding** (backend disabled, a timeout over 1.5 s, or a
  provider error): drop the `nn`/`vec` CTEs and run the lexical leg alone.
  This is the measured "full-text AND" row.
- **Index use.** With `SET LOCAL hnsw.iterative_scan = relaxed_order` and
  `hnsw.ef_search = 100`, the user filter doesn't starve the HNSW scan as
  the table grows. pgvector ≥ 0.8 is required; MPG's version wasn't
  published, so check on the cluster before rollout.
- **Short and empty queries.** A one-word query still runs both legs.
  Vectors always return *something*, which is why results are capped (UI,
  below). An empty query keeps today's list behaviour.

### UI (HTMX)

The search input in `templates/components/header.html` keeps its
`hx-get="/bookmarks"` contract.

- **Ordering.** With a non-empty `search`, results come in relevance order. A
  **"Relevance"** option is added to the sort dropdown and selected while
  searching. Choosing Newest, Oldest, Title or Domain re-sorts the fused
  result set; it doesn't widen it.
- **Cap.** At most 50 fused results, paginated with the existing
  `limit`/`offset` up to that cap. Vector search has no natural "no match",
  so an uncapped result set would list the whole library. A footer reads
  *"Showing the 50 most relevant"*.
- **Snippet.** Each card gets a one-line snippet: `ts_headline` on the body
  for lexical hits, otherwise the best chunk's text cut to about 160 chars.
  It's rendered in a small `<p class="text-gray-400 text-sm">` under the
  description, with the matched terms in `<mark>`. A subtle "semantic match"
  label appears when only the vector leg found the bookmark, so a result
  with no keyword overlap doesn't look like a bug.
- **Status.** Settings gets a **Search index** panel: "Indexed 1,204 of 1,290
  articles · 86 couldn't be fetched (Retry all)", with the failing domains
  listed. The bookmark card menu gains **"Refetch article"**.
- **Same contract for API and CLI.** `GET /api/v1/bookmarks?search=` returns
  relevance order plus an optional `snippet` field. `boop search` gets
  relevance order with no client change. `boop list --search` always sends
  `sort=newest` (its clap default, `cli/src/main.rs:33`), so it needs a small
  client change: make `--sort` optional, and omit it while searching.
- **Privacy.** `templates/legal/privacy.html` must say that the article text
  of saved pages is fetched and stored. With a hosted embedder, chunks of
  that text (public pages the user bookmarked) are sent to the embedding
  provider. Export includes article text only behind an explicit
  `include_content` option.

### Backfill

- Migration 015 enqueues every existing bookmark (the `INSERT … SELECT`
  above). Metadata search keeps working throughout, now ranked.
- The worker drains the queue at `ARTICLE_INGEST_CONCURRENCY` with
  per-host throttling.
- Throughput, measured:
  - fetch + extract: median 0.77 s per page, so about 5 pages/s at
    concurrency 4, or **about 3–4 minutes per 1,000 bookmarks**;
  - hosted embedding (96 inputs per request) is not the bottleneck;
  - local bge-small at about 1 bookmark/s (9.6 chunks/s) is: **about 17
    minutes per 1,000 bookmarks** on 4 vCPUs, and slower on
    `shared-cpu-1x`.
- Old bookmarks suffer link rot. Expect a higher failure rate than the 33%
  measured on popular HN links.
- Changing the embedding model later means a migration for the new dimension
  plus `UPDATE bookmark_contents SET embedded_model = NULL`. The worker then
  re-embeds from the stored text, with **no refetch**. The text is kept for
  exactly this reason.

### Cost

**Per 1,000 bookmarks.** Measured basis: 2,696 embedding-input tokens and
10.3 chunks per bookmark. Storage is scaled from the prototype's 490
bookmarks.

| item | E1 hosted (te3-small, 512-d halfvec) | E2 local (bge-small, 384-d halfvec) | E4 disabled |
|---|---|---|---|
| ingest: embedding API | 2.7M tok × $0.02/M = **$0.054** (te3-large: $0.35) | $0 | $0 |
| ingest: compute | negligible | ≈17 min of CPU (4 vCPU) | negligible |
| storage: article text + tsvector + GIN | ≈27 MB | ≈27 MB | ≈27 MB |
| storage: chunks + HNSW | ≈35 MB¹ | ≈30 MB¹ | 0 |
| storage $ on MPG ($0.28/GB-mo) | ≈$0.017/mo | ≈$0.016/mo | ≈$0.008/mo |
| query | ≈12 tok → **$0.00000024**/query ($0.24 per 1M) + ~100–300 ms RTT | ≈5 ms CPU per query (measured p50 4.1–6.5 ms) | $0 |
| fixed | none | VM 512 MB → ≥1 GB (about +$5/mo on Fly) | none |

¹ Measured: the 490-bookmark prototype at 384-d float32 used 24 MB for
chunks in total, of which 10 MB was the HNSW index; the rest is the vectors, the chunk text and TOAST. `halfvec` halves the
vector part.

At 50k bookmarks across all users, the chunk index is about 1.5–1.8 GB.
That stays manageable on Starter (2 GB RAM, $72/mo). On Basic (1 GB),
queries fall back to a mostly exact scan of the *user's* rows (fine per
user), and the shared HNSW won't stay cached. Revisit the plan at that size.

**Hosted spend while prototyping:** $0.00. No hosted API was called, because no embedding key was available in this worker's environment. The hosted path is implemented and ready (`embedders.py`, which logs every call to `spend.jsonl`). A full `text-embedding-3-small` run is estimated at 1.32M corpus tokens plus about 300 query tokens, roughly **$0.03**.

---

## Errors and edge cases

- **Duplicate URL across users.** Each user's bookmark is fetched and
  embedded separately. A shared content cache keyed by
  `sha256(normalized_url)` would cut cost, but it leaks across tenants
  (timing, and article versions behind logins), so it is deferred.
- **Paywalled or logged-in content.** We only ever see the public page, so a
  paywall teaser is indexed as the article. Accepted; the snippet makes this
  visible.
- **Huge single-page apps / docs sites.** Hitting the body cap or the chunk
  cap is fine: the lexical leg keeps everything up to 200k chars.
- **Non-English pages.** The `english` FTS config stems them badly. Both
  hosted models are multilingual, so the vector leg still works. `simple`
  config detection is deferred.
- **Deleted bookmark mid-ingest.** The `FOR UPDATE` row lock plus the FK
  cascade mean the worker's store hits a missing row: treat 0 rows updated
  as done.
- **Hosted embedder outage at query time.** Fall back to lexical-only after
  1.5 s, and log at `warn` with a rate limit.

## Testing

- **Unit:**
  - chunker: windows, overlap, header, chunk cap, token cap;
  - RRF fusion order, with a pure function mirrored in SQL;
  - status transitions and retry schedule;
  - SSRF: redirect to a private IP, and a DNS name resolving to 10.x;
  - content-type and size caps, using a local axum fixture server, as the
    existing metadata tests do.
- **Repository (Postgres + pgvector, like the `idempotency-postgres` CI
  job):**
  - `search` with and without `query_vec`;
  - user isolation (user B's chunks never appear);
  - tag filter;
  - cascade delete;
  - `claim_ingest_batch` with concurrent claimers (`SKIP LOCKED`).
- **Fake embedder** for deterministic tests: hash tokens into a fixed-dim
  vector.
- **E2E (Playwright):** save a bookmark served by the fixture server, wait
  for "indexed", search for a phrase that appears only in its body, and see
  the card with a `<mark>` snippet.
- **Quality regression:** keep `scripts/search-eval` as an opt-in benchmark
  (it needs network access to rebuild the corpus). Re-run before changing
  the model, chunking or fusion, and record nDCG@10 in the PR.

## Build sequence (suggested)

1. **Infrastructure.**
   - Enable `vector` on the MPG cluster (dashboard).
   - Switch the compose, CI and E2E Postgres images to pgvector (owner
     approval needed for the CI change).
   - Migration 015.
2. **Full-text body search** with no embeddings: the `ArticleFetcher`
   adapter, worker and queue, weighted `search`, relevance sort and
   snippets. This alone is the measured 0.106 → 0.635 nDCG step, with
   `EMBEDDING_BACKEND=disabled`.
3. **`Embedder` port + OpenAI adapter**, chunks, and the hybrid query. This
   is the measured step to 0.936.
4. **Settings → Search index panel**, retry and refetch, plus the backfill
   rollout.
5. **`local-embeddings` feature** (fastembed adapter) and the self-hosting
   docs.

## Open questions

1. **Hosted vs local quality.** This is still unmeasured, because no embedding key was available. To close it, run
   `embed_variant.py --variant oai-small --embedder openai:text-embedding-3-small`, then
   `eval.py --variants oai-small=openai:text-embedding-3-small` (about $0.03), and fill the
   pending row. Published MTEB retrieval scores put `text-embedding-3-small` and
   `bge-small-en-v1.5` close together. The recommendation does not depend on the result:
   E1 is the production default for operational reasons (RAM on the 512 MB VM, no ONNX in
   the default build), and every local model already reaches 0.926–0.941 hybrid nDCG.
2. **pgvector version on MPG.** It isn't published. `iterative_scan` needs
   ≥ 0.8.0 and `halfvec` needs ≥ 0.7.0. Check with `SELECT extversion FROM
   pg_extension` once it's enabled.
3. **Re-crawl policy.** Should articles be refetched periodically (for
   example yearly) to pick up edits, or only when the user asks? v1 refetches
   only on request.
4. **Embedding model choice on the 1 GB vs 2 GB MPG plan**, once the real
   bookmark count is known. A query against production (`SELECT count(*)
   FROM bookmarks`) decides whether 512-d is needed or 1536-d fits.
