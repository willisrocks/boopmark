# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy", "httpx>=0.27", "tiktoken>=0.7", "fastembed>=0.4"]
# ///
"""Hosted vs local embedding models on the labelled queries, without Postgres.

It embeds the corpus with every requested model, ranks bookmarks by exact
cosine (best chunk per bookmark, the same as the SQL in eval.py), and fuses
those rankings with the lexical legs that eval.py exported from Postgres
(results/lexical_runs.json; ts_rank and word_similarity score each document
on its own, so the lists stay valid without the database). The local
baseline runs on the same rebuilt text, so the comparison is like for like.

    uv run --python 3.12 scripts/search-eval/hosted_eval.py \
        --corpus ~/search-eval-data/corpus.jsonl \
        --models openai:text-embedding-3-small,openai:text-embedding-3-large

Safety:
  * OPENAI_API_KEY is read from the environment only, and is never printed.
  * Before the first paid call, the script counts every input with tiktoken
    and aborts if the projected total exceeds --max-usd (default 3). While
    it runs, it keeps a tally from the API's reported usage and aborts if
    that would cross --max-usd.
  * The output is metrics only. HTTP errors print just the status code;
    request and response bodies are never printed or written.
  * Embeddings are cached in results/cache/ (gitignored), so a rerun does
    not pay again.

Writes --out (default results/hosted-<UTC date>.json): metrics per model,
dimension and scenario, the fallback-gap table, corpus coverage, the
query-embedding latency, and the spend (model, calls, tokens, USD).
"""

import argparse
import datetime as dt
import hashlib
import json
import os
import pathlib
import statistics
import sys
import time

import httpx
import numpy as np

from chunking import chunk_article
from common import (HERE, K, LEG_DEPTH, TEXT_CAP_CHARS, load_jsonl, load_queries, load_unscraped,
                    meta_text, metrics, print_table, rrf, scenarios, summarize)

PRICES = {  # USD per 1M input tokens, list prices checked 2026-11-01
    "openai:text-embedding-3-small": 0.02,
    "openai:text-embedding-3-large": 0.13,
}
# Shortened outputs evaluated per model, besides the native size. OpenAI's
# text-embedding-3 vectors can be truncated and re-normalised, which is what
# the API's `dimensions` parameter does, so one call covers every size.
TRUNCATE = {"openai:text-embedding-3-small": [512], "openai:text-embedding-3-large": [1024, 512]}
BGE_QUERY_PREFIX = "Represent this sentence for searching relevant passages: "
OPENAI_URL = "https://api.openai.com/v1/embeddings"
BATCH_INPUTS = 256
CACHE = HERE / "results" / "cache"

AND_W = "fts AND, ts_rank weighted"
AND_PLUS = "fts AND+ (unaccent; unknown words -> prefix | typo fix)"
TRGM_META = "trgm word_similarity, title+description"


class SpendGuard(Exception):
    pass


class OpenAI:
    """Minimal embeddings client with a hard spend cap."""

    def __init__(self, model: str, max_usd: float, spent: dict):
        key = os.environ.get("OPENAI_API_KEY")
        if not key:
            sys.exit("OPENAI_API_KEY is not set")
        self.model = model
        self.price = PRICES[f"openai:{model}"]
        self.max_usd = max_usd
        self.spent = spent  # shared across models: {"usd": float, ...}
        self.calls = self.tokens = 0
        self.client = httpx.Client(timeout=120, headers={"Authorization": f"Bearer {key}"})

    def embed(self, texts: list[str]) -> np.ndarray:
        out = []
        for i in range(0, len(texts), BATCH_INPUTS):
            out.extend(self._call(texts[i:i + BATCH_INPUTS]))
        return np.asarray(out, dtype=np.float32)

    def _call(self, batch: list[str]) -> list[list[float]]:
        for attempt in range(6):
            try:
                r = self.client.post(OPENAI_URL, json={"model": self.model, "input": batch})
            except httpx.HTTPError as e:
                print(f"  network error ({type(e).__name__}), retrying")
                time.sleep(2 ** attempt)
                continue
            if r.status_code == 429 or r.status_code >= 500:
                wait = float(r.headers.get("retry-after", 0) or 2 ** attempt)
                print(f"  HTTP {r.status_code}, retrying in {wait:.0f}s")
                time.sleep(wait)
                continue
            if r.status_code != 200:
                sys.exit(f"OpenAI returned HTTP {r.status_code}; stopping (body not shown)")
            data = r.json()
            tokens = data["usage"]["total_tokens"]
            usd = tokens * self.price / 1e6
            self.calls += 1
            self.tokens += tokens
            self.spent["usd"] += usd
            if self.spent["usd"] > self.max_usd:
                raise SpendGuard(f"spend reached ${self.spent['usd']:.4f} > ${self.max_usd}")
            return [d["embedding"] for d in sorted(data["data"], key=lambda d: d["index"])]
        sys.exit("OpenAI kept failing after 6 attempts; stopping")


