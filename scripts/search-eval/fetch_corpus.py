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

The corpus text is not committed. corpus_manifest.jsonl (committed) pins the
exact documents: id, URL, title, description and the sha256 of the text. To
rebuild that same corpus, refetching only the listed URLs:

    uv run scripts/search-eval/fetch_corpus.py --out ~/search-eval-data \
        --from-manifest scripts/search-eval/corpus_manifest.jsonl

Each rebuilt doc carries `"match": true` when its text hashes to the pinned
value; pages edited since 2026-10-31 come back with `"match": false`.
To discover a fresh corpus instead (other docs, so the labels won't apply):

    uv run scripts/search-eval/fetch_corpus.py --out ~/search-eval-data
"""

import argparse
import concurrent.futures as cf
import hashlib
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


def sha256(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()


def write_manifest(corpus: pathlib.Path, manifest: pathlib.Path) -> None:
    keys = ("id", "source", "category", "url", "title", "description")
    with open(corpus) as src, open(manifest, "w") as out:
        for line in src:
            d = json.loads(line)
            out.write(json.dumps({**{k: d[k] for k in keys},
                                  "chars": len(d["text"]), "sha256": sha256(d["text"])}) + "\n")


def rebuild(manifest: pathlib.Path, out: pathlib.Path) -> None:
    pinned = [json.loads(line) for line in open(manifest)]

    def text_of(m: dict) -> str | None:
        if m["source"] == "wikipedia":
            with httpx.Client(headers={"User-Agent": UA}, timeout=30) as client:
                page = wiki_page(client, m["title"])
                time.sleep(0.25)
            return page["text"] if page else None
        doc, _ = fetch_article({"url": m["url"], "title": m["title"]})
        return doc["text"] if doc else None

    with cf.ThreadPoolExecutor(max_workers=4) as pool:
        texts = list(pool.map(text_of, pinned))
    n_match = 0
    with open(out / "corpus.jsonl", "w") as f:
        for m, text in zip(pinned, texts):
            if text is None:
                continue
            match = sha256(text) == m["sha256"]
            n_match += match
            doc = {k: m[k] for k in ("id", "source", "category", "url", "title", "description")}
            f.write(json.dumps({**doc, "text": text, "match": match}) + "\n")
    got = sum(t is not None for t in texts)
    print(f"rebuilt {got}/{len(pinned)} docs, {n_match} identical to the pinned text")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True, type=pathlib.Path)
    ap.add_argument("--hn", type=int, default=220)
    ap.add_argument("--from-manifest", type=pathlib.Path,
                    help="refetch exactly the documents pinned in this manifest")
    ap.add_argument("--write-manifest", type=pathlib.Path,
                    help="write a manifest for <out>/corpus.jsonl and exit")
    args = ap.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    if args.write_manifest:
        write_manifest(args.out / "corpus.jsonl", args.write_manifest)
        return
    if args.from_manifest:
        rebuild(args.from_manifest, args.out)
        return

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
