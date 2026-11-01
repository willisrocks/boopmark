# search-eval

Prototype and benchmark for `docs/superpowers/specs/2026-10-31-hybrid-article-search-design.md`.
It compares today's search, Postgres full-text variants, pg_trgm, ParadeDB
BM25, vector search and their RRF hybrids on 48 hand-labelled queries.

There are two ways to run it:

- **A. The hosted-model comparison** (`hosted_eval.py`). It needs no
  Postgres, Docker or Rust, only `uv` and `OPENAI_API_KEY`.
- **B. The full Postgres reproduction** (`eval.py`). It produces every
  lexical, trigram and BM25 number and the lexical rankings that A reuses.

## Files

| file | purpose |
|---|---|
| `fetch_corpus.py` | Builds the corpus: 342 Wikipedia pages from 20 categories, plus Hacker News links extracted with trafilatura. `--from-manifest` refetches exactly the pinned documents. |
| `corpus_manifest.jsonl` | Pins the corpus: id, URL, title, description, and the sha256 of each text. The text itself is not committed. |
| `unscraped.jsonl` | The 72 real HN links whose fetch failed (403, JS-only, PDF, ...). They become metadata-only bookmarks. |
| `queries.jsonl` | 48 labelled queries: keyword 7, natural 9, deep 8, typo 8, prefix 6, unscraped 10. Labels were completed by judging the pooled top 10 of every method. |
| `common.py` | Ids, scenarios, RRF and metrics, shared by both harnesses. |
| `chunking.py` | Splits articles into windows of about 200 words with 30 words of overlap, adds a title header, and keeps at most 64 chunks. |
| `sql/prototype_schema.sql` | The proposed `bookmark_contents` and `bookmark_chunks` tables. This is not a migration. |
| `seed.py` | Loads the corpus and the failed links into the real `bookmarks` table, fills the prototype tables, and embeds the chunks with bge-small. |
| `embed_variant.py` | Embeds the corpus with another model or mode (`chunks`, or `metadata`, which is the fallback) into `eval_chunks`. |
| `embedders.py` | Embedding providers used by B: fastembed, OpenAI and Voyage. |
| `eval.py` | B: every leg as SQL in four index scenarios, the fusions, the fallback-gap table and latency. It writes `results/local.json` and `results/lexical_runs.json`. |
| `hosted_eval.py` | A: embeds with hosted models and the local baseline, ranks with exact cosine in numpy, and fuses with `results/lexical_runs.json`. Spend guard included. |
| `app_eval.py` | C: seeds the corpus into the app's own tables (migration 016 on) and runs the 48 queries through a running server's `/api/v1/bookmarks?search=`. It measures the search the app serves. |
| `results/` | Committed results. `results/cache/` holds local embedding caches and is gitignored. |

## A. Hosted-model comparison from a clean macOS clone

You need `uv`: `brew install uv`, or
`curl -LsSf https://astral.sh/uv/install.sh | sh`. `uv` fetches Python 3.12
and every dependency itself. Postgres, Docker, nix and cargo are not needed.

```bash
git clone https://github.com/willisrocks/boopmark.git
cd boopmark
git checkout spec-hybrid-search

# 1. Rebuild the pinned corpus. This is free: about 490 public pages, ~3 min.
uv run --python 3.12 scripts/search-eval/fetch_corpus.py \
    --out ~/search-eval-data --from-manifest scripts/search-eval/corpus_manifest.jsonl

# 2. Run the comparison. The key goes in the environment only; this prompts for it without echoing.
read -rs OPENAI_API_KEY && export OPENAI_API_KEY
uv run --python 3.12 scripts/search-eval/hosted_eval.py \
    --corpus ~/search-eval-data/corpus.jsonl \
    --models openai:text-embedding-3-small,openai:text-embedding-3-large
unset OPENAI_API_KEY

# 3. Commit the result file (metrics and spend only, no keys, no text).
git add scripts/search-eval/results/hosted-*.json
git commit -m "search-eval: hosted embedding results"
git push
```

- **Expected cost.** About 1.36M input tokens per model.
  `text-embedding-3-small` costs about $0.03 and `text-embedding-3-large`
  about $0.18, so roughly $0.21 in total.
- **Spend guard.**
  - Before the first paid call, the script projects the spend with tiktoken
    and aborts if it is over `--max-usd`, which defaults to 3.
  - During the run, it keeps a tally from the API's reported usage. Once
    the tally crosses the cap, it makes no further call; the last batch can
    overshoot the cap by a cent or so.
  - It prints only counts, metrics and HTTP status codes, never the key or a
    request or response body.
- **Time.** Step 1 takes about 3 minutes. Step 2 runs the local bge-small
  baseline on the same text first (about 5 minutes on an Apple-silicon Mac;
  the model download is about 130 MB), then the hosted calls (a few minutes).
- **Output.** `scripts/search-eval/results/hosted-<UTC date>.json` holds:
  - nDCG/MRR/recall per model and dimension (1536 and 512 for small; 3072,
    1024 and 512 for large), per scenario and per query type;
  - the fallback-gap table;
  - corpus coverage;
  - query-embedding latency;
  - spend, as `{model: {calls, tokens, usd}}`.
