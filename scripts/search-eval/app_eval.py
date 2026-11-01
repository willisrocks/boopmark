# /// script
# requires-python = ">=3.11"
# dependencies = ["psycopg[binary]>=3.2", "httpx>=0.27"]
# ///
"""Measure the search the app actually serves, through its API.

Unlike eval.py, which runs every candidate leg as hand-written SQL, this
script seeds the corpus into the app's own tables (migration 016 and later)
and sends the labelled queries to a running server's
`GET /api/v1/bookmarks?search=`. It measures whatever the server's search
query, migrations and EMBEDDING_BACKEND currently are, with the same metrics
as eval.py (common.metrics), so the numbers compare with its table.

    # 1. A migrated database and a running server (any EMBEDDING_BACKEND).
    # 2. Seed and evaluate:
    uv run scripts/search-eval/app_eval.py \
        --db postgres://boopmark@127.0.0.1:55432/boopmark \
        --server http://127.0.0.1:4100 \
        --corpus ~/search-eval-data/corpus.jsonl --email eval@example.com

Seeding modes:

  --text corpus  (default) stores the pinned corpus text as fetched articles
                 and the 72 failed links as `failed`, like the ingest worker
                 would (the spec's main/fallback scenario). Same text as
                 eval.py, so the numbers are like for like.
  --text ingest  leaves every bookmark pending and waits for the server's
                 ingest worker to fetch and extract the live pages itself
                 (ARTICLE_INGEST_ENABLED=1). This measures the real fetcher,
                 extractor and failure rate too, against today's web.

The user is created if needed; their bookmarks are replaced. An API key for
the run is inserted and removed again.
"""

import argparse
import hashlib
import json
import pathlib
import secrets
import statistics
import time
import urllib.parse

import httpx
import psycopg

from common import (HERE, TEXT_CAP_CHARS, bookmark_uuid, load_jsonl, load_queries, load_unscraped, metrics,
                    print_table, summarize)

SPEC_LEXICAL_ONLY = 0.791  # spec: "fts AND->OR + trgm title+description", main/fallback


def seed(conn, user_id, docs, failed, mode: str) -> None:
    conn.execute("DELETE FROM bookmarks WHERE user_id = %s", (user_id,))
    for d in docs + failed:
        conn.execute(
            """INSERT INTO bookmarks (id, user_id, url, title, description, domain)
               VALUES (%s, %s, %s, %s, %s, %s)""",
            (bookmark_uuid(d["id"]), user_id, d["url"], d["title"], d.get("description") or None,
             urllib.parse.urlparse(d["url"]).hostname),
        )
    if mode == "ingest":
        return
    # What the ingest worker stores (SearchRepository::store_article and
    # record_ingest_failure), in the same transaction as the inserts so a
    # running worker never sees these rows pending.
    for d in docs:
        text = d["text"][:TEXT_CAP_CHARS]
        conn.execute(
            """UPDATE bookmark_contents c
               SET status = 'fetched', text = %s, text_chars = char_length(%s), content_hash = %s,
                   search = bookmark_search_document(b.title, b.description, b.url, b.tags, %s),
                   fetched_at = now(), updated_at = now()
               FROM bookmarks b WHERE c.bookmark_id = %s AND b.id = c.bookmark_id""",
            (text, text, hashlib.sha256(text.encode()).hexdigest(), text, bookmark_uuid(d["id"])),
        )
    for d in failed:
        conn.execute(
            "UPDATE bookmark_contents SET status = 'failed', error = %s WHERE bookmark_id = %s",
            (d["fetch_error"], bookmark_uuid(d["id"])),
        )


def wait_for_ingest(conn, user_id, timeout_s: int) -> dict:
    deadline = time.monotonic() + timeout_s
    while True:
        counts = dict(conn.execute(
            "SELECT status, count(*) FROM bookmark_contents WHERE user_id = %s GROUP BY status",
            (user_id,)).fetchall())
        print(f"  ingest: {counts}", flush=True)
        if not counts.get("pending") or time.monotonic() > deadline:
            return counts
        time.sleep(15)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--db", required=True)
    ap.add_argument("--server", required=True, help="base URL of the running server")
    ap.add_argument("--corpus", required=True, type=pathlib.Path)
    ap.add_argument("--email", default="eval@example.com")
    ap.add_argument("--text", choices=["corpus", "ingest"], default="corpus")
    ap.add_argument("--ingest-timeout", type=int, default=1800, help="seconds, --text ingest only")
    ap.add_argument("--queries", type=pathlib.Path, default=HERE / "queries.jsonl")
    ap.add_argument("--json", type=pathlib.Path, help="write the summary and per-query results here")
    args = ap.parse_args()

    docs = load_jsonl(args.corpus)
    failed = load_unscraped()
    queries = load_queries(args.queries)
    id_to_doc = {str(bookmark_uuid(d["id"])): d["id"] for d in docs + failed}
    # Queries whose relevant documents did not come back in a rebuilt corpus
    # are dropped, as in hosted_eval.py.
    present = set(id_to_doc.values())
    queries = [{**q, "relevant": [r for r in q["relevant"] if r in present]} for q in queries]
    queries = [q for q in queries if q["relevant"]]

    raw_key = f"boop_eval_{secrets.token_urlsafe(24)}"
    with psycopg.connect(args.db, autocommit=False) as conn:
        row = conn.execute("SELECT id FROM users WHERE email = %s", (args.email,)).fetchone()
        user_id = row[0] if row else conn.execute(
            "INSERT INTO users (email, name) VALUES (%s, 'search eval') RETURNING id",
            (args.email,)).fetchone()[0]
        key_id = conn.execute(
            "INSERT INTO api_keys (user_id, key_hash, name) VALUES (%s, %s, 'search-eval') RETURNING id",
            (user_id, hashlib.sha256(raw_key.encode()).hexdigest())).fetchone()[0]
        seed(conn, user_id, docs, failed, args.text)
        conn.commit()
        print(f"seeded {len(docs)} articles and {len(failed)} failed links for {args.email} "
              f"(--text {args.text})")
        coverage = None
        if args.text == "ingest":
            coverage = wait_for_ingest(conn, user_id, args.ingest_timeout)

        try:
            per_query, latency = {}, []
            with httpx.Client(base_url=args.server, timeout=30,
                              headers={"Authorization": f"Bearer {raw_key}"}) as http:
                for q in queries:
                    t0 = time.perf_counter()
                    resp = http.get("/api/v1/bookmarks", params={"search": q["query"], "limit": 10})
                    latency.append((time.perf_counter() - t0) * 1000)
                    resp.raise_for_status()
                    ranked = [id_to_doc[b["id"]] for b in resp.json()]
                    per_query[q["qid"]] = {**metrics(ranked, set(q["relevant"])), "top": ranked}
        finally:
            conn.execute("DELETE FROM api_keys WHERE id = %s", (key_id,))
            conn.commit()

    summary = summarize(per_query, queries)
    types = sorted({q["type"] for q in queries})
    print_table(f"app search via {args.server} ({len(queries)} queries)", {"app": summary}, types)
    latency.sort()
    print(f"\nnDCG@10 {summary['ndcg']:.3f} (spec, lexical only: {SPEC_LEXICAL_ONLY:.3f}); "
          f"API latency p50 {statistics.median(latency):.1f} ms, "
          f"p95 {latency[int(0.95 * len(latency))]:.1f} ms")
    if coverage:
        print(f"ingest coverage: {coverage}")
    if args.json:
        args.json.write_text(json.dumps({"summary": summary, "coverage": coverage,
                                         "per_query": per_query}, indent=1))


if __name__ == "__main__":
    main()
