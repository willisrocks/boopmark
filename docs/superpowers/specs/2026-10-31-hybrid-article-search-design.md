# Hybrid Full-Article Search — Design

**Status:** Draft (exploration; prototype on branch `spec-hybrid-search`)
**Date:** 2026-10-31, revised 2026-11-01 after review
**Prototype + harness:** `scripts/search-eval/` (corpus manifest, seed, chunker, 48 labelled queries, Postgres eval, no-Postgres hosted eval) and `server/src/{domain/ports,adapters}/embedder.rs`

## Summary

Search the **full article text** of every bookmark, not only its title,
description and URL, with **hybrid retrieval**. Three legs are fused with
Reciprocal Rank Fusion (RRF):

1. a weighted Postgres full-text leg;
2. a `pg_trgm` leg over title + description, for prefixes and typos;
3. a pgvector leg over article chunks.

A background worker fetches and extracts the article text after the
bookmark is saved, so saving stays as fast as today. When the fetch fails,
which happens for a third of real links, the bookmark is still indexed in
both the lexical and the vector leg by its title, description, URL and tags.

The test corpus has 562 bookmarks: 342 Wikipedia pages, 148 extracted Hacker
News articles, and 72 real HN links whose fetch failed. On 48 hand-labelled
queries (keyword, natural-language, deep-in-the-body, typo, prefix, and
failed-link):

- **The recommended hybrid scores nDCG@10 0.960 with the recommended
  embedder, `text-embedding-3-large` at 1,024 dimensions**, and 0.925 with
  the free local bge-small. On the original 24 queries it scores 0.962
  (bge-small: 0.932).
- **Today's search scores 0.053** and returns nothing for 92% of the
  queries.
- **BM25 is not needed.** Every hybrid built on Postgres built-ins beats
  BM25 + vector (0.874). Without embeddings, built-in full text + `pg_trgm`
  scores 0.791, against 0.706 for BM25 alone.
- **The failed-link fallback works.** On queries about the 72 failed
  links, nDCG goes from 0.061 to 0.918. On a simulated 33% failure rate, it
  closes 68% of the gap to full text.

**Recommendation: Option A**, Postgres-native hybrid search. It needs only
`vector` (pgvector) and `pg_trgm`, and **both Fly Managed Postgres (where
production runs since 2026-10-26) and Neon offer both**, so the
recommendation holds on either provider. Neither provider offers
ParadeDB/BM25.

Embeddings go through an `Embedder` port with OpenAI, local fastembed and
disabled adapters, chosen by configuration.

