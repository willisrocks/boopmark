# /// script
# requires-python = ">=3.11"
# dependencies = ["psycopg[binary]>=3.2", "fastembed>=0.4", "numpy", "httpx>=0.27"]
# ///
"""Embed the seeded corpus with another model/mode into eval_chunks.

  --mode chunks    article chunks (same chunker as the prototype)
  --mode metadata  one vector per bookmark from title + description + url only,
                   to isolate the value of the full article text

    uv run scripts/search-eval/embed_variant.py --db ... --corpus ... --email ... \
        --variant minilm --embedder fastembed:sentence-transformers/all-MiniLM-L6-v2
    uv run scripts/search-eval/embed_variant.py ... --variant oai-small \
        --embedder openai:text-embedding-3-small          # paid: see spend.jsonl
"""

import argparse
import json
import pathlib
import time

import psycopg

from chunking import chunk_article
from embedders import Embedder
from seed import TEXT_CAP_CHARS, bookmark_uuid

SCHEMA = """
CREATE TABLE IF NOT EXISTS eval_chunks (
    variant TEXT NOT NULL,
    embedder TEXT NOT NULL,
    bookmark_id UUID NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE,
    user_id UUID NOT NULL,
    ord INT NOT NULL,
    embedding vector NOT NULL,          -- no fixed dims: exact search only
    PRIMARY KEY (variant, bookmark_id, ord)
);
"""


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--db", required=True)
    ap.add_argument("--corpus", required=True, type=pathlib.Path)
    ap.add_argument("--email", required=True)
    ap.add_argument("--variant", required=True)
    ap.add_argument("--embedder", required=True)
    ap.add_argument("--mode", choices=["chunks", "metadata"], default="chunks")
    args = ap.parse_args()

    docs = [json.loads(line) for line in open(args.corpus)]
    rows = []
    for d in docs:
        if args.mode == "metadata":
            rows.append((bookmark_uuid(d["id"]), 0,
                         f"{d['title']}\n{d['description'] or ''}\n{d['url']}"))
        else:
            for ord_, (_, embed_input) in enumerate(
                chunk_article(d["title"], d["description"], d["text"][:TEXT_CAP_CHARS])
            ):
                rows.append((bookmark_uuid(d["id"]), ord_, embed_input))

    embedder = Embedder(args.embedder)
    t0 = time.monotonic()
    vectors = embedder.documents([r[2] for r in rows])
    secs = time.monotonic() - t0

    with psycopg.connect(args.db) as conn:
        user = conn.execute("SELECT id FROM users WHERE email = %s", (args.email,)).fetchone()[0]
        conn.execute(SCHEMA)
        conn.execute("DELETE FROM eval_chunks WHERE variant = %s", (args.variant,))
        with conn.cursor() as cur:
            cur.executemany(
                "INSERT INTO eval_chunks VALUES (%s, %s, %s, %s, %s, %s::vector)",
                [(args.variant, args.embedder, r[0], user, r[1],
                  "[" + ",".join(f"{x:.6f}" for x in v) + "]") for r, v in zip(rows, vectors)],
            )
    print(f"{args.variant}: embedded {len(rows)} inputs with {args.embedder} "
          f"in {secs:.1f}s ({len(rows) / secs:.1f}/s), dims={len(vectors[0])}")


if __name__ == "__main__":
    main()
