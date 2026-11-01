# /// script
# requires-python = ">=3.11"
# dependencies = ["psycopg[binary]>=3.2", "fastembed>=0.4", "numpy", "httpx>=0.27"]
# ///
"""Measure lexical vs trigram vs vector vs hybrid retrieval on the labelled queries.

Every lexical and vector leg is one SQL statement against the seeded
database, user-scoped, returning its top 50. Hybrids fuse two or three legs
with RRF in Python (common.rrf). The production SQL from the spec is run as
well, and the script checks that it returns the same top 10. Each method runs
in four index scenarios (common.scenarios): with and without the metadata
fallback for failed fetches, on the real failed links and on a simulated 33%
failure rate.

    uv run scripts/search-eval/eval.py --db postgres://boopmark@127.0.0.1:55432/boopmark \
        --email eval@example.com [--bm25] [--pool pool.jsonl] [--json results/local.json] \
        [--per-query per_query.json] [--export-runs results/lexical_runs.json]

--export-runs writes every lexical leg's ranked ids. hosted_eval.py fuses them
with hosted-model vectors, so it does not need Postgres.
"""

import argparse
import collections
import json
import pathlib
import re
import statistics
import time

import psycopg

from common import (HERE, K, LOCAL_MODEL, MODEL_SLUG, bookmark_uuid, load_jsonl, load_queries,
                    load_unscraped, metrics, print_table, rrf, scenarios, summarize)
from embedders import Embedder

META = """setweight(to_tsvector('{cfg}', coalesce(b.title, '')), 'A') ||
          setweight(to_tsvector('{cfg}', coalesce(b.description, '')), 'B') ||
          setweight(to_tsvector('{cfg}', b.url), 'C')"""

SETUP = f"""
CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE EXTENSION IF NOT EXISTS unaccent;
DROP TEXT SEARCH CONFIGURATION IF EXISTS en_unaccent CASCADE;
CREATE TEXT SEARCH CONFIGURATION en_unaccent (COPY = english);
ALTER TEXT SEARCH CONFIGURATION en_unaccent
    ALTER MAPPING FOR hword, hword_part, word WITH unaccent, english_stem;

-- One row per bookmark with every lexical representation the methods need.
-- search_full is NULL when the fetch failed; search_meta covers title (A),
-- description (B) and URL (C) only; the body is weight D.
DROP TABLE IF EXISTS eval_contents;
CREATE TABLE eval_contents AS
SELECT b.id AS bookmark_id, b.user_id, (c.status = 'fetched') AS scraped,
       CASE WHEN c.status = 'fetched' THEN {META.format(cfg='english')}
            || setweight(to_tsvector('english', c.text), 'D') END AS search_full,
       {META.format(cfg='english')} AS search_meta,
       CASE WHEN c.status = 'fetched' THEN {META.format(cfg='en_unaccent')}
            || setweight(to_tsvector('en_unaccent', c.text), 'D') END AS search_full_ua,
       {META.format(cfg='en_unaccent')} AS search_meta_ua,
       unaccent(coalesce(b.title, '') || ' ' || coalesce(b.description, '')) AS meta_text,
       unaccent(coalesce(c.text, '')) AS body
FROM bookmarks b JOIN bookmark_contents c ON c.bookmark_id = b.id
WHERE b.user_id = :user;

-- Production-shaped columns (fallback on), used only by the latency pass.
ALTER TABLE eval_contents ADD COLUMN search_main tsvector, ADD COLUMN search_main_ua tsvector,
    ADD COLUMN all_text text;
UPDATE eval_contents SET search_main = coalesce(search_full, search_meta),
    search_main_ua = coalesce(search_full_ua, search_meta_ua),
    all_text = meta_text || ' ' || body;
CREATE INDEX ON eval_contents USING gin (search_main);
CREATE INDEX ON eval_contents USING gin (search_main_ua);
CREATE INDEX ON eval_contents USING gin (meta_text gin_trgm_ops);
CREATE INDEX ON eval_contents USING gin (all_text gin_trgm_ops);

-- The user's vocabulary, for typo correction (PostgreSQL docs, pg_trgm,
-- "Text Search Integration"). Production keeps one per user.
DROP TABLE IF EXISTS eval_lexicon;
CREATE TABLE eval_lexicon AS
SELECT word, ndoc FROM ts_stat($$SELECT to_tsvector('simple', meta_text || ' ' || body)
                                 FROM eval_contents$$)
WHERE length(word) >= 3;
CREATE UNIQUE INDEX ON eval_lexicon (word);
CREATE INDEX ON eval_lexicon USING gin (word gin_trgm_ops);

-- bge-small chunks as an eval_chunks variant. The design's fallback is that
-- a failed fetch gets one chunk made of its metadata, so bookmark_chunks
-- (used by the production SQL) gets those too.
DELETE FROM eval_chunks WHERE variant = 'bge-small';
INSERT INTO eval_chunks (variant, embedder, bookmark_id, user_id, ord, embedding)
SELECT 'bge-small', 'fastembed:{LOCAL_MODEL}', bookmark_id, user_id, ord, embedding::vector
FROM bookmark_chunks WHERE model = '{MODEL_SLUG}' AND user_id = :user
  AND bookmark_id IN (SELECT bookmark_id FROM bookmark_contents WHERE status = 'fetched');
DELETE FROM bookmark_chunks WHERE bookmark_id IN
    (SELECT bookmark_id FROM bookmark_contents WHERE status = 'failed');
INSERT INTO bookmark_chunks (bookmark_id, user_id, ord, text, model, embedding)
SELECT e.bookmark_id, e.user_id, 0, b.title, '{MODEL_SLUG}', e.embedding::vector(384)
FROM eval_chunks e JOIN bookmarks b ON b.id = e.bookmark_id
JOIN bookmark_contents c ON c.bookmark_id = e.bookmark_id
WHERE e.variant = 'meta-bge' AND c.status = 'failed';
ANALYZE eval_contents; ANALYZE eval_lexicon; ANALYZE eval_chunks; ANALYZE bookmark_chunks;
"""