- **Production: operator-paid `text-embedding-3-large`, shortened to 1,024
  dimensions** (`halfvec(1024)`). It ingests at about **$0.35 per 1,000
  bookmarks**, so the current 334 bookmarks cost about $0.12 once. It scores
  the same as the full 3,072 dimensions (0.960) with a third of the index,
  and beats `text-embedding-3-small` at any size (0.944 at 1,536).
  The [Hosted measurement](#hosted-measurement) has the numbers; the
  helper's run cost $0.2032.
- **Self-hosters get `disabled` by default:** full text + trigram (0.791),
  with no key and no extra RAM. One env var switches on OpenAI with their
  own key. A `local-embeddings` build adds the free bge-small (0.925), which
  needs a ≥1 GB VM.

## Problem

Today's search (`bookmark_repo.rs::list`) is a boolean filter over
`to_tsvector(title || description || url)` (index from migration 004) with
`plainto_tsquery`. Results come back in the user's chosen sort order
(newest first by default), not by relevance. Three things follow:

1. **Nothing below the description is searchable.** When you remember a
   bookmark by something it *said*, the search misses it.
2. **Every term must appear in the metadata** (`plainto_tsquery` ANDs every
   word), so natural-language queries almost always return zero results. Of
   the 48 evaluation queries, 44 returned nothing.
3. **No ranking.** When a query does match, the best result can sit anywhere
   in the newest-first list.

The page fetch already exists, but the article body is thrown away.
`HtmlMetadataExtractor` downloads the whole HTML page to read the `og:*` tags
and discards everything else.

## Goals

- Find a bookmark by any passage of its article, by keyword *or* by meaning,
  and tolerate prefixes and typos.
- Rank by relevance, and show why each result matched (a snippet).
- Fit the current stack: Postgres (Fly MPG today, Neon before), SQLx 0.8,
  hexagonal ports and adapters, HTMX. No new always-on service.
- Every embedding and LLM vendor sits behind a port, so it can be swapped by
  configuration.
- Keep saving a bookmark as fast as it is today; ingestion is asynchronous.
- Degrade gracefully:
  - if the embedder is unavailable, search falls back to the lexical legs;
  - if the article fetch fails, the bookmark stays searchable by its
    metadata in every leg.
- Self-hosting stays possible without any paid API.

## Non-Goals (v1)

- PDFs, YouTube transcripts, and JS-rendered SPAs (v1 ingests HTML only; these
  show up as `skipped` or `failed`, and fall back to metadata).
- Question answering / RAG over bookmarks (this design is its foundation,
  though), and LLM query rewriting. v1 search makes no LLM call.
- Cross-user search, and offline search in the mobile app or the browser
  extension.
- Re-crawling bookmarks to pick up changed articles (the text is a snapshot
  taken at save time; manual "refetch" only).

---

## Where production runs, and which extensions it allows

### Production database: Fly Managed Postgres (moved from Neon on 2026-10-26)

Evidence from the repository and git history (checked 2026-11-01):

| source | says |
|---|---|
| PR #33, "Move production from Railway to Fly.io", merged 2026-10-26 (`gh pr view 33`) | "Production now runs on Fly.io with Fly Managed Postgres". Under "Already done outside this PR": "DB moved Neon → Fly MPG (2 users, 334 bookmarks, migration 14 verified)". |
| `fly.toml` (from #33) | app `boopmark`, region `sjc`. The release workflow deploys with `flyctl deploy --image`. |
| PR #34 (`README.md`, `skills/boop/references/server-setup.md`) | Self-hosting docs: `fly mpg create` / `fly mpg attach`, and `fly mpg connect` for psql. |
| `AGENTS.md:68` (= `CLAUDE.md:68`) | "… Docker Compose, Neon (Postgres)". The file was last changed on 2026-03-22 (`git log -- AGENTS.md`), seven months before the move, so this line is stale. It dates from the 2026-03 prod-deployment design (`docs/superpowers/specs/2026-03-17-prod-deployment-design.md`, Neon + Railway). |

**What cannot be verified without prod secrets:**

- **Which host the live `DATABASE_URL` points to.** Fly secrets are
  write-only, and `fly secrets list` shows only a digest. I did not use
  flyctl or any prod credential.
- **Whether the Neon project still exists**, with or without a stale copy
  of the data. The "Boopmark - Prod - Neon" vault item suggests it was not
  deleted. If it still holds data, someone should decide whether to delete
  it.
- **The pgvector version on the MPG cluster**, and whether `vector` and
  `pg_trgm` are already enabled.

Someone with access can settle all three in a minute. Run `fly mpg list`,
then `fly mpg connect <cluster-id>`, then:

```sql
SELECT name, default_version, installed_version FROM pg_available_extensions
WHERE name IN ('vector', 'pg_trgm', 'unaccent', 'pg_search');
SELECT count(*) FROM bookmarks;   -- 334 at the move
```

Neon's hosts end in `neon.tech`, which identifies the provider from the
`DATABASE_URL` host if the secret is ever shown. AGENTS.md should say "Fly
Managed Postgres". That edit is outside this spec PR (it is the tag
worker's and the owner's file too).

### Extensions on both providers (primary docs, checked 2026-11-01)

| extension | needs `shared_preload_libraries` | Fly MPG ([docs.fly.io/mpg/extensions](https://docs.fly.io/mpg/extensions)) | Neon ([neon.com/docs/extensions/pg-extensions](https://neon.com/docs/extensions/pg-extensions)) | used by this design |
|---|---|---|---|---|
| `vector` (pgvector) | no | ✓ (one of two third-party extensions, with PostGIS); version not published | ✓ 0.8.0 on PG14–17, 0.8.6 on PG18 | **yes**: chunk vectors, `halfvec`, `iterative_scan` (needs ≥ 0.8.0) |
| `pg_trgm` | no | ✓ | ✓ 1.6 | **yes**: prefix/typo leg (`word_similarity`, GIN `gin_trgm_ops`) |
| `unaccent` | no | ✓ | ✓ 1.1 | optional (accent-folding config; small measured gain, see below) |
| `fuzzystrmatch` | no | ✓ | ✓ | no |
| `btree_gin` | no | ✓ | ✓ | no |
| `pg_search` (ParadeDB BM25) | yes | ✗ not listed; third-party support is "Vector and PostGIS for now" | ✗ "Unavailable"; the docs say the library is deprecated and "do not add it for new projects" | no |
| `pg_textsearch` (Tiger BM25), `vchord_bm25` | yes | ✗ not listed | ✗ not listed | no |
| `rum` | no | ✗ not listed | ✓ 1.3 on PG14–17 | no |

**How to enable them:**

- **Fly MPG.** Toggle each extension on the cluster's Extensions page in
  the dashboard; it is per database, not cluster-wide. The app role is not
  a superuser.
- **Neon.** Run `CREATE EXTENSION`; the default role, `neon_superuser`, may
  do this.
- **In the migration.** It starts with `CREATE EXTENSION IF NOT EXISTS
  vector; CREATE EXTENSION IF NOT EXISTS pg_trgm;`. These do nothing once
  an extension is enabled, so the same migration runs on both providers. On
  MPG, flip the toggles first.
- **Trust.** `pg_trgm`, `unaccent` and `fuzzystrmatch` are "trusted"
  extensions (their control files say so), so a database owner could create
  them. `vector` is not trusted (`vector.control` has no `trusted` line);
  that is why MPG gates it behind the toggle.
- **Version.** Clusters created before 2025-07-23 need a role-system upgrade
  before extensions can be managed
  ([announcement](https://community.fly.io/t/mpg-extensions-management-is-here/25551)).

**Plans:**

- MPG Basic: 1 GB RAM, $38/mo. Starter: 2 GB, $72/mo. Storage:
  $0.28/GB-month.
- Neon pricing was not compared; the design doesn't depend on it.

---

## Evidence: what was measured

### Environment

Docker is not available on the shared host, so `devproxy` could not run.
The prototype instead runs Postgres 16.15 from nixpkgs, with pgvector 0.8.2,
pg_trgm 1.6, unaccent 1.1 and ParadeDB pg_search 0.23.0, on its own port
(55432). The real server (`boopmark-server`, migrations 001–014) runs on
port 4100 against that database, with one seeded user. The app shows the
seeded bookmarks, and its live search reproduces the "today" numbers below.
For example, `/bookmarks?search=how do bees tell other bees where the flowers are`
returns 0 cards, while `search=bees` returns 15.

### Corpus (`fetch_corpus.py`, pinned by `corpus_manifest.jsonl`)

| source | docs | article chars median / p90 / max | notes |
|---|---|---|---|
| Wikipedia, 20 categories × ≤18 pages | 342 | 5.6k / 22k / 92k | Neighbouring pages in a category serve as hard negatives (sorting algorithms, hash functions, Icelandic volcanoes, coffee, …). |
| Hacker News stories with >900 points, extracted with trafilatura | 148 of 220 | 7.2k / 32k / 99k | Real-world ingestion: blogs, news, vendor pages, X posts. |
| the 72 HN links that failed to extract (`unscraped.jsonl`) | 72 | 0 | Metadata-only bookmarks. The HN title stands in for the title the user or the LLM enrichment would have saved; there is no description. |

Totals: **562 bookmarks, 5,052 chunks**, 1.14M article tokens (2,334 per
scraped bookmark, cl100k), 1.32M embedding-input tokens (2,696 per scraped
bookmark, including chunk overlap and headers).

`corpus_manifest.jsonl` pins every document: URL, title, description and
the sha256 of its text. `fetch_corpus.py --from-manifest` refetches exactly
those documents. A rebuild on 2026-11-01 returned 489 of 490 documents, 485
of them byte-identical.

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

There are 48 queries in six types. Each has 1–8 relevant bookmarks.

| type | n | example |
|---|---|---|
| keyword | 7 | `sscanf strlen`, `Laki 1783 eruption` |
| natural | 9 | *"why the first price I hear skews my judgement"* → Anchoring effect |
| deep (answer only in the body) | 8 | *"why a game's online mode took minutes to load and how parsing JSON was fixed"* → "How I cut GTA Online loading times by 70%" |
| typo (added in review) | 8 | `byzantine fualt tolerance`, `aeropres coffee maker`, `Grimsvotn volcano` (no accents; the page is "Grímsvötn") |
| prefix (added in review) | 6 | `garb collect`, `byzant`, `dynamo` (→ Amazon DynamoDB), `kaprek` |
| unscraped (added in review) | 10 | *"popular third-party Reddit app shutting down"* → "Apollo will close down on June 30th" (fetch failed: HTTP 403) |

Relevance labels were written first, then completed by judging the
**pool**: the union of every method's top 10 across every scenario
(TREC-style). That way no method was scored against labels only another
method could find.

- The original 24 queries were judged in two passes; the second pass added
  nothing.
- The 24 new queries were judged after the new methods ran. That added 10
  relevant documents to 6 queries. For example, `byzantine fualt tolerance`
  gained "Consensus (computer science)" and "Brooks–Iyengar algorithm", the
  same pages the original Byzantine query (q16) counts.

### Index scenarios

Every method runs in four scenarios (`common.scenarios`):

| scenario | failed links (72) | scraped articles (490) |
|---|---|---|
| `main/no-fallback` | not indexed | full text |
| `main/fallback` (**the design**) | metadata only, in both legs | full text |
| `sim/no-fallback` | not indexed | a fixed random 33% (162) treated as failed and not indexed |
| `sim/fallback` | metadata only | those 162 metadata only |

`main/no-fallback` on the original 24 queries is exactly the previous
corpus, and it reproduces every number of the first draft. The Python RRF
equals the production SQL (below) on 48 of 48 queries.

### Results (`eval.py --bm25`, exact KNN, scenario `main/fallback`)

nDCG@10 unless marked. The last column is the original 24 queries on the
original corpus, comparable with the first draft.

| method | nDCG (48) | MRR | R@10 | zero | keyword | natural | deep | typo | prefix | unscraped | orig. 24 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| **today** (metadata filter, newest first) | 0.053 | 0.083 | 0.045 | 92% | 0.222 | 0.111 | 0.000 | 0.000 | 0.000 | 0.000 | 0.106 |
| *Postgres full text (built-in)* | | | | | | | | | | | |
| fts AND (`websearch_to_tsquery`), `ts_rank` weighted | 0.353 | 0.385 | 0.354 | 54% | 0.972 | 0.499 | 0.500 | 0.125 | 0.105 | 0.000 | 0.637 |
| fts AND, `ts_rank` unweighted (`'{1,1,1,1}'`) | 0.327 | 0.340 | 0.354 | 54% | 0.870 | 0.439 | 0.500 | 0.125 | 0.105 | 0.000 | 0.585 |
| fts AND, `ts_rank_cd` weighted | 0.352 | 0.385 | 0.354 | 54% | 0.976 | 0.490 | 0.500 | 0.125 | 0.105 | 0.000 | 0.635 |
| fts OR, `ts_rank` weighted | 0.656 | 0.628 | 0.774 | 8% | 0.870 | 0.507 | 0.547 | 0.799 | 0.239 | 0.861 | 0.722 |
| fts AND, falling back to OR when AND is empty | 0.706 | 0.743 | 0.734 | 8% | 0.972 | 0.610 | 0.766 | 0.799 | 0.239 | 0.761 | 0.789 |
| fts AND, every term as prefix (`term:*`) | 0.447 | 0.490 | 0.458 | 42% | 0.957 | 0.378 | 0.500 | 0.125 | 0.957 | 0.061 | 0.587 |
| fts AND, `unaccent` text-search config | 0.373 | 0.406 | 0.375 | 52% | 0.972 | 0.499 | 0.500 | 0.250 | 0.105 | 0.000 | 0.637 |
| fts AND+ (unaccent; unknown words → `word:* \| nearest known word` via pg_trgm) | 0.531 | 0.594 | 0.531 | 33% | 0.948 | 0.445 | 0.500 | 0.625 | 0.873 | 0.061 | 0.610 |
| *pg_trgm* | | | | | | | | | | | |
| `word_similarity`, title + description | 0.295 | 0.354 | 0.283 | 62% | 0.444 | 0.068 | 0.000 | 0.622 | 0.913 | 0.000 | 0.155 |
| `word_similarity`, title + description + body | 0.380 | 0.403 | 0.424 | 54% | 0.784 | 0.207 | 0.000 | 0.713 | 0.863 | 0.000 | 0.307 |
| *BM25 (not on Fly MPG or Neon)* | | | | | | | | | | | |
| BM25 (ParadeDB pg_search, title ×3, description ×1.5) | 0.706 | 0.697 | 0.785 | 8% | 0.984 | 0.667 | 0.938 | 0.865 | 0.105 | 0.595 | 0.850 |
| *Lexical-only fusions (`EMBEDDING_BACKEND=disabled`)* | | | | | | | | | | | |
| fts AND + trgm title+description | 0.550 | 0.625 | 0.536 | 31% | 0.976 | 0.567 | 0.500 | 0.622 | 0.913 | 0.000 | 0.664 |
| **fts AND→OR + trgm title+description** ⭐ disabled mode | **0.791** | 0.837 | 0.812 | 0% | 0.976 | 0.610 | 0.766 | 0.797 | 0.919 | 0.761 | 0.790 |
| fts AND+ + trgm title+description | 0.578 | 0.656 | 0.562 | 27% | 0.976 | 0.503 | 0.500 | 0.750 | 0.960 | 0.061 | 0.640 |
| *Vector (bge-small-en-v1.5, 384-d)* | | | | | | | | | | | |
| vector, article chunks (+ metadata chunk for failed links) | 0.923 | 0.953 | 0.944 | 0% | 0.960 | 0.814 | 1.000 | 0.931 | 0.820 | 0.992 | 0.918 |
| vector, metadata only (1 per bookmark) | 0.855 | 0.907 | 0.878 | 0% | 0.713 | 0.754 | 0.954 | 0.894 | 0.808 | 0.961 | 0.813 |
| *Hybrids (RRF k=60, top 50 per leg) with the chunk vector leg* | | | | | | | | | | | |
| fts AND weighted + vector (first draft's pick) | 0.917 | 0.948 | 0.944 | 0% | 0.990 | 0.838 | 1.000 | 0.931 | 0.820 | 0.918 | **0.936** |
| fts AND unweighted + vector | 0.901 | 0.927 | 0.944 | 0% | 0.933 | 0.797 | 1.000 | 0.931 | 0.820 | 0.918 | 0.904 |
| fts AND `ts_rank_cd` + vector | 0.917 | 0.948 | 0.944 | 0% | 0.990 | 0.838 | 1.000 | 0.931 | 0.820 | 0.918 | 0.936 |
| fts OR + vector | 0.880 | 0.884 | 0.949 | 0% | 0.965 | 0.771 | 0.929 | 0.890 | 0.817 | 0.910 | 0.873 |
| fts AND→OR + vector | 0.888 | 0.898 | 0.954 | 0% | 0.990 | 0.796 | 0.929 | 0.890 | 0.817 | 0.907 | 0.897 |
| fts AND prefix + vector | 0.903 | 0.915 | 0.959 | 0% | 0.975 | 0.749 | 1.000 | 0.868 | 0.930 | 0.926 | 0.898 |
| fts AND unaccent + vector | 0.917 | 0.948 | 0.944 | 0% | 0.990 | 0.838 | 1.000 | 0.931 | 0.820 | 0.918 | 0.936 |
| fts AND+ + vector | 0.915 | 0.936 | 0.959 | 0% | 0.984 | 0.808 | 1.000 | 0.868 | 0.928 | 0.926 | 0.923 |
| trgm title+description + vector | 0.931 | 0.953 | 0.954 | 0% | 0.960 | 0.796 | 1.000 | 0.931 | 0.904 | 0.992 | 0.912 |
| trgm title+description+body + vector (1 s per query, rejected) | 0.947 | 0.950 | 0.980 | 0% | 0.975 | 0.796 | 1.000 | 0.992 | 0.936 | 0.992 | 0.916 |
| BM25 + vector (not on MPG/Neon) | 0.874 | 0.866 | 0.944 | 0% | 0.981 | 0.774 | 1.000 | 0.896 | 0.743 | 0.849 | 0.910 |
| **fts AND weighted + trgm title+description + vector** ⭐ | **0.926** | 0.948 | 0.954 | 0% | **0.990** | 0.828 | 1.000 | 0.931 | 0.904 | 0.918 | 0.932 |
| fts AND+ + trgm title+description + vector | 0.916 | 0.929 | 0.959 | 0% | 0.990 | 0.797 | 1.000 | 0.868 | 0.943 | 0.926 | 0.921 |

**Latency** (production-shaped single statements, warm, local, 562
bookmarks; `eval.py` prints these):

| statement | p50 ms | p95 ms |
|---|---|---|
| fts AND on `search` (GIN) | 0.3 | 0.6 |
| fts AND, unaccent config (GIN) | 0.4 | 0.9 |
| fts AND+ rewrite: lexicon lookups, per query | 1.7 | 3.2 |
| trgm `word_similarity` on title + description (GIN trgm) | 0.6 | 3.7 |
| trgm `word_similarity` on title + description + body (GIN trgm) | **1,023** | 1,292 |
| vector, exact scan of the user's chunks | 13.0 | 15.7 |
| hybrid SQL (fts AND + vector) | 12.7 | 17.0 |

Query embedding is extra:

- locally with bge-small: **6.5 ms p50 / 10.3 ms p95** (warm, one query at
  a time; the Python process held 274 MB with ONNX Runtime and the model
  loaded);
- with a hosted model: one HTTPS round trip, measured by the hosted harness.

With `hnsw.iterative_scan = relaxed_order` and `ef_search = 100`, the HNSW
index gave the same rankings and latency at this size.

**Embedding variants** (first draft, original 24 queries and corpus,
`embed_variant.py`, same fts AND lexical leg):

| variant | dims | embed speed (4 shared vCPU) | vector nDCG@10 | hybrid nDCG@10 |
|---|---|---|---|---|
| **bge-small-en-v1.5, article chunks** (prototype default) | 384 | 9.6 chunks/s | 0.918 | 0.936 |
| bge-small-en-v1.5, metadata only | 384 | 39 docs/s | 0.813 | 0.915 |
| all-MiniLM-L6-v2, article chunks | 384 | 35 chunks/s | 0.919 | 0.926 |
| bge-base-en-v1.5, article chunks | 768 | 3.2 chunks/s | 0.905 | 0.941 |
| OpenAI text-embedding-3-small (1536) / -large (1024), original 24 | 1536 / 1024 | hosted | 0.920 / 0.965 | 0.940 / 0.967 |

The hosted rows come from the helper's run on the rebuilt corpus. All 48
queries, every truncation and the three-leg hybrid are in
[Hosted measurement](#hosted-measurement).

### Do we need BM25? No.

The question was whether Postgres built-ins plus `pg_trgm` are enough. They
are:

1. **Next to vectors, BM25 makes the result worse, not better.** BM25 +
   vector scores 0.874. Every hybrid built on the AND full-text leg scores
   0.901–0.926, and trgm + vector scores 0.931. BM25 and the OR query match
   on any term, so they feed the fusion a long tail of weak matches. The AND
   leg contributes only when every term matches, which is exactly when it
   is trustworthy.
2. **Without vectors** (`EMBEDDING_BACKEND=disabled`), built-ins + trgm beat
   BM25 overall, 0.791 vs 0.706.
   - BM25 is still clearly better on natural and deep queries: 0.667 vs
     0.610, and 0.938 vs 0.766. On the original 24 it scores 0.850 vs 0.790.
     Saturating term frequency and IDF matter when there is no vector leg.
   - BM25 has no answer to prefixes (0.105), and trigram has (0.919).
3. **BM25 is not available anyway.** Neither MPG nor Neon offers
   `pg_search` (Neon deprecated it), and all the BM25 extensions need
   `shared_preload_libraries`.

The built-in variants rank as follows:

- **Weighted beats unweighted.** The title (A) and description (B) weights
  are worth +0.016 in the hybrid and +0.03–0.05 alone.
- **`ts_rank` and `ts_rank_cd` are equivalent.**
- **OR alone is too loose; "AND, else OR"** gets the recall without
  flooding the fusion. It is the right lexical leg when there is no vector
  leg, and the wrong one beside a vector leg (0.888).
- **`term:*` prefixes and the AND+ rewrite** fix prefix and typo queries
  for lexical-only search. Inside the hybrid they cost natural-language
  quality, because the vector leg already absorbs typos (vector typo nDCG
  0.931; probably because the WordPiece tokenizer splits a misspelling
  into known pieces, though that was not tested).
- **`unaccent`** only matters for accent-free queries for accented names
  (1 of 48 queries). It is free at query time (same GIN), but it needs a
  custom text-search config in the migration. It is optional for v1.
- **Trigram over the article body** gives the best score of all (0.947), but
  costs about 1 s per query, even with a GIN trigram index, on only 490
  articles. Rejected. **Trigram over title + description** is cheap (0.6 ms)
  and carries the prefix and typo gains, so it becomes the third leg.

### Scrape-failure fallback: index what exists

When the article fetch fails, the bookmark is indexed by its metadata,
title + description + URL + tags, in **both** legs:

- **lexical:** the `search` vector holds only the A/B/C weights;
- **vector:** one chunk embeds `title\ndescription\nurl`, the same
  `meta_text` as `embed_variant.py --mode metadata`.

Tags are part of the design but not measured, since the corpus has none.

How much of the gap it closes (nDCG@10, `eval.py` "Fallback gap"):

| method | real failed links, 10 queries: none → fallback | simulated 33% failure, 18 queries: none → fallback (full text = ceiling) | share of the simulated gap closed |
|---|---|---|---|
| fts AND weighted | 0.000 → 0.000 | 0.051 → 0.154 (0.473) | 24% |
| vector (chunks) | 0.061 → **0.992** | 0.178 → 0.725 (0.934) | 72% |
| hybrid fts AND + vector | 0.061 → **0.918** | 0.175 → 0.688 (0.935) | **68%** |
| hybrid fts AND+ + vector | 0.061 → 0.926 | 0.175 → 0.665 (0.910) | 67% |
| *hosted embedders (same harness, rebuilt corpus)* | | | |
| 3-leg hybrid, te3-small @ 1536 | 0.061 → 0.926 | 0.174 → 0.799 (0.959) | 80% |
| 3-leg hybrid, te3-large @ 1024 | 0.061 → 0.926 | 0.188 → 0.850 (0.976) | **84%** |
| vector, te3-large @ 1024 | 0.061 → 1.000 | 0.208 → 0.857 (0.935) | 89% |

- **Without the fallback, a failed link cannot be found at all.** The
  0.061 comes from one query whose second relevant page, a New Yorker
  profile, was scraped.
- **Most of the recovery comes from the metadata vector.** The real
  failed-link queries are paraphrases ("Microsoft buying GitHub" for
  "Microsoft Is Said to Have Agreed to Acquire GitHub"), so the AND leg
  finds nothing. The metadata vector finds them.
- **What the fallback cannot recover is the body:** deep queries whose
  answer is below the description. That remaining third of the gap is the
  case for better fetching (a headless browser via the existing screenshot
  service, or the iframely/opengraph.io fallbacks), not for better ranking.

### What the numbers say

1. **Full article text is the biggest single win.** Indexing the body with
   the same Postgres FTS takes the original-24 nDCG from 0.106 to 0.637
   (AND) and 0.789 (AND, else OR). That gain needs no embeddings, so the
   `disabled` setting is still a big upgrade.
2. **Embeddings carry the natural-language, deep, typo and failed-link
   queries.**
3. **The hybrid beats either leg on keywords** (0.990 vs 0.960 for vector
   alone): exact identifiers favour the lexical leg. The trigram leg adds
   prefixes (0.820 → 0.904) at no measured cost elsewhere.
4. **The three-leg hybrid and the two-leg "trgm + vector" are close, and
   which one wins depends on the embedder.**
   - With bge-small: 0.926 vs 0.931, within noise. The three-leg version
     keeps keyword nDCG at 0.990 instead of 0.960.
   - With the hosted models, two legs score higher on all 48 queries:
     te3-small 0.949 vs 0.944, te3-large@1024 0.974 vs 0.960. The whole
     difference is in the 10 failed-link queries (1.000 vs 0.926). There,
     scraped articles whose body contains every query word outrank the
     metadata-only bookmark in the full-text leg.
   - On the original 24 queries, three legs win or tie with every
     embedder (te3-large@1024: 0.962 vs 0.958).
   - **Three legs stay the recommendation.** It is one query shape for
     every backend; on te3-small and bge-small it gains +0.03–0.07 on
     keyword queries; quotes and `-exclusion` need the full-text leg; and
     the no-embedding mode needs it anyway. Dropping the full-text leg when
     the embedder is te3-large is a measured, one-CTE option (open
     question 1).
5. **Caveats.**
   - 48 queries is still small: one query moves nDCG by about 0.02.
   - The corpus favours vectors, because its topics are distinct.
   - Wikipedia text is cleaner than extracted HTML.
   - The labels come from one judge.
   - The new query types were written after seeing the corpus, though
     before running the new methods.

---

## Proposals

### Option A — Postgres-native hybrid (pgvector + tsvector + pg_trgm + RRF) ⭐ recommended

- **Full-text leg:** a weighted `tsvector` over title (A), description (B),
  URL (C) and article body (D), stored in a new `bookmark_contents` table
  with a GIN index. The query is `websearch_to_tsquery('english', $q)`,
  ranked by `ts_rank(search, q, 1)`. Users also get quotes and `-exclusion`
  for free.
- **Trigram leg:** `word_similarity($q, meta_text)` over title +
  description, with a GIN `gin_trgm_ops` index. It catches prefixes and
  typos in the words people actually remember: the title.
- **Vector leg:** `bookmark_chunks(embedding halfvec)` with an HNSW
  cosine index. Each bookmark scores its best chunk (`min(distance)`). A
  failed fetch contributes one metadata chunk.
- **Fusion:** RRF with k=60 over the top 50 of each leg, in one SQL
  statement (below). There are no score weights to tune.
- **Without embeddings:** the full-text leg becomes "AND, else OR", fused
  with the trigram leg (measured 0.791).
- **Pros:**
  - Runs on Fly MPG and Neon today, with `vector` + `pg_trgm` only.
  - One datastore, transactional with the bookmark row, so delete is a
    cascade.
  - SQLx-friendly; no new service.
  - Measured best among the options that can run.
- **Cons:**
  - `ts_rank` is not BM25. Measured, it doesn't matter once fused.
  - The vector index adds RAM pressure on the 1 GB MPG plan at scale (see
    Cost).
- **Variant A2: one vector per bookmark.** Embed only the metadata, and keep
  the full article in the lexical leg. It scores 0.855 for the vector alone
  vs 0.923, with about 10× fewer vectors. Keep it in reserve for very large
  libraries.

### Option B — Postgres + ParadeDB pg_search (BM25) + pgvector

- Real BM25 (Tantivy) in Postgres, the best lexical-only ranker on natural
  and deep queries.
- **Available on neither Fly MPG nor Neon.** Neon marks it unavailable and
  deprecated. It would need one of:
  - self-managed Postgres on a Fly Machine with a volume, giving up MPG's HA,
    backups and pooling;
  - ParadeDB Cloud as a logical-replication read target (eventually
    consistent, an extra bill, and data leaves Fly).
- The licence is AGPL-3.0.
- Measured: fused with vectors, it is *worse* than Option A (0.874 vs
  0.926).
- **Verdict:** reject. If a provider ever offers it, the full-text leg is
  one CTE behind the same repository method.

### Option C — External search index (Meilisearch / Typesense / Qdrant / Elasticsearch)

- Built-in hybrid ranking and typo tolerance; scales independently.
- Costs:
  - another always-on Fly Machine plus a volume (about $5–30/mo);
  - dual writes with an outbox to keep it consistent;
  - deletions and user scoping enforced in two places;
  - backups and an upgrade path for a second stateful service;
  - self-hosters must run it too.
- Not measured. Option A already covers typos via `pg_trgm` and the vector
  leg. At this app's scale (thousands, not hundreds of millions, of
  chunks), Option C brings operational cost with no measured quality gain.
- **Verdict:** reject. Revisit only if per-query latency at production
  scale exceeds about 200 ms.

### Embedding model — sub-options

| option | quality (this eval) | ingest cost / 1k bookmarks | query cost | ops | verdict |
|---|---|---|---|---|---|
| **E1. Operator-paid hosted, `text-embedding-3-large` @ 1024** | vector 0.954 / hybrid **0.960** (3072: 0.957 / 0.960; 512: 0.942 / 0.954) | 2.7M tok × $0.13/M = **$0.35** | ~12 tok/query: $1.56 per 1M queries; 286 ms p50 | API key in Fly secrets; network call per query | ⭐ **production default** |
| E1'. Hosted, `text-embedding-3-small` | 1536: 0.935 / 0.944; 512: 0.903 / 0.932 | $0.054 | $0.24 per 1M queries; 187 ms p50 | same | Cheaper, measurably worse: rejected |
| E1''. Hosted, Voyage `voyage-4-lite` | not measured | $0.054 | same as E1 | same; another adapter behind the same port | Equivalent alternative |
| **E2. Local fastembed (`bge-small-en-v1.5`, ONNX)** | vector 0.923 / hybrid 0.926 | $0 API cost; measured **9.6 chunks/s** (≈1 bookmark/s) on 4 shared vCPUs | ≈5–25 ms CPU (25 ms p50 on the helper's Mac) | about 130 MB model + ONNX Runtime; does **not** fit next to the app in the current 512 MB `shared-cpu-1x` VM; needs ≥1 GB | ⭐ **self-host option** |
| E3. Per-user BYO key (like `user_llm_settings`) | — | user pays | user pays; needs the user's key on every search | Anthropic, the default text provider, has no embeddings API, so most users get nothing. Index coverage differs per user. A key rotation or removal breaks query embedding, and a model switch means a full re-embed per user. | reject for v1 |
| E4. `disabled` (no vectors) | lexical-only fusion 0.791 | $0 | $0 | none | ⭐ fallback / default when unconfigured |

Why **operator-paid** over BYO, even though BYO matches today's LLM settings:

- **Embeddings cost very little.** Ingesting 1k bookmarks costs about $0.35
  with te3-large, and a heavy user's searches cost fractions of a cent per
  month.
- **The index must be one consistent vector space.** That is also why the
  `Embedder` port carries no per-request API key, unlike `LlmClient`.

Self-hosters choose the backend the same way they choose `STORAGE_BACKEND`,
`SCREENSHOT_BACKEND` or `METADATA_FALLBACK_BACKEND`.

#### Production embedder: `text-embedding-3-large` at 1,024 dimensions

Measured on 48 queries (`results/hosted-2026-11-01.json`). Storage is per
1,000 bookmarks (10.3 chunks each), scaled from the prototype's measured
sizes (see Cost).

| embedder | 3-leg nDCG | vector alone | ingest $/1k bookmarks | query p50¹ | chunks + HNSW, `halfvec` | HNSW alone | fixed cost |
|---|---|---|---|---|---|---|---|
| bge-small, local (384) | 0.925 | 0.923 | $0 | 25 ms | ≈30 MB | ≈13 MB | VM ≥1 GB: ≈+$5/mo; ONNX in the image |
| te3-small @ 512 | 0.932 | 0.903 | $0.054 | 187 ms | ≈35 MB | ≈15 MB | none |
| te3-small @ 1536 | 0.944 | 0.935 | $0.054 | 187 ms | ≈75 MB | ≈36 MB | none |
| te3-large @ 512 | 0.954 | 0.942 | $0.35 | 286 ms | ≈35 MB | ≈15 MB | none |
| **te3-large @ 1024** ⭐ | **0.960** | 0.954 | $0.35 | 286 ms | ≈55 MB | ≈26 MB | none |
| te3-large @ 3072 | 0.960 | 0.957 | $0.35 | 286 ms | ≈135 MB | ≈68 MB | none |

¹ Query-embedding round trip from the helper's Mac, 10 queries. From Fly to
OpenAI it will differ; it is not measured.

Why this one:

- **Quality.** te3-large leads at every size. At 1,024 it equals 3,072 on
  the hybrid and is ahead of te3-small@1536 by 0.016 (+0.030 vector alone),
  with the gains in natural-language (0.904 vs 0.839) and prefix queries
  (0.961 vs 0.939). The gap from 1,024 to 512 is 0.006, under one query.
- **Cost.** The price difference is real in ratio (6.5×) and irrelevant in
  dollars: about $0.12 to backfill production once, then $0.35 per
  1,000 new bookmarks. Local bge-small saves that, but needs a bigger VM,
  which costs more per month than te3-large costs per year at this
  library size.
- **Index size.** At production's 334 bookmarks, the chunks and index take
  about 18 MB. The 1 GB MPG plan holds the HNSW for roughly 20k
  bookmarks at 1,024 dimensions. Past that, re-embed at 512 from the
  stored text (no refetch, about $0.35 per 1,000) or move to Starter.
- **Latency.** te3-large adds about 100 ms per search over te3-small. The
  search box is debounced and the 1.5 s query timeout falls back to the
  lexical legs, so the risk is bounded. If it bothers users, te3-large@512
  should not help (the latency is the model, not the size); te3-small@1536
  is the fallback choice, at −0.016.

**Self-hosters by default:** `EMBEDDING_BACKEND=disabled`, the full-text +
trigram search (0.791, against 0.053 today), with no key, no extra RAM and
no ONNX. `EMBEDDING_BACKEND=openai` with their own key gives the production
setup (te3-large@1024 is the default model and size). A `local-embeddings`
build with `EMBEDDING_BACKEND=fastembed` gives bge-small for free (0.925),
on a VM with ≥1 GB.

---

## Design (Option A + E1/E2/E4)

### Configuration

```
EMBEDDING_BACKEND=disabled | openai | fastembed     # default: disabled
EMBEDDING_MODEL=text-embedding-3-large              # openai default; fastembed default: BAAI/bge-small-en-v1.5
EMBEDDING_DIMENSIONS=1024                           # openai default for te3-large; must match an HNSW index (see Schema)
OPENAI_EMBEDDING_API_KEY=...                        # operator key; separate from users' BYO keys
ORT_DYLIB_PATH=/usr/lib/libonnxruntime.so           # fastembed only
ARTICLE_INGEST_CONCURRENCY=4
ARTICLE_INGEST_ENABLED=1                            # 0 = metadata-only (today's behaviour + ranking)
```

`fastembed` sits behind a cargo feature, `local-embeddings`, so the default
build and binary don't carry ONNX Runtime. The prototype enables fastembed
7.5 with `ort-load-dynamic` (ONNX Runtime is loaded at run time from
`ORT_DYLIB_PATH`, so the build downloads no native binaries) and
`hf-hub-rustls-tls` (the model download uses rustls, like the rest of the
app).

### Schema (migration `015_bookmark_content_search.sql`)

The prototype version is in `scripts/search-eval/sql/prototype_schema.sql`,
plus `eval.py`'s setup for the trigram column.

```sql
CREATE EXTENSION IF NOT EXISTS vector;   -- Fly MPG: enable in the dashboard first
CREATE EXTENSION IF NOT EXISTS pg_trgm;  -- (Neon: these statements enable them)

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
    search      tsvector,          -- A title, B description, C url + tags, D body
    meta_text   TEXT NOT NULL DEFAULT '',  -- title || ' ' || description, for the trigram leg
    embedded_model TEXT,           -- Embedder::model_id() the chunks were built with
    fetched_at  TIMESTAMPTZ,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_bookmark_contents_user ON bookmark_contents(user_id);
CREATE INDEX idx_bookmark_contents_search ON bookmark_contents USING GIN(search);
CREATE INDEX idx_bookmark_contents_meta_trgm ON bookmark_contents USING GIN(meta_text gin_trgm_ops);
CREATE INDEX idx_bookmark_contents_queue ON bookmark_contents(next_attempt_at)
    WHERE status = 'pending';

CREATE TABLE bookmark_chunks (
    bookmark_id UUID NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE,
    user_id     UUID NOT NULL,
    ord         INT  NOT NULL,     -- a failed fetch has one chunk, ord 0, made of the metadata
    text        TEXT NOT NULL,     -- the passage, shown as a snippet
    embedding   halfvec NOT NULL,  -- no fixed dimension: one schema for every backend
    PRIMARY KEY (bookmark_id, ord)
);
CREATE INDEX idx_bookmark_chunks_user ON bookmark_chunks(user_id);
-- One partial expression index per supported default size: te3-large@1024
-- (production, and the openai default) and bge-small@384 (fastembed).
CREATE INDEX idx_bookmark_chunks_hnsw_1024 ON bookmark_chunks
    USING hnsw ((embedding::halfvec(1024)) halfvec_cosine_ops)
    WHERE vector_dims(embedding) = 1024;
CREATE INDEX idx_bookmark_chunks_hnsw_384 ON bookmark_chunks
    USING hnsw ((embedding::halfvec(384)) halfvec_cosine_ops)
    WHERE vector_dims(embedding) = 384;

-- Backfill queue: every existing bookmark starts pending, already searchable
-- by its metadata in the lexical legs.
INSERT INTO bookmark_contents (bookmark_id, user_id, search, meta_text)
SELECT id, user_id,
       setweight(to_tsvector('english', coalesce(title, '')), 'A') ||
       setweight(to_tsvector('english', coalesce(description, '')), 'B') ||
       setweight(to_tsvector('english', url || ' ' || array_to_string(tags, ' ')), 'C'),
       coalesce(title, '') || ' ' || coalesce(description, '')
FROM bookmarks;
```

- **Why a separate table.** The `bookmarks` row stays narrow; the list page
  never reads the article text. Delete cascades. A failed fetch keeps its
  metadata-only `search`, `meta_text` and metadata chunk.
- **Why `search` and `meta_text` are maintained by the app**, not generated
  columns: they combine `bookmarks` fields with the body, and a generated
  column can't read another table. The repository rebuilds them whenever
  the title, description, URL, tags or text change, in the same transaction
  as the bookmark update. (`eval.py` stores `unaccent(meta_text)`; that is
  optional.)
- **Why `halfvec` at 1,024 dimensions.** `text-embedding-3-large` supports
  shortened Matryoshka output (`dimensions: 1024`), measured equal to the
  full 3,072 on the hybrid (0.960). With half precision (pgvector ≥ 0.7)
  that is 2 KB per chunk instead of 12 KB at 3,072-d float32. See
  [Production embedder](#production-embedder-text-embedding-3-large-at-1024-dimensions).
- **Why no fixed dimension in the column.** A self-hoster on bge-small
  (384) and production on te3-large (1,024) run the same migration.
  pgvector indexes an untyped column through a partial expression index
  per size (pgvector README, "Can I store vectors with different
  dimensions in the same column?"). The vector leg casts to the configured
  size and filters on it (Search query), so it uses the matching index.
  - Any other size still works, as an exact scan of the user's chunks
    (13 ms for 5k chunks, measured). A deployment that wants an index for
    it adds one more partial index.
  - **Changing models is a re-embed backfill, with no schema change.**
    `embedded_model` (`Embedder::model_id()`, which includes the dimension)
    lets the worker find stale rows; the stored text means no refetch.
- **Self-hosting / dev / CI** need a Postgres with pgvector:
  - `docker-compose.yml`: `postgres:16-alpine` → `pgvector/pgvector:pg16`
    (it includes contrib, so `pg_trgm` too);
  - the CI `postgres:16` service → `pgvector/pgvector:pg16`;
  - `scripts/e2e/start-server.sh` reuses the compose `db`, so the compose
    change covers E2E;
  - the self-hosting docs: `fly mpg` toggles for `vector` and `pg_trgm`.

  A CI change is an implementation step for the owner, not part of this
  spec PR.

### Ports and adapters

**Every embedding and LLM call sits behind a port with vendor adapters,
chosen by configuration.** v1 search makes no LLM call. Any later LLM use
in search, such as query rewriting or answers over bookmarks, goes through
the shared `LlmClient` port defined by the tag-management spec
(`server/src/domain/ports/llm_client.rs`, branch `spec-tag-management`):

```rust
// Owned by the tag-management spec; unchanged here.
pub trait LlmClient: Send + Sync {
    fn complete_structured(&self, req: StructuredRequest)
        -> BoxFuture<'_, Result<StructuredResponse, DomainError>>;
}
// StructuredRequest { provider: TextProvider, api_key, model, system, user, json_schema, max_output_tokens }
// StructuredResponse { json: serde_json::Value, usage: TokenUsage { input_tokens, output_tokens } }
```

**`Embedder` port** (implemented in this branch:
`server/src/domain/ports/embedder.rs`):

```rust
/// Same alias as ports::llm_client::BoxFuture; import that one once both are on main.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedKind { Document, Query }

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Embeddings {
    pub vectors: Vec<Vec<f32>>,   // one per input, input order, dims() long, L2-normalised
    pub usage_tokens: u32,        // billable tokens from a hosted vendor; 0 for local models
}

pub trait Embedder: Send + Sync {
    fn model_id(&self) -> &str;   // "<backend>:<model>@<dims>", e.g. "openai:text-embedding-3-large@1024"
    fn dims(&self) -> usize;      // 0 when disabled
    fn enabled(&self) -> bool { true }
    fn embed(&self, inputs: Vec<String>, kind: EmbedKind)
        -> BoxFuture<'_, Result<Embeddings, DomainError>>;
}
```

**Contract:**

- **Batching.** Callers pass any number of inputs; the worker sends one
  bookmark's chunks, at most 64. Adapters split them into vendor-sized
  requests (OpenAI: 256 inputs or 400k characters per request, against API
  limits of 2,048 inputs and 300k tokens) and keep the order.
- **`EmbedKind`** selects the query or document form (the BGE query prefix,
  Voyage's `input_type`) and the timeout: 1.5 s for a query, which is on the
  request path, and 30 s for documents.
- **Errors use `DomainError`, like `LlmClient`:**
  - `InvalidInput` means the vendor will never accept the request (an empty
    input, an unknown model, HTTP 400, or the backend is disabled). The
    caller must not retry.
  - `Internal` covers everything else: network, timeout, 429, 5xx, or a
    rejected key. The ingest worker retries with backoff; search drops the
    vector leg and logs at `warn`.
  - Error messages carry the HTTP status, never the response body.

**What changed from the interface in the review brief:**

1. **Added `fn enabled(&self) -> bool { true }`, with a default.** The
   disabled backend returns false, so search skips the vector leg without
   calling `embed` and matching on an error. Existing adapters need no
   code for it.
2. **Defined the `model_id()` format** as `<backend>:<model>@<dims>`. The
   signature is unchanged. Two vector spaces are comparable only if
   backend, model *and* dimension match, and the string is stored per
   bookmark.
3. **`BoxFuture` is defined locally**, with the same definition as
   `llm_client.rs`, until both land; then the local one becomes a
   re-export. The `futures` crate is not added.
4. **No `api_key` in the request.** This is deliberate, unlike
   `StructuredRequest`: the key is the operator's and lives in the adapter
   (E1 over E3, above).

**Adapters** (`server/src/adapters/embedder.rs`, mirroring
`adapters/llm_client.rs`):

- **`OpenAiEmbedder`.** `/v1/embeddings` with `dimensions` (only for
  `text-embedding-3-*`, and only when shortened). It validates the count
  and length of every returned vector and re-normalises.
- **`FastembedEmbedder`**, behind `#[cfg(feature = "local-embeddings")]`.
  It runs on `spawn_blocking` behind a `Mutex`, because fastembed 7's
  `embed` takes `&mut self`. It accepts the upstream model names
  (`BAAI/bge-small-en-v1.5`), and adds the BGE query prefix for `Query`.
- **`DisabledEmbedder`.** `enabled() == false`, `dims() == 0`, and `embed`
  returns `InvalidInput`.
- **`EmbedderRouter::from_settings(&EmbeddingSettings::from_env())`.** It
  builds the backend named by `EMBEDDING_BACKEND` and delegates to it. It
  refuses `openai` without a key, and `fastembed` in a build without the
  feature.
- **Tests:**
  - 6 unit tests: settings parsing; the router; impossible dimensions;
    batching, order, usage and the `dimensions` parameter against a local
    axum fake vendor; error mapping without leaking the body; a wrong
    vector size.
  - 1 ignored fastembed test: it downloads the model and needs
    `ORT_DYLIB_PATH`. It passed locally with ONNX Runtime 1.31.1.
  - `cargo clippy -D warnings` passes with and without the feature.
- The prototype is not wired into `main.rs`. The optional `fastembed`
  dependency adds about 670 lines to `Cargo.lock` but nothing to the
  default build.

**Other ports:**

```rust
// domain/ports/article_fetcher.rs
pub struct FetchedArticle { pub text: String, pub final_url: String, pub html_bytes: usize }
pub trait ArticleFetcher: Send + Sync {
    fn fetch(&self, url: &str) -> BoxFuture<'_, Result<FetchedArticle, DomainError>>;
}
```

- `adapters/article/readability.rs`: `HtmlArticleFetcher`, using reqwest and
  `dom_smoothie` (a Readability.js port; MIT; active, 0.19 in 2026-10).

**Repository** (a new `SearchRepository` port, to keep
`BookmarkRepository` from growing):

- `search(user_id, query, query_vec: Option<Vec<f32>>, tags, limit, offset)
  -> Vec<SearchHit { bookmark, snippet, matched: Lexical|Semantic|Both }>`
- `claim_ingest_batch(n) -> Vec<(bookmark_id, url)>`, using `FOR UPDATE SKIP
  LOCKED` and moving `next_attempt_at` forward as a lease
- `store_article(bookmark_id, text, hash, chunks: Vec<(String, Vec<f32>)>, model_id)`
  in one transaction: replace the chunks, rebuild `search`, set
  `status='fetched'`
- `mark_ingest_failed(bookmark_id, error, retry_at: Option<DateTime>,
  meta_chunk: Option<(String, Vec<f32>)>)`: on the final failure, store the
  metadata chunk

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
   `bookmark_contents` row (`status='pending'`, metadata-only `search` and
   `meta_text`) in the same transaction as the bookmark. A URL change on
   `update` resets it to `pending`; a title, description or tag edit only
   rebuilds `search`, `meta_text` and, for failed fetches, the metadata
   chunk.
2. **Worker.** `IngestWorker` is a long-lived tokio task spawned in
   `main.rs`. It would be the first one there; the image-fix and
   tag-consolidation jobs are spawned from request handlers. It runs with
   its own one-connection budget, so it can't starve the request pool
   (`max_connections(5)` today). Each loop:
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
     lexical leg still sees it, and the metadata chunk covers the vector
     leg.
   - At most 64 chunks embedded per bookmark (6 of the 490 hit the cap). The
     whole capped text stays in the lexical index.
5. **Failures.**
   - Transient errors (timeout, 429, 5xx, connection errors) are retried
     after 1 min, then 1 h, then 24 h, then marked `failed`.
   - Permanent errors (401/403/404/410, non-HTML, a Cloudflare challenge)
     are marked `failed` straight away.
   - **On `failed` or `skipped`, embed the metadata
     (`title\ndescription\nurl\ntags`) as chunk 0.** That is the measured
     fallback: it takes failed-link queries from 0.061 to 0.918.
   - If the embedder fails (provider outage), keep the text, set
     `status='fetched'` with `embedded_model = NULL`, and retry embedding
     later. Full-text search works immediately.
   - Expect **about a third of real-world links to fail**, as measured. They
     are listed in Settings → Search index with a "Retry" button.
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

### Search query (three legs)

The two-leg core is `SQL_HYBRID` in `eval.py`, which `eval.py` checks
against the Python fusion on every run. The `trg` CTE is the measured
trigram leg (`TRGM` in `eval.py`, on plain columns).

```sql
WITH lex AS (
    SELECT bookmark_id, row_number() OVER (ORDER BY ts_rank(search, q, 1) DESC, bookmark_id) AS r
    FROM bookmark_contents, websearch_to_tsquery('english', $2) q
    WHERE user_id = $1 AND search @@ q
    ORDER BY ts_rank(search, q, 1) DESC, bookmark_id LIMIT 50
),
trg AS (
    SELECT bookmark_id, row_number() OVER (ORDER BY word_similarity($2, meta_text) DESC, bookmark_id) AS r
    FROM bookmark_contents
    WHERE user_id = $1 AND $2 <% meta_text          -- pg_trgm.word_similarity_threshold, default 0.6
    ORDER BY word_similarity($2, meta_text) DESC, bookmark_id LIMIT 50
),
nn AS (
    -- 1024 = Embedder::dims(), inlined by the repository so the partial index matches
    SELECT bookmark_id, embedding::halfvec(1024) <=> $3::halfvec(1024) AS dist
    FROM bookmark_chunks
    WHERE user_id = $1 AND vector_dims(embedding) = 1024
    ORDER BY embedding::halfvec(1024) <=> $3::halfvec(1024) LIMIT 200
),
vec AS (
    SELECT bookmark_id, row_number() OVER (ORDER BY min(dist), bookmark_id) AS r
    FROM nn GROUP BY bookmark_id ORDER BY min(dist), bookmark_id LIMIT 50
)
SELECT bookmark_id, sum(1.0 / (60 + r)) AS score
FROM (SELECT * FROM lex UNION ALL SELECT * FROM trg UNION ALL SELECT * FROM vec) u
GROUP BY bookmark_id
ORDER BY score DESC, bookmark_id
LIMIT $4 OFFSET $5;
```

- **Tags filter.** Join `bookmarks` in each leg with `AND b.tags && $tags`.
  This is not measured, but it is cheap: GIN on `tags`, applied after the
  user scope.
- **No query embedding** (`enabled() == false`, a timeout over 1.5 s, or a
  provider error): drop `nn`/`vec`, and make `lex` "AND, else OR". Use the
  AND query, and if it returns no rows, rerun the CTE with the OR query
  (`replace(plainto_tsquery(...)::text, ' & ', ' | ')`). This is the
  measured 0.791 lexical-only row.
- **Index use.** With `SET LOCAL hnsw.iterative_scan = relaxed_order` and
  `hnsw.ef_search = 100`, the user filter doesn't starve the HNSW scan as
  the table grows. pgvector ≥ 0.8 is required: Neon has 0.8.0/0.8.6, and
  MPG's version is to be checked on the cluster.
- **Short and empty queries.** A one-word query still runs every leg.
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
  with no keyword overlap doesn't look like a bug. A failed-fetch bookmark
  shows no snippet.
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
  above). Metadata search keeps working throughout, now ranked and
  typo-tolerant.
- The worker drains the queue at `ARTICLE_INGEST_CONCURRENCY` with
  per-host throttling.
- Throughput, measured:
  - fetch + extract: median 0.77 s per page, so about 5 pages/s at
    concurrency 4, or **about 3–4 minutes per 1,000 bookmarks**;
  - hosted embedding is not the bottleneck;
  - local bge-small at about 1 bookmark/s (9.6 chunks/s) is: **about 17
    minutes per 1,000 bookmarks** on 4 vCPUs, and slower on
    `shared-cpu-1x`.
- Production had 334 bookmarks at the move (PR #33). The full backfill is
  minutes, and costs about $0.02 with E1.
- Old bookmarks suffer link rot. Expect a higher failure rate than the 33%
  measured on popular HN links; the metadata fallback covers them.
- Changing the embedding model later means `UPDATE bookmark_contents SET
  embedded_model = NULL` (plus a partial HNSW index if the new size has
  none). The worker then
  re-embeds from the stored text, with **no refetch**. The text is kept for
  exactly this reason.

### Cost

**Per 1,000 bookmarks.** Measured basis: 2,696 embedding-input tokens and
10.3 chunks per scraped bookmark. Storage is scaled from the prototype's 490
bookmarks.

| item | E1 hosted (te3-large, 1024-d halfvec) | E2 local (bge-small, 384-d halfvec) | E4 disabled |
|---|---|---|---|
| ingest: embedding API | 2.7M tok × $0.13/M = **$0.35** (te3-small: $0.054) | $0 | $0 |
| ingest: compute | negligible | ≈17 min of CPU (4 vCPU) | negligible |
| storage: article text + tsvector + GIN + trigram on metadata² | ≈28 MB | ≈28 MB | ≈28 MB |
| storage: chunks + HNSW | ≈55 MB¹ | ≈30 MB¹ | 0 |
| storage $ on MPG ($0.28/GB-mo) | ≈$0.023/mo | ≈$0.016/mo | ≈$0.008/mo |
| query | ≈12 tok → **$0.0000016**/query ($1.56 per 1M) + one HTTPS round trip (286 ms p50 from the helper's Mac) | ≈5 ms CPU per query (measured p50 4.1–6.5 ms) | $0 |
| fixed | none | VM 512 MB → ≥1 GB (about +$5/mo on Fly) | none |

¹ Measured: the 490-bookmark prototype at 384-d float32 used 24 MB for
chunks in total, of which 10 MB was the HNSW index; the rest is the vectors,
the chunk text and TOAST. `halfvec` halves the vector part. Other sizes
are scaled from this at 2 bytes per dimension, twice (heap and HNSW), per
chunk: about +0.04 MB per 1,000 bookmarks for each extra dimension.

² The GIN trigram index on `meta_text` measured 752 kB for 562 bookmarks,
about 1.3 MB per 1,000.

At 50k bookmarks across all users and 1,024 dimensions, chunks + HNSW are
about 2.8 GB, of which the HNSW is about 1.3 GB (at 512: 1.75 GB and
0.75 GB). That fits Starter (2 GB RAM, $72/mo). On Basic (1 GB), queries
fall back to a mostly exact scan of the *user's* rows (fine per user), and
the shared HNSW won't stay cached. Revisit the plan, or re-embed at 512, at
about 20k bookmarks.

**Hosted spend for this track: $0.2032 in total**, all of it the helper's
run on 2026-11-01 (`results/hosted-2026-11-01.json`, `spend`). This worker
made no paid call.

| model | calls | tokens | USD |
|---|---|---|---|
| `text-embedding-3-small` | 34 | 1,354,960 | $0.0271 |
| `text-embedding-3-large` | 34 | 1,354,960 | $0.1761 |
| **total** | 68 | 2,709,920 | **$0.2032** |

The truncated sizes come from the same API responses, at no extra cost.

---

## Hosted measurement

The hosted comparison runs on a helper's Mac with the real key, not in this
worker. `scripts/search-eval/README.md` section A has the exact commands.
In short:

1. `brew install uv`, then clone and check out `spec-hybrid-search`.
2. `fetch_corpus.py --from-manifest` rebuilds the pinned corpus. It is free
   and takes about 3 minutes.
3. Run `hosted_eval.py` with `OPENAI_API_KEY` in the environment. It needs
   no Postgres: it ranks with exact cosine in numpy, and fuses with the
   committed lexical rankings (`results/lexical_runs.json`). `ts_rank` and
   `word_similarity` score each document on its own, so those lists stay
   valid without the database.
4. It writes `scripts/search-eval/results/hosted-<date>.json`.

Safeguards and checks:

- **Spend guard.** Before any call, it projects the spend with tiktoken and
  aborts above `--max-usd 3`. While it runs, it stops at the cap from the
  API's reported usage. It prints only metrics and HTTP status codes.
- **Cost:** projected $0.21; **actual $0.2032** (te3-small $0.0271,
  te3-large $0.1761; see Cost). The guard did not trip (`aborted: null`).
- **What it measures:** each model at its native size and truncated (512
  for small; 1024 and 512 for large), next to the local bge-small baseline
  on the same rebuilt text.
- **Validated without paying:**
  - With the local model only, on a fresh rebuild (489 of 490 documents),
    it reproduced the Postgres numbers to within 0.002 nDCG.
  - The paid path was exercised against a fake local endpoint: the
    pre-flight abort, the missing-key exit, the runtime guard, and the
    spend logged even when the guard trips.

### Results (`results/hosted-2026-11-01.json`)

Run on 2026-11-01 on the helper's Mac. The corpus rebuilt 489 of 490
documents: 1 missing (its labels dropped), 7 edited since pinning. 48
queries, scenario `main/fallback`. Truncated sizes are the native vectors
cut and re-normalised, which is what the API's `dimensions` parameter
does. The bge-small row is the local baseline on the same rebuilt text; it
matches the Postgres run (0.923 / 0.926) to within 0.001.

nDCG@10, all 48 queries:

| embedder | vector | fts AND + vector | trgm + vector | **3-leg (recommended SQL)** | 3-leg, orig. 24 | 3-leg MRR | 3-leg R@10 |
|---|---|---|---|---|---|---|---|
| bge-small (384), local | 0.923 | 0.917 | 0.930 | **0.925** | 0.932 | 0.948 | 0.954 |
| te3-small @ 1536 | 0.935 | 0.929 | 0.949 | **0.944** | 0.935 | 0.969 | 0.962 |
| te3-small @ 512 | 0.903 | 0.902 | 0.933 | **0.932** | 0.931 | 0.958 | 0.962 |
| te3-large @ 3072 | 0.957 | 0.939 | 0.977 | **0.960** | 0.954 | 0.969 | 0.987 |
| te3-large @ 1024 ⭐ | 0.954 | 0.939 | 0.974 | **0.960** | 0.962 | 0.969 | 0.984 |
| te3-large @ 512 | 0.942 | 0.928 | 0.966 | **0.954** | 0.960 | 0.969 | 0.982 |

3-leg nDCG@10 by query type:

| embedder | keyword (7) | natural (9) | deep (8) | typo (8) | prefix (6) | unscraped (10) |
|---|---|---|---|---|---|---|
| bge-small (384) | 0.990 | 0.826 | 1.000 | 0.931 | 0.904 | 0.918 |
| te3-small @ 1536 | 0.982 | 0.839 | 1.000 | 1.000 | 0.939 | 0.926 |
| te3-small @ 512 | 0.982 | 0.829 | 1.000 | 0.964 | 0.908 | 0.926 |
| te3-large @ 3072 | 0.994 | 0.883 | 1.000 | 0.998 | 0.988 | 0.926 |
| te3-large @ 1024 | 0.994 | 0.904 | 1.000 | 0.996 | 0.961 | 0.926 |
| te3-large @ 512 | 0.990 | 0.900 | 1.000 | 0.992 | 0.931 | 0.926 |

Query-embedding latency (10 queries, p50 / max): bge-small 25 / 175 ms,
te3-small 187 / 228 ms, te3-large 286 / 357 ms. The hosted numbers include
the helper's network path to OpenAI.

What it shows:

- **te3-large is the best embedder at every size**, and 1,024 dimensions
  lose nothing on the hybrid. It is the production choice
  ([Production embedder](#production-embedder-text-embedding-3-large-at-1024-dimensions)).
- **te3-small gains less than expected over bge-small** (+0.019 on the
  hybrid). At 512 it is barely ahead of the free local model.
- **The fallback closes more of the gap with better vectors:** 84% of the
  simulated gap with te3-large@1024, against 68% with bge-small.
- **The unscraped column is flat at 0.926 for every hosted model**: it is
  set by the full-text leg ranking scraped articles above metadata-only
  bookmarks (What the numbers say, item 4).

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
  hosted models are multilingual, so the vector leg still works, and the
  trigram leg is language-agnostic. `simple` config detection is deferred.
- **Accented names typed without accents** (`Grimsvotn`). The vector and
  trigram legs find them (typo nDCG 0.931). An `unaccent` text-search
  config would make the full-text leg match too. It is optional, and it
  needs `CREATE EXTENSION unaccent` (available on both providers).
- **Deleted bookmark mid-ingest.** The `FOR UPDATE` row lock plus the FK
  cascade mean the worker's store hits a missing row: treat 0 rows updated
  as done.
- **Hosted embedder outage at query time.** Fall back to the lexical legs
  after 1.5 s, and log at `warn` with a rate limit.

## Testing

- **Unit:**
  - chunker: windows, overlap, header, chunk cap, token cap;
  - RRF fusion order, with a pure function mirrored in SQL (`common.rrf` is
    the reference);
  - status transitions, the retry schedule, and the metadata chunk on
    failure;
  - SSRF: redirect to a private IP, and a DNS name resolving to 10.x;
  - content-type and size caps, using a local axum fixture server, as the
    existing metadata tests do;
  - `Embedder` adapters: already written, see above.
- **Repository (Postgres + pgvector, like the `idempotency-postgres` CI
  job):**
  - `search` with and without `query_vec`;
  - prefix and typo queries hitting the trigram leg;
  - user isolation (user B's chunks never appear);
  - tag filter;
  - cascade delete;
  - `claim_ingest_batch` with concurrent claimers (`SKIP LOCKED`).
- **Fake embedder** for deterministic tests: hash tokens into a fixed-dim
  vector.
- **E2E (Playwright):** save a bookmark served by the fixture server, wait
  for "indexed", search for a phrase that appears only in its body, and see
  the card with a `<mark>` snippet.
- **Quality regression:** keep `scripts/search-eval` as an opt-in benchmark.
  Re-run `eval.py` before changing the model, chunking or fusion, and record
  nDCG@10 in the PR.

## Build sequence (suggested)

1. **Infrastructure.**
   - Enable `vector` and `pg_trgm` on the MPG database (dashboard toggles).
   - Run the pg_available_extensions check above.
   - Switch the compose, CI and E2E Postgres images to pgvector (owner
     approval needed for the CI change).
   - Migration 015.
2. **Full-text body search** with no embeddings: the `ArticleFetcher`
   adapter, worker and queue, weighted `search`, the trigram leg, relevance
   sort and snippets. This is the measured lexical-only step (0.053 →
   0.791), with `EMBEDDING_BACKEND=disabled`.
3. **Wire the `Embedder` port** (already on this branch) into `main.rs`,
   along with chunks, the metadata fallback chunk, and the three-leg query.
   This is the measured step to 0.960 with te3-large@1024 (0.925 with
   bge-small).
4. **Settings → Search index panel**, retry and refetch, plus the backfill
   rollout.
5. **`local-embeddings` build and docs** for self-hosters (ONNX Runtime in
   the image, `ORT_DYLIB_PATH`).

## Open questions

1. **Two legs or three with te3-large?** Measured: dropping the full-text
   leg scores 0.974 vs 0.960 on 48 queries, all of it on failed-link
   queries, and 0.958 vs 0.962 on the original 24. v1 keeps three legs (one
   query shape, quotes and `-exclusion`, keyword safety on smaller
   embedders). Revisit with production query logs; the change is one CTE.
   Weighting the full-text leg down, or letting metadata-only bookmarks
   rank on `meta_text` in it, are untested alternatives.
2. **The Neon leftovers.** Does the "Boopmark - Prod - Neon" project still
   hold data? If so, decide whether to delete it. AGENTS.md:68 should say
   Fly Managed Postgres.
3. **pgvector version on MPG.** It isn't published. `iterative_scan` needs
   ≥ 0.8.0 and `halfvec` needs ≥ 0.7.0. Check with the query above.
4. **Re-crawl policy.** Should articles be refetched periodically (for
   example yearly) to pick up edits, or only when the user asks? v1
   refetches only on request.
5. **Better fetching for the failed third.** The metadata fallback closes
   about two thirds of the gap. The rest needs the body: retry 403s and
   JS-only pages through the existing screenshot service (headless Chrome)
   or the iframely/opengraph.io fallbacks. This is not measured.
