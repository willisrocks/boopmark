# /// script
# requires-python = ">=3.11"
# dependencies = ["httpx>=0.27", "trafilatura>=1.12"]
# ///
"""Build the hybrid-search evaluation corpus (public articles only).

Two sources, written to <out>/corpus.jsonl (one bookmark per line):

* Wikipedia: pages from a fixed list of categories, chosen so that each topic
  has several near neighbours (hard negatives for ranking). Full plain text via
  the MediaWiki `extracts` API; description = first sentences (like og:description).
* Hacker News: the URLs of highly upvoted stories, fetched and run through a
  readability-style extractor (trafilatura), to measure real ingestion
  failure rates and article sizes. Failures are recorded in <out>/ingest_log.jsonl.

The corpus is not committed; re-run this script to regenerate it:

    uv run scripts/search-eval/fetch_corpus.py --out ~/search-eval-data
"""

import argparse
import concurrent.futures as cf
import json
import pathlib
import re
import time

import httpx
import trafilatura

UA = "boopmark-search-eval/0.1 (+https://github.com/willisrocks/boopmark)"
WIKI = "https://en.wikipedia.org/w/api.php"

CATEGORIES = [
    "Sorting algorithms",
    "Cryptographic hash functions",
    "Distributed computing problems",
    "Relational database management systems",
    "NoSQL",
    "Automatic memory management",
    "Concurrency control",
    "Web frameworks",
    "Lossless compression algorithms",
    "Space telescopes",
    "Volcanoes of Iceland",
    "Bees",
    "Fermented foods",
    "Coffee preparation",
    "Breads",
    "Cognitive biases",
    "Sleep",
    "Typefaces",
    "Board games",
    "Personal finance",
]
PER_CATEGORY = 18


def wiki_get(client: httpx.Client, params: dict) -> httpx.Response:
    """GET with polite backoff: Wikipedia answers bursts with 429."""
    for attempt in range(6):
        r = client.get(WIKI, params=params)
        if r.status_code != 429:
            r.raise_for_status()
            return r
        time.sleep(int(r.headers.get("retry-after", 0) or 0) or 2**attempt)
    r.raise_for_status()
    return r


def wiki_titles(client: httpx.Client, category: str) -> list[str]:
    r = wiki_get(
        client,
        {
            "action": "query",
            "list": "categorymembers",
            "cmtitle": f"Category:{category}",
            "cmlimit": 60,
            "cmtype": "page",
            "format": "json",
        },
    )
    titles = [m["title"] for m in r.json()["query"]["categorymembers"]]
    return [t for t in titles if not t.startswith(("List of", "Outline of", "Index of"))]


def wiki_page(client: httpx.Client, title: str) -> dict | None:
    r = wiki_get(
        client,
        {
            "action": "query",
            "prop": "extracts|info",
            "inprop": "url",
            "explaintext": 1,
            "redirects": 1,
            "titles": title,
            "format": "json",
        },
    )
    page = next(iter(r.json()["query"]["pages"].values()))
    text = (page.get("extract") or "").strip()
    if len(text) < 1500:
        return None
    first = re.split(r"(?<=[.!?])\s+", text.split("\n", 1)[0])
    return {
        "url": page["fullurl"],
        "title": page["title"],
        "description": " ".join(first[:2])[:300],
        "text": text,
    }


def build_wikipedia(out: list[dict], log: list[dict]) -> None:
    with httpx.Client(headers={"User-Agent": UA}, timeout=30) as client:
        seen: set[str] = set()
        for category in CATEGORIES:
            got = 0
            for title in wiki_titles(client, category):
                if got >= PER_CATEGORY or title in seen:
                    continue
                seen.add(title)
                try:
                    page = wiki_page(client, title)
                except httpx.HTTPError as e:
                    log.append({"source": "wikipedia", "title": title, "error": str(e)})
                    continue
                if page is None:
                    continue
                page.update(source="wikipedia", category=category)
                out.append(page)
                got += 1
                time.sleep(0.25)
            print(f"wikipedia {category}: {got}")


def hn_urls(n: int) -> list[dict]:
    hits: list[dict] = []
    page = 0
    with httpx.Client(timeout=30) as client:
        while len(hits) < n and page < 20:
            r = client.get(
                "https://hn.algolia.com/api/v1/search",
                params={
                    "tags": "story",
                    "numericFilters": "points>900",
                    "hitsPerPage": 100,
                    "page": page,
                },
            )
            r.raise_for_status()
            hits += [h for h in r.json()["hits"] if h.get("url")]
            page += 1
    return hits[:n]


def fetch_article(hit: dict) -> tuple[dict | None, dict]:
    url = hit["url"]
    t0 = time.monotonic()
    entry = {"source": "hn", "url": url}
    try:
        with httpx.Client(
            headers={"User-Agent": UA}, timeout=10, follow_redirects=True
        ) as client:
            r = client.get(url)
        entry["status"] = r.status_code
        entry["content_type"] = r.headers.get("content-type", "")
        entry["bytes"] = len(r.content)
        if r.status_code != 200:
            entry["error"] = f"http {r.status_code}"
            return None, entry
        if "html" not in entry["content_type"]:
            entry["error"] = "not html"
            return None, entry
        html = r.text
        text = trafilatura.extract(html, include_comments=False, favor_recall=True)
        meta = trafilatura.extract_metadata(html)
        entry["ms"] = int((time.monotonic() - t0) * 1000)
        if not text or len(text) < 800:
            entry["error"] = "extraction too short"
            entry["chars"] = len(text or "")
            return None, entry
        entry["chars"] = len(text)
        doc = {
            "url": url,
            "title": (meta.title if meta and meta.title else hit.get("title")) or url,
            "description": (meta.description if meta and meta.description else "")[:300],
            "text": text,
            "source": "hn",
            "category": "hn",
        }
        return doc, entry
    except Exception as e:  # network errors, decode errors, timeouts
        entry["error"] = type(e).__name__
        return None, entry


def build_hn(out: list[dict], log: list[dict], n: int) -> None:
    hits = hn_urls(n)
    with cf.ThreadPoolExecutor(max_workers=12) as pool:
        for doc, entry in pool.map(fetch_article, hits):
            log.append(entry)
            if doc:
                out.append(doc)
    ok = sum(1 for e in log if e.get("source") == "hn" and "error" not in e)
    print(f"hn: {ok}/{len(hits)} extracted")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True, type=pathlib.Path)
    ap.add_argument("--hn", type=int, default=220)
    args = ap.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)

    docs: list[dict] = []
    log: list[dict] = []
    build_wikipedia(docs, log)
    build_hn(docs, log, args.hn)

    seen = set()
    with open(args.out / "corpus.jsonl", "w") as f:
        for i, d in enumerate(docs):
            if d["url"] in seen:
                continue
            seen.add(d["url"])
            d["id"] = i
            f.write(json.dumps(d) + "\n")
    with open(args.out / "ingest_log.jsonl", "w") as f:
        for e in log:
            f.write(json.dumps(e) + "\n")
    print(f"wrote {len(seen)} docs to {args.out / 'corpus.jsonl'}")


if __name__ == "__main__":
    main()
