# /// script
# requires-python = ">=3.11"
# dependencies = ["psycopg[binary]>=3.2", "fastembed>=0.4", "numpy"]
# ///
"""Measure lexical vs vector vs hybrid retrieval on the labelled queries.

Every method is one SQL statement against the seeded database, written the
way the server would issue it (user-scoped, top 10). Prints a metrics table
(recall@10, MRR@10, nDCG@10, zero-result rate, median latency) per method and
per query type, and writes the judging pool (union of every method's top 10)
to --pool so that unlabelled hits can be judged and added to queries.jsonl.

    uv run scripts/search-eval/eval.py --db postgres://boopmark@127.0.0.1:55432/boopmark \
        --email eval@example.com [--pool pool.jsonl] [--bm25]
"""

import argparse
import collections
import json
import math
import pathlib
import statistics
import time
import uuid

import psycopg

from seed import LOCAL_MODEL, MODEL_SLUG, bookmark_uuid

K = 10
RRF_K = 60
QUERY_PREFIX = "Represent this sentence for searching relevant passages: "  # BGE convention

# Today's production query (bookmark_repo.rs list()): boolean filter on the
# metadata tsvector, no ranking, newest first.
SQL_TODAY = """
SELECT id FROM bookmarks
WHERE user_id = %(user)s
  AND to_tsvector('english', coalesce(title, '') || ' ' || coalesce(description, '') || ' ' || url)
      @@ plainto_tsquery('english', %(q)s)
ORDER BY created_at DESC LIMIT 10
"""

# Same metadata-only document, ranked.
SQL_META_RANKED = """
SELECT id FROM bookmarks,
     to_tsvector('english', coalesce(title, '') || ' ' || coalesce(description, '') || ' ' || url) doc,
     plainto_tsquery('english', %(q)s) q
WHERE user_id = %(user)s AND doc @@ q
ORDER BY ts_rank(doc, q) DESC LIMIT 10
"""

# Full article, all terms required (websearch syntax: quotes, -exclusion).
SQL_FTS_AND = """
SELECT bookmark_id FROM bookmark_contents, websearch_to_tsquery('english', %(q)s) q
WHERE user_id = %(user)s AND search @@ q
ORDER BY ts_rank_cd(search, q, 1) DESC LIMIT 10
"""

# Full article, any term (OR of the lexemes), ranked by weighted ts_rank.
# This is the lexical leg of the recommended hybrid: high recall, and the
# vector leg plus fusion decide the order.
OR_QUERY = "to_tsquery('english', replace(plainto_tsquery('english', %(q)s)::text, ' & ', ' | '))"
SQL_FTS_OR = f"""
SELECT bookmark_id FROM bookmark_contents, {OR_QUERY} q
WHERE user_id = %(user)s AND search @@ q
ORDER BY ts_rank(search, q, 1) DESC LIMIT 10
"""

# Vector only: nearest chunks, best chunk per bookmark.
SQL_VECTOR = """
WITH nn AS (
    SELECT bookmark_id, embedding <=> %(qv)s::vector AS dist
    FROM bookmark_chunks
    WHERE user_id = %(user)s AND model = %(model)s
    ORDER BY embedding <=> %(qv)s::vector
    LIMIT 200
)
SELECT bookmark_id FROM nn GROUP BY bookmark_id ORDER BY min(dist) LIMIT 10
"""

# Recommended: RRF of the OR-lexical leg (top 50) and the vector leg (top 50).
SQL_HYBRID = f"""
WITH lex AS (
    SELECT bookmark_id, row_number() OVER (ORDER BY ts_rank(search, q, 1) DESC) AS r
    FROM bookmark_contents, {OR_QUERY} q
    WHERE user_id = %(user)s AND search @@ q
    ORDER BY ts_rank(search, q, 1) DESC LIMIT 50
),
nn AS (
    SELECT bookmark_id, embedding <=> %(qv)s::vector AS dist
    FROM bookmark_chunks
    WHERE user_id = %(user)s AND model = %(model)s
    ORDER BY embedding <=> %(qv)s::vector
    LIMIT 200
),
vec AS (
    SELECT bookmark_id, row_number() OVER (ORDER BY min(dist)) AS r
    FROM nn GROUP BY bookmark_id ORDER BY min(dist) LIMIT 50
)
SELECT bookmark_id
FROM (SELECT bookmark_id, r FROM lex UNION ALL SELECT bookmark_id, r FROM vec) u
GROUP BY bookmark_id
ORDER BY sum(1.0 / ({RRF_K} + r)) DESC
LIMIT 10
"""