# --- lexical legs -----------------------------------------------------------
# The scenario decides how each bookmark is represented: excluded rows are
# invisible, and meta_only rows are searched by their metadata only.
LEX = """
SELECT c.bookmark_id FROM eval_contents c,
  LATERAL (SELECT CASE WHEN c.bookmark_id = ANY(%(meta_only)s) THEN c.{meta}
                       ELSE coalesce(c.{full}, c.{meta}) END AS doc) d,
  {tsq} q
WHERE c.user_id = %(user)s AND NOT c.bookmark_id = ANY(%(excluded)s) AND d.doc @@ q
ORDER BY {rank} DESC, c.bookmark_id LIMIT 50
"""
EN = {"full": "search_full", "meta": "search_meta"}
UA = {"full": "search_full_ua", "meta": "search_meta_ua"}
AND_Q = "websearch_to_tsquery('english', %(q)s)"
OR_Q = "to_tsquery('english', replace(plainto_tsquery('english', %(q)s)::text, ' & ', ' | '))"
W, UNW, CD = "ts_rank(d.doc, q, 1)", "ts_rank('{1,1,1,1}', d.doc, q, 1)", "ts_rank_cd(d.doc, q, 1)"

TRGM = """
SELECT c.bookmark_id FROM eval_contents c,
  LATERAL (SELECT CASE WHEN c.bookmark_id = ANY(%(meta_only)s) OR c.search_full IS NULL
                       THEN c.meta_text ELSE {text} END AS t) d
WHERE c.user_id = %(user)s AND NOT c.bookmark_id = ANY(%(excluded)s)
  AND unaccent(%(q)s) <%% d.t
ORDER BY word_similarity(unaccent(%(q)s), d.t) DESC, c.bookmark_id LIMIT 50
"""

