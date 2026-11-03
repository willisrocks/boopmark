"""Summarise data/results-*.jsonl as markdown tables (domains only, no URLs).

python3 report.py            tables to stdout
python3 report.py --pages    per-page quality lines for a manual check (local only)
"""

import collections
import json
import math
import re
import statistics
import sys

from fetch_inputs import DATA

CANDIDATES = ["baseline", "obscura", "firecrawl"]
BLOCK = re.compile(
    r"Performing security verification|Just a moment|verify you are human|Access denied|"
    r"enable JavaScript and cookies|Attention Required|unusual traffic|are you a robot",
    re.IGNORECASE,
)
PAYWALL = re.compile(
    r"Create an account to read the full story|read the full story|available to Medium members only|"
    r"Sign up to read|Become a member to read|Subscribe to (continue|keep) reading|member-only story",
    re.IGNORECASE,
)
FULL_MIN_CHARS = 1500


def quality(r: dict, dead: set) -> str:
    if r["id"] in dead:
        return "dead"
    text = (r.get("text_head") or "") + " " + (r.get("text_tail") or "")
    if not r.get("ok") or not r.get("text_chars"):
        return "block" if r.get("cf_challenge") or "403" in (r.get("error") or "") else "empty"
    if r.get("cf_challenge") or (BLOCK.search(text) and r["text_chars"] < FULL_MIN_CHARS):
        return "block"
    if (
        r.get("locked")
        or r["text_chars"] < FULL_MIN_CHARS
        or (PAYWALL.search(r.get("text_tail") or "") and r["text_chars"] < 4000)
    ):
        return "preview"
    return "full"


def other(counts: collections.Counter) -> str:
    return ", ".join(f"{k} {v}" for k, v in sorted(counts.items()) if k != "full") or "-"


def pct(values: list[int], p: float) -> int:
    s = sorted(values)
    return s[max(0, math.ceil(p * len(s)) - 1)]


def load(name: str, dead: set) -> list[dict]:
    path = DATA / f"results-{name}.jsonl"
    rows = [json.loads(l) for l in path.read_text().splitlines() if l.strip()] if path.exists() else []
    for r in rows:
        r["quality"] = quality(r, dead)
    return rows


def main() -> None:
    # A page the server's own fetch gets a 404 or 410 for, or whose host does not
    # resolve, is gone: no scraper can recover it.
    baseline = load("baseline", set())
    dead = {r["id"] for r in baseline if re.match(r"HTTP 4(04|10)|connection failed", r.get("error") or "")}
    results = {c: load(c, dead) for c in CANDIDATES}
    if "--pages" in sys.argv:
        for c, rows in results.items():
            for r in rows:
                print(
                    c,
                    r["domain"],
                    r["quality"],
                    r.get("text_chars"),
                    (r.get("error") or "")[:60],
                    "|",
                    (r.get("text_tail") or "")[-120:].replace("\n", " "),
                )
        return
    n = max(len(rows) for rows in results.values())
    print(
        f"| Candidate | Full text /{n} | Preview or paywall stub | Block page | Dead link | Empty or error | Median latency | p95 latency | Peak RSS per page (max) |"
    )
    print("|---|---|---|---|---|---|---|---|---|")
    for c, rows in results.items():
        q = collections.Counter(r["quality"] for r in rows)
        lat = [r["latency_ms"] for r in rows]
        rss = [r["peak_rss_mb"] for r in rows if r.get("peak_rss_mb")]
        mem = f"{statistics.median(rss):.0f} MB median, {max(rss):.0f} MB max" if rss else "n/a"
        print(
            f"| {c} | {q['full']} | {q['preview']} | {q['block']} | {q['dead']} | {q['empty']} | "
            f"{statistics.median(lat) / 1000:.1f} s | {pct(lat, 0.95) / 1000:.1f} s | {mem} |"
        )
    print()
    # A rerun returns Firecrawl's cache: the first pass, kept as -run1, has the cold latency.
    for c in CANDIDATES:
        path = DATA / f"results-{c}-run1.jsonl"
        if path.exists():
            lat = [json.loads(line)["latency_ms"] for line in path.read_text().splitlines() if line.strip()]
            print(
                f"{c} first pass (cold cache): median {statistics.median(lat) / 1000:.1f} s, "
                f"p95 {pct(lat, 0.95) / 1000:.1f} s over {len(lat)} pages"
            )
    print()
    by_domain = collections.defaultdict(lambda: collections.defaultdict(collections.Counter))
    for c, rows in results.items():
        for r in rows:
            by_domain[r["domain"]][c][r["quality"]] += 1
    print("| Domain | Pages | " + " | ".join(f"{c} full" for c in CANDIDATES) + " | Obscura other | Firecrawl other |")
    print("|---|---|" + "---|" * (len(CANDIDATES) + 2))
    for d in sorted(by_domain, key=lambda d: (-sum(by_domain[d]["firecrawl"].values()), d)):
        pages = max(sum(by_domain[d][c].values()) for c in CANDIDATES)
        print(
            f"| {d} | {pages} | "
            + " | ".join(str(by_domain[d][c]["full"]) for c in CANDIDATES)
            + f" | {other(by_domain[d]['obscura'])} | {other(by_domain[d]['firecrawl'])} |"
        )


if __name__ == "__main__":
    main()