# Same fusion but with the AND (websearch) lexical leg.
SQL_HYBRID_AND = SQL_HYBRID.replace(OR_QUERY, "websearch_to_tsquery('english', %(q)s)")

# ParadeDB pg_search BM25 (local only: not available on Fly MPG).
SQL_BM25 = """
SELECT bookmark_id FROM eval_bm25
WHERE id @@@ paradedb.boolean(should => ARRAY[
    paradedb.boost(3.0, paradedb.match('title', %(q)s)),
    paradedb.boost(1.5, paradedb.match('description', %(q)s)),
    paradedb.match('body', %(q)s)
]) AND user_id = %(user)s
ORDER BY paradedb.score(id) DESC LIMIT 10
"""
SQL_BM25_SETUP = """
DROP TABLE IF EXISTS eval_bm25;
CREATE EXTENSION IF NOT EXISTS pg_search;
CREATE TABLE eval_bm25 AS
SELECT row_number() OVER ()::int AS id, b.id AS bookmark_id, b.user_id,
       coalesce(b.title, '') AS title, coalesce(b.description, '') AS description,
       coalesce(c.text, '') AS body
FROM bookmarks b JOIN bookmark_contents c ON c.bookmark_id = b.id;
CREATE INDEX eval_bm25_idx ON eval_bm25 USING bm25 (id, title, description, body, user_id)
    WITH (key_field = 'id');
"""
SQL_HYBRID_BM25 = f"""
WITH lex AS (
    SELECT bookmark_id, row_number() OVER (ORDER BY paradedb.score(id) DESC) AS r FROM eval_bm25
    WHERE id @@@ paradedb.boolean(should => ARRAY[
        paradedb.boost(3.0, paradedb.match('title', %(q)s)),
        paradedb.boost(1.5, paradedb.match('description', %(q)s)),
        paradedb.match('body', %(q)s)
    ]) AND user_id = %(user)s
    ORDER BY paradedb.score(id) DESC LIMIT 50
),
nn AS (
    SELECT bookmark_id, embedding <=> %(qv)s::vector AS dist
    FROM bookmark_chunks
    WHERE user_id = %(user)s AND model = %(model)s
    ORDER BY embedding <=> %(qv)s::vector
    LIMIT 200
),
vec AS (
    SELECT bookmark_id, row_number() OVER (ORDER BY min(dist)) AS r
    FROM nn GROUP BY bookmark_id ORDER BY min(dist) LIMIT 50
)
SELECT bookmark_id
FROM (SELECT bookmark_id, r FROM lex UNION ALL SELECT bookmark_id, r FROM vec) u
GROUP BY bookmark_id
ORDER BY sum(1.0 / ({RRF_K} + r)) DESC
LIMIT 10
"""