def embed_cached(spec: str, kind: str, texts: list[str], embed) -> np.ndarray:
    digest = hashlib.sha256(json.dumps([spec, kind, texts]).encode()).hexdigest()[:16]
    path = CACHE / f"{spec.replace(':', '_').replace('/', '_')}-{kind}-{digest}.npy"
    if path.exists():
        return np.load(path)
    vecs = embed(texts)
    CACHE.mkdir(parents=True, exist_ok=True)
    np.save(path, vecs)
    return vecs


def normalize(m: np.ndarray, dims: int | None = None) -> np.ndarray:
    if dims:
        m = m[:, :dims]
    return m / np.linalg.norm(m, axis=1, keepdims=True)


def vector_ranking(qv: np.ndarray, mat: np.ndarray, owner: np.ndarray) -> list[int]:
    """Top 200 chunks by cosine, then bookmarks by their best chunk (top 50)."""
    if len(mat) == 0:
        return []
    dist = 1.0 - mat @ qv
    top = np.argsort(dist, kind="stable")[:200]
    best: dict[int, float] = {}
    for i in top:
        d = int(owner[i])
        best[d] = min(best.get(d, 9.0), float(dist[i]))
    return sorted(best, key=lambda d: best[d])[:LEG_DEPTH]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--corpus", required=True, type=pathlib.Path,
                    help="corpus.jsonl rebuilt with fetch_corpus.py --from-manifest")
    ap.add_argument("--models", default="openai:text-embedding-3-small",
                    help="comma list of hosted models (openai:...)")
    ap.add_argument("--baseline", default="fastembed:BAAI/bge-small-en-v1.5",
                    help="local model run on the same text; '' to skip")
    ap.add_argument("--runs", type=pathlib.Path, default=HERE / "results" / "lexical_runs.json")
    ap.add_argument("--max-usd", type=float, default=3.0)
    ap.add_argument("--out", type=pathlib.Path,
                    default=HERE / "results" / f"hosted-{dt.datetime.now(dt.timezone.utc):%Y-%m-%d}.json")
    args = ap.parse_args()

    manifest = {d["id"]: d for d in load_jsonl(HERE / "corpus_manifest.jsonl")}
    corpus = {d["id"]: d for d in load_jsonl(args.corpus)}
    missing = sorted(set(manifest) - set(corpus))
    changed = sorted(i for i, d in corpus.items() if not d.get("match", True))
    unscraped = load_unscraped()
    print(f"corpus: {len(corpus)}/{len(manifest)} docs rebuilt, {len(changed)} with edited text "
          f"(kept), {len(missing)} missing (dropped)")

    queries = []
    for q in load_queries():
        rel = [d for d in q["relevant"] if d not in missing]
        if rel:
            queries.append({**q, "relevant": rel})
    texts = [q["query"] for q in queries]
    runs = json.loads(args.runs.read_text())
    drop = set(missing)
    runs = {s: {m: {qid: [d for d in v if d not in drop] for qid, v in per.items()}
                for m, per in r.items()} for s, r in runs.items()}

    # Inputs: article chunks of the scraped docs, one metadata text for every
    # bookmark (the fallback), and the queries.
    chunk_in, chunk_owner = [], []
    for i in sorted(corpus):
        d = corpus[i]
        for _, embed_input in chunk_article(d["title"], d["description"], d["text"][:TEXT_CAP_CHARS]):
            chunk_in.append(embed_input)
            chunk_owner.append(i)
    meta_docs = [corpus[i] for i in sorted(corpus)] + unscraped
    meta_in, meta_owner = [meta_text(d) for d in meta_docs], [d["id"] for d in meta_docs]
    chunk_owner, meta_owner = np.asarray(chunk_owner), np.asarray(meta_owner)
    print(f"inputs: {len(chunk_in)} chunks, {len(meta_in)} metadata texts, {len(texts)} queries")

    hosted = [m for m in args.models.split(",") if m]
    for m in hosted:
        if m not in PRICES:
            sys.exit(f"unknown hosted model {m}; known: {', '.join(PRICES)}")
    import tiktoken

    enc = tiktoken.get_encoding("cl100k_base")
    n_tokens = sum(len(enc.encode(t)) for t in chunk_in + meta_in + texts) + 10 * 20
    projected = {m: n_tokens * PRICES[m] / 1e6 for m in hosted}
    print(f"projected tokens per model: {n_tokens:,}; projected spend: "
          + ", ".join(f"{m} ${v:.4f}" for m, v in projected.items())
          + f"; total ${sum(projected.values()):.4f} (cap ${args.max_usd})")
    if sum(projected.values()) > args.max_usd:
        sys.exit("projected spend exceeds --max-usd; nothing was called")

    spent = {"usd": 0.0}
    specs = ([args.baseline] if args.baseline else []) + hosted
    embedded, spend, latency = {}, {}, {}
    for spec in specs:
        provider, model = spec.split(":", 1)
        print(f"embedding with {spec} ...")
        if provider == "fastembed":
            from fastembed import TextEmbedding

            local = TextEmbedding(model)
            prefix = BGE_QUERY_PREFIX if "bge" in model.lower() else ""

            def doc_fn(t, local=local):
                return np.asarray(list(local.embed(t, batch_size=64)), dtype=np.float32)

            def query_fn(t, local=local, prefix=prefix):
                return doc_fn([prefix + x for x in t])
        else:
            client = OpenAI(model, args.max_usd, spent)
            doc_fn = query_fn = client.embed
        try:
            vecs = {kind: embed_cached(spec, kind, inp, fn) for kind, inp, fn in
                    [("chunks", chunk_in, doc_fn), ("meta", meta_in, doc_fn), ("queries", texts, query_fn)]}
            ms = []
            for t in texts[:10]:  # one query at a time, as the server would
                t0 = time.perf_counter()
                query_fn([t])
                ms.append((time.perf_counter() - t0) * 1000)
            latency[spec] = {"p50_ms": statistics.median(ms), "max_ms": max(ms), "n": len(ms)}
        except SpendGuard as e:
            print(f"SPEND GUARD: {e}; stopping before any further call")
            break
        embedded[spec] = vecs
        if provider != "fastembed":
            spend[spec] = {"calls": client.calls, "tokens": client.tokens,
                           "usd": round(client.tokens * client.price / 1e6, 6)}

    scraped_ids = sorted(manifest)
    scen = scenarios(scraped_ids, [d["id"] for d in unscraped])
    dropped = scen.pop("_dropped")
    types = sorted({q["type"] for q in queries})
    out: dict = {"date": dt.datetime.now(dt.timezone.utc).isoformat(timespec="seconds"),
                 "corpus": {"rebuilt": len(corpus), "pinned": len(manifest), "edited": changed,
                            "missing": missing, "queries": len(queries)},
                 "spend": spend, "spend_total_usd": round(spent["usd"], 6),
                 "query_latency": latency, "models": {}}
    for spec, vecs in embedded.items():
        native = vecs["chunks"].shape[1]
        for dims in [None] + TRUNCATE.get(spec, []):
            label = f"{spec} ({dims or native}d)"
            C, M, Q = (normalize(vecs[k], dims) for k in ("chunks", "meta", "queries"))
            res: dict = {}
            for sname, sc in scen.items():
                nochunk = set(sc["excluded"]) | set(sc["meta_only"])
                cmask = ~np.isin(chunk_owner, list(nochunk))
                mmask = np.isin(meta_owner, list(sc["meta_only"]))
                mat = np.vstack([C[cmask], M[mmask]])
                owner = np.concatenate([chunk_owner[cmask], meta_owner[mmask]])
                meta_all = ~np.isin(meta_owner, list(sc["excluded"]))
                lex = runs[sname]
                methods: dict[str, dict] = {"vector (chunks)": {}, "vector (metadata only)": {}}
                for qi, q in enumerate(queries):
                    qid = q["qid"]
                    vr = vector_ranking(Q[qi], mat, owner)
                    methods["vector (chunks)"][qid] = vr
                    methods["vector (metadata only)"][qid] = vector_ranking(Q[qi], M[meta_all], meta_owner[meta_all])
                    for name, legs in {
                        f"hybrid: {AND_W} + vector": [AND_W],
                        f"hybrid: {AND_PLUS} + vector": [AND_PLUS],
                        f"hybrid: {TRGM_META} + vector": [TRGM_META],
                        f"hybrid: {AND_W} + trgm title+description + vector": [AND_W, TRGM_META],
                    }.items():
                        methods.setdefault(name, {})[qid] = rrf(*[lex[leg][qid] for leg in legs], vr)
                res[sname] = {m: {q["qid"]: metrics(ranked[q["qid"]], set(q["relevant"])) for q in queries}
                              for m, ranked in methods.items()}
            summ = {s: {m: summarize(pq, queries) for m, pq in r.items()} for s, r in res.items()}
            unscr = [q for q in queries if q["type"] == "unscraped"]
            sim_q = [q for q in queries if q["type"] != "unscraped" and set(q["relevant"]) & set(dropped)]
            orig = [q for q in queries if q["type"] in ("keyword", "natural", "deep")]
            gap = {m: {"real_none": summarize(res["main/no-fallback"][m], unscr)["ndcg"],
                       "real_fallback": summarize(res["main/fallback"][m], unscr)["ndcg"],
                       "sim_none": summarize(res["sim/no-fallback"][m], sim_q)["ndcg"],
                       "sim_fallback": summarize(res["sim/fallback"][m], sim_q)["ndcg"],
                       "sim_full": summarize(res["main/fallback"][m], sim_q)["ndcg"]}
                   for m in res["main/fallback"]}
            out["models"][label] = {
                "main/fallback": summ["main/fallback"],
                "main/no-fallback (original 24)": {m: summarize(pq, orig) for m, pq in res["main/no-fallback"].items()},
                "fallback_gap": gap}
            print_table(f"{label}: main/fallback, {len(queries)} queries", summ["main/fallback"], types)

    print(f"\nspend: {json.dumps(spend)}; total ${spent['usd']:.4f}")
    print("query embedding latency: " + json.dumps(
        {k: round(v["p50_ms"], 1) for k, v in latency.items()}))
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(out, indent=1))
    print(f"wrote {args.out}")


if __name__ == "__main__":
    main()
