"""Shared by the Postgres harness (eval.py) and the no-Postgres hosted harness
(hosted_eval.py): ids, corpus loading, scenarios, fusion and metrics.

Only the standard library here, so every script can import it.
"""

import json
import math
import pathlib
import random
import statistics
import uuid

HERE = pathlib.Path(__file__).parent
K = 10
RRF_K = 60
LEG_DEPTH = 50  # each leg contributes its top 50 to the fusion
TEXT_CAP_CHARS = 200_000
LOCAL_MODEL = "BAAI/bge-small-en-v1.5"
MODEL_SLUG = "bge-small-en-v1.5"
NS = uuid.UUID("8b0c9a52-56f4-4b8e-9b0e-2f3f2b1a0c11")
SIM_DROP_SEED = 33
SIM_DROP_RATE = 0.33


def bookmark_uuid(doc_id: int) -> uuid.UUID:
    """Stable id so qrels can name bookmarks by corpus id."""
    return uuid.uuid5(NS, str(doc_id))


def load_jsonl(path: pathlib.Path) -> list[dict]:
    return [json.loads(line) for line in open(path) if line.strip()]


def load_queries(path: pathlib.Path = HERE / "queries.jsonl") -> list[dict]:
    return load_jsonl(path)


def load_unscraped(path: pathlib.Path = HERE / "unscraped.jsonl") -> list[dict]:
    """Real Hacker News links whose article fetch failed (403, JS-only, ...).

    They become bookmarks with metadata only: the HN title stands in for the
    title the user or the LLM enrichment would have saved, no description.
    """
    return [{**d, "description": "", "text": ""} for d in load_jsonl(path)]


def meta_text(d: dict) -> str:
    """What the metadata fallback embeds: one vector per bookmark."""
    return f"{d['title']}\n{d.get('description') or ''}\n{d['url']}"


def scenarios(scraped_ids: list[int], unscraped_ids: list[int]) -> dict[str, dict]:
    """Which bookmarks each index scenario can see, and how.

    excluded:  not in either index at all (no fallback for failed fetches)
    meta_only: indexed by title + description + URL only, in both legs

    main/no-fallback and main/fallback differ only in the 72 real failed links.
    sim/* additionally pretend that a fixed random 33% of the scraped
    articles failed, so that the gap the fallback closes can be measured on
    the original labelled queries (whose relevant articles were all scraped).
    """
    rng = random.Random(SIM_DROP_SEED)
    dropped = sorted(rng.sample(sorted(scraped_ids), round(SIM_DROP_RATE * len(scraped_ids))))
    return {
        "main/no-fallback": {"excluded": unscraped_ids, "meta_only": []},
        "main/fallback": {"excluded": [], "meta_only": unscraped_ids},
        "sim/no-fallback": {"excluded": unscraped_ids + dropped, "meta_only": []},
        "sim/fallback": {"excluded": [], "meta_only": unscraped_ids + dropped},
        "_dropped": dropped,
    }


def rrf(*legs: list[int], k: int = RRF_K, limit: int = K) -> list[int]:
    """Reciprocal Rank Fusion; ties broken by bookmark uuid like the SQL."""
    score: dict[int, float] = {}
    for leg in legs:
        for r, d in enumerate(leg[:LEG_DEPTH], start=1):
            score[d] = score.get(d, 0.0) + 1.0 / (k + r)
    return sorted(score, key=lambda d: (-score[d], str(bookmark_uuid(d))))[:limit]


def metrics(ranked: list[int], relevant: set[int]) -> dict:
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


def summarize(per_query: dict[str, dict], queries: list[dict]) -> dict:
    """Mean metrics over the queries that are present, overall and per type."""
    qs = [q for q in queries if q["qid"] in per_query]
    if not qs:
        return {}
    mean = lambda key, sub: statistics.mean(per_query[q["qid"]][key] for q in sub)  # noqa: E731
    out = {key: mean(key, qs) for key in ("recall", "mrr", "ndcg", "zero")}
    out["n"] = len(qs)
    out["ndcg_by_type"] = {
        t: mean("ndcg", [q for q in qs if q["type"] == t]) for t in sorted({q["type"] for q in qs})
    }
    return out


def print_table(title: str, rows: dict[str, dict], types: list[str]) -> None:
    print(f"\n### {title}\n")
    hdr = "| method | n | R@10 | MRR@10 | nDCG@10 | zero | " + " | ".join(types) + " |"
    print(hdr)
    print("|" + "---|" * (hdr.count("|") - 1))
    for name, s in rows.items():
        if not s:
            continue
        cells = [f"{s['ndcg_by_type'][t]:.3f}" if t in s["ndcg_by_type"] else "–" for t in types]
        print(f"| {name} | {s['n']} | {s['recall']:.3f} | {s['mrr']:.3f} | {s['ndcg']:.3f} | "
              f"{s['zero']:.0%} | " + " | ".join(cells) + " |")