AND_PLUS = "fts AND+ (unaccent; unknown words -> prefix | typo fix)"
LEGS = {
    "fts AND, ts_rank weighted": LEX.format(tsq=AND_Q, rank=W, **EN),
    "fts AND, ts_rank unweighted": LEX.format(tsq=AND_Q, rank=UNW, **EN),
    "fts AND, ts_rank_cd weighted": LEX.format(tsq=AND_Q, rank=CD, **EN),
    "fts OR, ts_rank weighted": LEX.format(tsq=OR_Q, rank=W, **EN),
    "fts AND, OR if empty": None,  # composed in Python from the two rows above
    "fts AND, prefix (term:*)": LEX.format(tsq="to_tsquery('english', %(tsq)s)", rank=W, **EN),
    "fts AND, unaccent": LEX.format(tsq=AND_Q.replace("'english'", "'en_unaccent'"), rank=W, **UA),
    AND_PLUS: LEX.format(tsq="to_tsquery('en_unaccent', %(tsq)s)", rank=W, **UA),
    "trgm word_similarity, title+description": TRGM.format(text="c.meta_text"),
    "trgm word_similarity, title+description+body": TRGM.format(text="c.meta_text || ' ' || c.body"),
}

BM25_SETUP = """
DROP TABLE IF EXISTS eval_bm25;
CREATE EXTENSION IF NOT EXISTS pg_search;
CREATE TABLE eval_bm25 AS
SELECT row_number() OVER ()::int AS id, c.bookmark_id, c.user_id,
       b.title, coalesce(b.description, '') AS description, c.body
FROM eval_contents c JOIN bookmarks b ON b.id = c.bookmark_id;
CREATE INDEX eval_bm25_idx ON eval_bm25 USING bm25 (id, title, description, body, user_id)
    WITH (key_field = 'id');
"""
BM25 = """
SELECT bookmark_id FROM eval_bm25
WHERE id @@@ paradedb.boolean(should => ARRAY[
    paradedb.boost(3.0, paradedb.match('title', %(q)s)),
    paradedb.boost(1.5, paradedb.match('description', %(q)s)),
    paradedb.match('body', %(q)s)
]) AND user_id = %(user)s AND NOT bookmark_id = ANY(%(excluded)s)
ORDER BY paradedb.score(id) DESC, bookmark_id LIMIT 50
"""

# Today's production query (bookmark_repo.rs list()): boolean filter on the
# metadata tsvector, no ranking, newest first.
TODAY = """
SELECT id FROM bookmarks
WHERE user_id = %(user)s AND NOT id = ANY(%(excluded)s)
  AND to_tsvector('english', coalesce(title, '') || ' ' || coalesce(description, '') || ' ' || url)
      @@ plainto_tsquery('english', %(q)s)
ORDER BY created_at DESC LIMIT 10
"""

# --- vector leg: best chunk per bookmark ------------------------------------
VEC = """
WITH nn AS (
    SELECT bookmark_id, embedding <=> %(qv)s::vector AS dist FROM eval_chunks
    WHERE user_id = %(user)s AND (
        (variant = %(chunks)s AND NOT bookmark_id = ANY(%(nochunks)s))
        OR (variant = %(meta)s AND bookmark_id = ANY(%(meta_only)s)))
    ORDER BY dist LIMIT 200
)
SELECT bookmark_id FROM nn GROUP BY bookmark_id ORDER BY min(dist), bookmark_id LIMIT 50
"""

# --- the spec's production query, used to check the Python fusion ----------
SQL_HYBRID = """
WITH lex AS (
    SELECT bookmark_id, row_number() OVER (ORDER BY ts_rank(search, q, 1) DESC, bookmark_id) AS r
    FROM bookmark_contents, websearch_to_tsquery('english', %(q)s) q
    WHERE user_id = %(user)s AND search @@ q
    ORDER BY ts_rank(search, q, 1) DESC, bookmark_id LIMIT 50
),
nn AS (
    SELECT bookmark_id, embedding <=> %(qv)s::vector AS dist
    FROM bookmark_chunks
    WHERE user_id = %(user)s AND model = %(model)s
    ORDER BY embedding <=> %(qv)s::vector
    LIMIT 200
),
vec AS (
    SELECT bookmark_id, row_number() OVER (ORDER BY min(dist), bookmark_id) AS r
    FROM nn GROUP BY bookmark_id ORDER BY min(dist), bookmark_id LIMIT 50
)
SELECT bookmark_id
FROM (SELECT bookmark_id, r FROM lex UNION ALL SELECT bookmark_id, r FROM vec) u
GROUP BY bookmark_id
ORDER BY sum(1.0 / (60 + r)) DESC, bookmark_id
LIMIT 10
"""