- **Corpus drift.** Pages edited since 2026-10-31 are kept (counted as
  `edited`). Pages that no longer load are dropped together with their
  labels (`missing`). The local baseline runs on the same rebuilt text, so
  the comparison stays like for like. On 2026-11-01 the rebuild got 489 of
  490 pages, 4 of them edited, and the baseline reproduced the Postgres
  numbers to within 0.002 nDCG.
- **Lexical runs stay pinned.** The full-text and trigram runs in
  `results/lexical_runs.json` come from the pinned 2026-10-31 text, because
  this path has no Postgres; only the vectors use the rebuilt text. So for
  edited pages the fusion mixes pinned lexical ranks with current vectors.
  Every model in a run, the local baseline included, gets the same lexical
  runs, so model-to-model comparisons are unaffected, and the baseline
  matching Postgres bounds the drift. To remove it, rebuild the runs with
  `eval.py --export-runs` (path B) on the rebuilt corpus.

## B. Full reproduction with Postgres

1. **Start Postgres 16** with pgvector and the contrib extensions
   (`pg_trgm`, `unaccent`). Add pg_search only if you want `--bm25`. Docker
   is not required. With nix:

   ```bash
   PG=$(nix build --no-link --print-out-paths --impure --expr \
     'with import <nixpkgs> {}; postgresql_16.withPackages (p: [p.pgvector p.pg_search])')/bin
   $PG/initdb -D ./pgdata -U boopmark --auth=trust
   echo "shared_preload_libraries='pg_search'
   port=55432" >> pgdata/postgresql.conf
   $PG/pg_ctl -D ./pgdata -l pgdata/log start
   $PG/createdb -h 127.0.0.1 -p 55432 -U boopmark boopmark
   ```

   With Homebrew, `brew install postgresql@16 pgvector` works instead; skip
   `--bm25` there.

2. **Run the server once** to apply migrations 001–014: set
   `DATABASE_URL=postgres://boopmark@127.0.0.1:55432/boopmark` and
   `PORT=4100`. Then create the user `eval@example.com`; hash a password
   with `cargo run -p boopmark-server --example hash_password`.

3. **Rebuild the corpus** with step A1.

4. **Seed and embed.** This embeds the 5k chunks with bge-small (about 9
   minutes on 4 vCPUs), then adds the metadata vectors for the fallback.

   ```bash
   cd scripts/search-eval
   DB=postgres://boopmark@127.0.0.1:55432/boopmark
   uv run seed.py --db $DB --corpus ~/search-eval-data/corpus.jsonl --email eval@example.com
   uv run embed_variant.py --db $DB --corpus ~/search-eval-data/corpus.jsonl --email eval@example.com \
       --variant meta-bge --embedder fastembed:BAAI/bge-small-en-v1.5 --mode metadata
   ```

5. **Evaluate.**

   ```bash
   uv run eval.py --db $DB --email eval@example.com --bm25 \
       --json results/local.json --export-runs results/lexical_runs.json --pool pool.jsonl
   ```

   It prints the method table, the fallback-gap table and latency. It also
   checks that the Python RRF equals the production SQL from the spec; this
   matched on 48 of 48 queries. To judge new hits, look at the unlabelled
   results in `pool.jsonl` and add the relevant ones to `queries.jsonl`.

## C. The app's own search, through its API

`app_eval.py` measures what the server actually serves: its migrations, its
search query and its `EMBEDDING_BACKEND`. It needs a database migrated by the
server (migration 016 or later), the server running against it, and the
rebuilt corpus from step A1. No Docker, and no pgvector for the lexical-only
search.

```bash
# Postgres 16 on 55432 as in B1 (pg_trgm is enough), then the server:
DATABASE_URL=postgres://boopmark@127.0.0.1:55432/boopmark PORT=4100 \
    EMBEDDING_BACKEND=disabled cargo run --release -p boopmark-server &

uv run scripts/search-eval/app_eval.py --db postgres://boopmark@127.0.0.1:55432/boopmark \
    --server http://127.0.0.1:4100 --corpus ~/search-eval-data/corpus.jsonl
```

- `--text corpus` (the default) stores the pinned corpus text as fetched
  articles and the 72 failed links as `failed`, the spec's `main/fallback`
  scenario. Same text as `eval.py`, so the result compares with its table.
- `--text ingest` leaves every bookmark pending and lets the server's ingest
  worker fetch and extract the live pages (`ARTICLE_INGEST_ENABLED=1`, the
  default). Wikipedia is one host, so at one request per host per second the
  run takes about six minutes.
- It creates the user if needed, replaces their bookmarks, and uses a
  temporary API key that it deletes afterwards.

On 2026-11-01, with the lexical-only search (migration 016,
`EMBEDDING_BACKEND=disabled`), `--text corpus` scored nDCG@10 **0.791**, the
same as `eval.py`'s "fts AND→OR + trgm title+description" row in every query
type.
