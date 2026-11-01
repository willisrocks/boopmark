# search-eval

Prototype and benchmark for `docs/superpowers/specs/2026-10-31-hybrid-article-search-design.md`.
It compares lexical, vector and hybrid retrieval on 24 hand-labelled queries.

Everything runs locally. The only paid step is the optional hosted-model run in
step 6, and it logs what it spends.

## Files

| file | purpose |
|---|---|
| `fetch_corpus.py` | Builds the corpus: Wikipedia pages from 20 categories, plus the Hacker News links extracted with trafilatura. It also writes `ingest_log.jsonl` with the per-URL failure reasons. |
| `sql/prototype_schema.sql` | The proposed `bookmark_contents` and `bookmark_chunks` tables. This is not a real migration, so CI is unaffected. |
| `chunking.py` | Splits articles into chunks of about 200 words with 30 words of overlap, adds a title header, and keeps at most 64 chunks. |
| `seed.py` | Loads the corpus into the real `bookmarks` table for one user, fills the prototype tables, and embeds the chunks with bge-small. |
| `embed_variant.py` | Embeds the corpus with another model or mode (`chunks` or `metadata`) into `eval_chunks`. |
| `embedders.py` | The embedding providers: local fastembed, OpenAI and Voyage. The hosted ones log their spend to `spend.jsonl`. |
| `queries.jsonl` | 24 labelled queries: 7 keyword, 9 natural-language and 8 "deep", meaning the answer is only in the article body. The labels are completed by judging the pool of every method's top 10. |
| `eval.py` | Runs every method as one SQL statement, then prints recall@10, MRR@10, nDCG@10, the zero-result rate and p50 latency. |

## Reproduce

1. **Start Postgres 16** with pgvector, and with pg_search if you want `--bm25`.
   Docker is not required. With nix:

   ```bash
   PG=$(nix build --no-link --print-out-paths --impure --expr \
     'with import <nixpkgs> {}; postgresql_16.withPackages (p: [p.pgvector p.pg_search])')/bin
   $PG/initdb -D ./pgdata -U boopmark --auth=trust
   echo "shared_preload_libraries='pg_search'
   port=55432" >> pgdata/postgresql.conf
   $PG/pg_ctl -D ./pgdata -l pgdata/log start
   $PG/createdb -h 127.0.0.1 -p 55432 -U boopmark boopmark
   ```

2. **Run the server once** so it applies migrations 001–014. Use
   `DATABASE_URL=postgres://boopmark@127.0.0.1:55432/boopmark` and `PORT=4100`.
   Then create the user `eval@example.com`. You can hash a password with
   `cargo run -p boopmark-server --example hash_password`.

3. **Build the corpus.** This takes about 10 minutes and the result is not
   committed.

   ```bash
   uv run scripts/search-eval/fetch_corpus.py --out ~/search-eval-data
   ```

4. **Seed the database.** This embeds the 5k chunks with bge-small, which
   takes about 9 minutes on 4 vCPUs.

   ```bash
   cd scripts/search-eval
   uv run seed.py --db $DB --corpus ~/search-eval-data/corpus.jsonl --email eval@example.com
   ```

5. **Optional: add more embedding variants.**

   ```bash
   uv run embed_variant.py --db $DB --corpus ... --email eval@example.com \
       --variant meta-bge --embedder fastembed:BAAI/bge-small-en-v1.5 --mode metadata
   uv run embed_variant.py ... --variant minilm --embedder fastembed:sentence-transformers/all-MiniLM-L6-v2
   ```

6. **Optional: run hosted models.** These calls are paid and need
   `OPENAI_API_KEY` or `VOYAGE_API_KEY`. The corpus is about 1.32M tokens, so
   `text-embedding-3-small` costs about $0.03 and `voyage-4-lite` about the
   same.

   ```bash
   uv run embed_variant.py ... --variant oai-small --embedder openai:text-embedding-3-small
   ```

7. **Evaluate.**

   ```bash
   uv run eval.py --db $DB --email eval@example.com --bm25 \
       --variants meta-bge=fastembed:BAAI/bge-small-en-v1.5,oai-small=openai:text-embedding-3-small
   ```

Add `--pool pool.jsonl` to write the union of every method's top 10. Judge the
unlabelled hits in that file, then add the relevant ones to `queries.jsonl`.