# Production-shaped single statements (plain indexed columns, fallback on).
# They are timed separately because the scenario CASE expressions above
# defeat the indexes.
LATENCY = {
    "fts AND (GIN)":
        "SELECT bookmark_id FROM eval_contents, websearch_to_tsquery('english', %(q)s) q "
        "WHERE user_id = %(user)s AND search_main @@ q ORDER BY ts_rank(search_main, q, 1) DESC LIMIT 50",
    "fts AND unaccent (GIN)":
        "SELECT bookmark_id FROM eval_contents, websearch_to_tsquery('en_unaccent', %(q)s) q "
        "WHERE user_id = %(user)s AND search_main_ua @@ q "
        "ORDER BY ts_rank(search_main_ua, q, 1) DESC LIMIT 50",
    "AND+ rewrite: lexicon lookups": None,
    "trgm title+description (GIN trgm)":
        "SELECT bookmark_id FROM eval_contents WHERE user_id = %(user)s "
        "AND %(q)s <%% meta_text ORDER BY word_similarity(%(q)s, meta_text) DESC LIMIT 50",
    "trgm title+description+body (GIN trgm)":
        "SELECT bookmark_id FROM eval_contents WHERE user_id = %(user)s "
        "AND %(q)s <%% all_text ORDER BY word_similarity(%(q)s, all_text) DESC LIMIT 50",
    "vector (exact)":
        "SELECT bookmark_id FROM bookmark_chunks WHERE user_id = %(user)s AND model = %(model)s "
        "ORDER BY embedding <=> %(qv)s::vector LIMIT 200",
    "hybrid SQL (fts AND + vector)": SQL_HYBRID,
}

TOKEN = re.compile(r"\w+")


def fixed_tsquery(conn, q: str) -> str:
    """Keep words the user's library contains. Turn each unknown word into
    `(word:* | nearest_known_word)`, so a prefix or a typo still matches."""
    terms = []
    for t in TOKEN.findall(conn.execute("SELECT lower(unaccent(%s))", (q,)).fetchone()[0]):
        if len(t) < 3 or conn.execute("SELECT 1 FROM eval_lexicon WHERE word = %s", (t,)).fetchone():
            terms.append(t)
            continue
        fix = conn.execute(
            "SELECT word FROM eval_lexicon WHERE word %% %(t)s "
            "ORDER BY similarity(word, %(t)s) DESC, ndoc DESC, word LIMIT 1", {"t": t}).fetchone()
        terms.append(f"({t}:* | {fix[0]})" if fix else f"{t}:*")
    return " & ".join(terms)