def metrics(ranked: list[uuid.UUID], relevant: set[uuid.UUID]) -> dict:
    top = ranked[:K]
    hits = [1 if d in relevant else 0 for d in top]
    rr = next((1 / (i + 1) for i, h in enumerate(hits) if h), 0.0)
    dcg = sum(h / math.log2(i + 2) for i, h in enumerate(hits))
    idcg = sum(1 / math.log2(i + 2) for i in range(min(len(relevant), K)))
    return {
        "recall": sum(hits) / len(relevant),
        "mrr": rr,
        "ndcg": dcg / idcg,
        "zero": 1.0 if not top else 0.0,
    }


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--db", required=True)
    ap.add_argument("--email", required=True)
    ap.add_argument("--queries", type=pathlib.Path,
                    default=pathlib.Path(__file__).parent / "queries.jsonl")
    ap.add_argument("--pool", type=pathlib.Path)
    ap.add_argument("--bm25", action="store_true", help="also measure ParadeDB pg_search")
    ap.add_argument("--hnsw", action="store_true",
                    help="use the HNSW index (iterative scan) instead of exact search")
    ap.add_argument("--json", type=pathlib.Path, help="write per-query results here")
    args = ap.parse_args()

    queries = [json.loads(line) for line in open(args.queries)]
    from fastembed import TextEmbedding

    model = TextEmbedding(LOCAL_MODEL)
    qvecs = list(model.embed([QUERY_PREFIX + q["query"] for q in queries]))

    methods = {
        "today (metadata filter, newest first)": SQL_TODAY,
        "metadata ts_rank": SQL_META_RANKED,
        "full-text AND ts_rank_cd": SQL_FTS_AND,
        "full-text OR ts_rank": SQL_FTS_OR,
        f"vector ({MODEL_SLUG})": SQL_VECTOR,
        "hybrid RRF (full-text AND + vector)": SQL_HYBRID_AND,
        "hybrid RRF (full-text OR + vector)": SQL_HYBRID,
    }

    with psycopg.connect(args.db, autocommit=True) as conn:
        user = conn.execute("SELECT id FROM users WHERE email = %s", (args.email,)).fetchone()[0]
        if args.bm25:
            conn.execute(SQL_BM25_SETUP)
            methods["BM25 (pg_search)"] = SQL_BM25
            methods["hybrid RRF (BM25 + vector)"] = SQL_HYBRID_BM25
        if args.hnsw:
            conn.execute("SET hnsw.ef_search = 100")
            conn.execute("SET hnsw.iterative_scan = relaxed_order")
        else:
            # Exact KNN (no HNSW index scan): the correctness baseline. GIN and
            # bm25 still run as bitmap/custom scans.
            conn.execute("SET enable_indexscan = off")

        id_to_doc = {bookmark_uuid(i): i for i in range(5000)}
        titles = dict(conn.execute("SELECT id, title FROM bookmarks WHERE user_id = %s", (user,)).fetchall())
        results = collections.defaultdict(dict)
        latency = collections.defaultdict(list)
        pool = collections.defaultdict(set)

        for q, qv in zip(queries, qvecs):
            params = {"user": user, "q": q["query"], "model": MODEL_SLUG,
                      "qv": "[" + ",".join(f"{x:.6f}" for x in qv) + "]"}
            rel = {bookmark_uuid(i) for i in q["relevant"]}
            for name, sql in methods.items():
                for _ in range(2):  # warm once, time the second run
                    t0 = time.perf_counter()
                    ranked = [r[0] for r in conn.execute(sql, params).fetchall()]
                    ms = (time.perf_counter() - t0) * 1000
                latency[name].append(ms)
                results[name][q["qid"]] = {**metrics(ranked, rel),
                                           "top": [id_to_doc[r] for r in ranked]}
                pool[q["qid"]].update(ranked)

    types = sorted({q["type"] for q in queries})
    hdr = "| method | R@10 | MRR@10 | nDCG@10 | zero-result | " + " | ".join(
        f"nDCG {t}" for t in types) + " | p50 ms |"
    print(hdr)
    print("|" + "---|" * (hdr.count("|") - 1))
    for name in methods:
        rows = results[name]
        avg = lambda key, qs: statistics.mean(rows[q["qid"]][key] for q in qs)  # noqa: E731
        by_type = [avg("ndcg", [q for q in queries if q["type"] == t]) for t in types]
        print(f"| {name} | {avg('recall', queries):.3f} | {avg('mrr', queries):.3f} | "
              f"{avg('ndcg', queries):.3f} | {avg('zero', queries):.0%} | "
              + " | ".join(f"{v:.3f}" for v in by_type)
              + f" | {statistics.median(latency[name]):.1f} |")

    print("\nper-query nDCG@10:")
    print("qid  " + "  ".join(f"m{i}" for i in range(len(methods))))
    for q in queries:
        print(q["qid"], "  ".join(f"{results[n][q['qid']]['ndcg']:.2f}" for n in methods), q["query"])

    if args.pool:
        with open(args.pool, "w") as f:
            for q in queries:
                cands = sorted(id_to_doc[d] for d in pool[q["qid"]])
                f.write(json.dumps({
                    "qid": q["qid"], "query": q["query"], "relevant": q["relevant"],
                    "unjudged": [[c, titles[bookmark_uuid(c)]] for c in cands if c not in q["relevant"]],
                }) + "\n")
    if args.json:
        args.json.write_text(json.dumps({n: results[n] for n in methods}, indent=1))


if __name__ == "__main__":
    main()