def prefix_tsquery(q: str) -> str:
    return " & ".join(f"{t}:*" for t in TOKEN.findall(q.lower()))


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--db", required=True)
    ap.add_argument("--email", required=True)
    ap.add_argument("--queries", type=pathlib.Path, default=HERE / "queries.jsonl")
    ap.add_argument("--manifest", type=pathlib.Path, default=HERE / "corpus_manifest.jsonl",
                    help="lists the scraped corpus ids")
    ap.add_argument("--bm25", action="store_true", help="also measure ParadeDB pg_search")
    ap.add_argument("--pool", type=pathlib.Path, help="write unjudged top-10 hits here")
    ap.add_argument("--json", type=pathlib.Path, help="write the summaries, gap table and latency here")
    ap.add_argument("--per-query", type=pathlib.Path, help="write every method's per-query results here")
    ap.add_argument("--export-runs", type=pathlib.Path,
                    help="write every lexical leg's ranked ids (input of hosted_eval.py)")
    args = ap.parse_args()

    queries = load_queries(args.queries)
    texts = [q["query"] for q in queries]
    scraped = [d["id"] for d in load_jsonl(args.manifest)]
    unscraped = [d["id"] for d in load_unscraped()]
    scen = scenarios(scraped, unscraped)
    dropped = scen.pop("_dropped")
    id_to_doc = {bookmark_uuid(i): i for i in scraped + unscraped}

    local = Embedder(f"fastembed:{LOCAL_MODEL}")
    qvecs = [("[" + ",".join(f"{x:.6f}" for x in v) + "]") for v in local.queries(texts)]

    with psycopg.connect(args.db, autocommit=True) as conn:
        user = conn.execute("SELECT id FROM users WHERE email = %s", (args.email,)).fetchone()[0]
        conn.execute(SETUP.replace(":user", f"'{user}'"))
        legs = dict(LEGS)
        if args.bm25:
            conn.execute(BM25_SETUP)
            legs["BM25 (pg_search)"] = BM25
        # Exact KNN everywhere: the correctness baseline. GIN still runs as
        # bitmap scans.
        conn.execute("SET enable_indexscan = off")

        def ids(cur) -> list[int]:
            return [id_to_doc[r[0]] for r in cur.fetchall()]

        runs: dict = collections.defaultdict(lambda: collections.defaultdict(dict))
        for sname, sc in scen.items():
            excl = [bookmark_uuid(i) for i in sc["excluded"]]
            meta = [bookmark_uuid(i) for i in sc["meta_only"]]
            r = runs[sname]
            for qi, q in enumerate(queries):
                qid = q["qid"]
                p = {"user": user, "q": q["query"], "excluded": excl, "meta_only": meta,
                     "nochunks": excl + meta, "qv": qvecs[qi]}
                r["today (metadata filter, newest first)"][qid] = ids(conn.execute(TODAY, p))
                for name, sql in legs.items():
                    if sql is None or (name.startswith("BM25") and sname.startswith("sim/")):
                        continue  # BM25 sim would need a second index without the dropped bodies
                    p["tsq"] = fixed_tsquery(conn, q["query"]) if name == AND_PLUS else prefix_tsquery(q["query"])
                    r[name][qid] = ids(conn.execute(sql, p))
                r["fts AND, OR if empty"][qid] = (
                    r["fts AND, ts_rank weighted"][qid] or r["fts OR, ts_rank weighted"][qid])
                r[f"vector ({MODEL_SLUG}, chunks)"][qid] = ids(conn.execute(
                    VEC, {**p, "chunks": "bge-small", "meta": "meta-bge"}))
                r["vector (metadata only, 1 per bookmark)"][qid] = ids(conn.execute(
                    VEC, {**p, "chunks": "meta-bge", "meta": "", "nochunks": excl}))
            print(f"ran {sname}")

        # The Python fusion must equal the production SQL (main/fallback).
        same = 0
        for qi, q in enumerate(queries):
            sql_top = ids(conn.execute(SQL_HYBRID, {"user": user, "q": q["query"],
                                                    "qv": qvecs[qi], "model": MODEL_SLUG}))
            r = runs["main/fallback"]
            same += sql_top == rrf(r["fts AND, ts_rank weighted"][q["qid"]],
                                   r[f"vector ({MODEL_SLUG}, chunks)"][q["qid"]])
        print(f"python RRF == production SQL on {same}/{len(queries)} queries")

        latency = {}
        for name, sql in LATENCY.items():
            ms = []
            for qi, q in enumerate(queries):
                p = {"user": user, "q": q["query"], "qv": qvecs[qi], "model": MODEL_SLUG}
                for _ in range(2):  # warm once, time the second run
                    t0 = time.perf_counter()
                    if sql is None:
                        fixed_tsquery(conn, q["query"])
                    else:
                        conn.execute(sql, p).fetchall()
                    elapsed = (time.perf_counter() - t0) * 1000
                ms.append(elapsed)
            ms.sort()
            latency[name] = {"p50": statistics.median(ms), "p95": ms[int(0.95 * len(ms))]}
        titles = {id_to_doc[i]: t for i, t in conn.execute(
            "SELECT id, title FROM bookmarks WHERE user_id = %s", (user,)).fetchall() if i in id_to_doc}

    # --- fusion + metrics --------------------------------------------------
    vec_name = f"vector ({MODEL_SLUG}, chunks)"
    trgm_meta = "trgm word_similarity, title+description"
    results: dict[str, dict[str, dict]] = {}
    for sname, r in runs.items():
        methods = dict(r)
        for leg in list(r):
            if leg.startswith(("fts", "trgm", "BM25")):
                methods[f"hybrid: {leg} + vector"] = {q: rrf(r[leg][q], r[vec_name][q]) for q in r[vec_name]}
        for leg in ("fts AND, ts_rank weighted", AND_PLUS):
            methods[f"hybrid: {leg} + trgm title+description + vector"] = {
                q: rrf(r[leg][q], r[trgm_meta][q], r[vec_name][q]) for q in r[vec_name]}
        # EMBEDDING_BACKEND=disabled: lexical legs only.
        for leg in ("fts AND, ts_rank weighted", "fts AND, OR if empty", AND_PLUS):
            methods[f"lexical only: {leg} + trgm title+description"] = {
                q: rrf(r[leg][q], r[trgm_meta][q]) for q in r[vec_name]}
        results[sname] = {
            name: {q["qid"]: {**metrics(ranked[q["qid"]], set(q["relevant"])), "top": ranked[q["qid"]][:K]}
                   for q in queries if q["qid"] in ranked}
            for name, ranked in methods.items()
        }

    types = sorted({q["type"] for q in queries})
    summaries = {s: {m: summarize(pq, queries) for m, pq in res.items()} for s, res in results.items()}
    orig = [q for q in queries if q["type"] in ("keyword", "natural", "deep")]
    print_table(f"main/fallback: all {len(queries)} queries (nDCG@10 per type)", summaries["main/fallback"], types)
    print_table("main/no-fallback: the original 24 queries (the previous corpus)",
                {m: summarize(pq, orig) for m, pq in results["main/no-fallback"].items()}, types)

    # Fallback gap: queries whose relevant docs have no article text.
    unscr = [q for q in queries if q["type"] == "unscraped"]
    sim_q = [q for q in queries if q["type"] != "unscraped" and set(q["relevant"]) & set(dropped)]
    key = ["fts AND, ts_rank weighted", AND_PLUS, vec_name, "hybrid: fts AND, ts_rank weighted + vector",
           f"hybrid: {AND_PLUS} + vector"]
    gap = {}
    print(f"\n### Fallback gap (nDCG@10). real: {len(unscr)} queries on the 72 failed links; "
          f"sim: {len(sim_q)} queries with a relevant doc among the {len(dropped)} simulated failures\n")
    print("| method | real: none | real: fallback | sim: none | sim: fallback | sim: full text | sim gap closed |")
    print("|---|---|---|---|---|---|---|")
    for m in key:
        nd = lambda s, qs: summarize(results[s][m], qs)["ndcg"]  # noqa: E731
        row = {"real_none": nd("main/no-fallback", unscr), "real_fallback": nd("main/fallback", unscr),
               "sim_none": nd("sim/no-fallback", sim_q), "sim_fallback": nd("sim/fallback", sim_q),
               "sim_full": nd("main/fallback", sim_q)}
        denom = row["sim_full"] - row["sim_none"]
        row["sim_gap_closed"] = (row["sim_fallback"] - row["sim_none"]) / denom if denom > 0 else None
        gap[m] = row
        closed = f"{row['sim_gap_closed']:.0%}" if row["sim_gap_closed"] is not None else "–"
        print(f"| {m} | {row['real_none']:.3f} | {row['real_fallback']:.3f} | {row['sim_none']:.3f} | "
              f"{row['sim_fallback']:.3f} | {row['sim_full']:.3f} | {closed} |")

    print("\n### Latency (production-shaped SQL, warm, ms: p50 | p95)\n")
    for name, v in latency.items():
        print(f"| {name} | {v['p50']:.1f} | {v['p95']:.1f} |")

    if args.pool:
        with open(args.pool, "w") as f:
            for q in queries:
                seen = set()
                for res in results.values():
                    for pq in res.values():
                        if q["qid"] in pq:
                            seen.update(pq[q["qid"]]["top"])
                f.write(json.dumps({
                    "qid": q["qid"], "query": q["query"], "relevant": q["relevant"],
                    "unjudged": [[d, titles.get(d)] for d in sorted(seen - set(q["relevant"]))],
                }) + "\n")
    if args.json:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(json.dumps({
            "summaries": summaries, "fallback_gap": gap, "latency_ms": latency,
            "dropped_sim": dropped}, indent=1))
    if args.per_query:
        args.per_query.write_text(json.dumps(results, separators=(",", ":")))
    if args.export_runs:
        args.export_runs.parent.mkdir(parents=True, exist_ok=True)
        lex = {s: {m: v for m, v in r.items() if not m.startswith("vector")} for s, r in runs.items()}
        args.export_runs.write_text(json.dumps(lex, separators=(",", ":")))


if __name__ == "__main__":
    main()
