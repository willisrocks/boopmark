# /// script
# requires-python = ">=3.11"
# dependencies = ["psycopg[binary]>=3.2", "fastembed>=0.4", "numpy"]
# ///
"""Seed a local Boopmark database with the evaluation corpus.

Writes, for one user:
  * bookmarks         - the real app table (migration 004), so the running app
                        shows the seeded data and today's search can be measured;
  * bookmark_contents - extracted article text + weighted tsvector (prototype);
  * bookmark_chunks   - chunked passages embedded with a local model (prototype).

Usage (after the server has run migrations 001-014 and the user exists):

    uv run scripts/search-eval/seed.py --db postgres://boopmark@127.0.0.1:55432/boopmark \
        --corpus ~/search-eval-data/corpus.jsonl --email eval@example.com
"""

import argparse
import datetime as dt
import hashlib
import json
import pathlib
import random
import time
import urllib.parse
import uuid

import psycopg

from chunking import chunk_article

TEXT_CAP_CHARS = 200_000
LOCAL_MODEL = "BAAI/bge-small-en-v1.5"
MODEL_SLUG = "bge-small-en-v1.5"
NS = uuid.UUID("8b0c9a52-56f4-4b8e-9b0e-2f3f2b1a0c11")


def bookmark_uuid(doc_id: int) -> uuid.UUID:
    """Stable id so qrels can name bookmarks by corpus id."""
    return uuid.uuid5(NS, str(doc_id))


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--db", required=True)
    ap.add_argument("--corpus", required=True, type=pathlib.Path)
    ap.add_argument("--email", required=True)
    ap.add_argument("--schema", type=pathlib.Path,
                    default=pathlib.Path(__file__).parent / "sql" / "prototype_schema.sql")
    args = ap.parse_args()

    docs = [json.loads(line) for line in open(args.corpus)]
    rng = random.Random(7)
    now = dt.datetime.now(dt.timezone.utc)

    with psycopg.connect(args.db, autocommit=False) as conn:
        user_id = conn.execute("SELECT id FROM users WHERE email = %s", (args.email,)).fetchone()
        if not user_id:
            raise SystemExit(f"user {args.email} not found; create it first")
        user_id = user_id[0]

        conn.execute(args.schema.read_text())
        conn.execute("DELETE FROM bookmarks WHERE user_id = %s", (user_id,))

        for d in docs:
            created = now - dt.timedelta(minutes=rng.randint(0, 2 * 365 * 24 * 60))
            domain = urllib.parse.urlparse(d["url"]).hostname
            conn.execute(
                """INSERT INTO bookmarks (id, user_id, url, title, description, domain, created_at, updated_at)
                   VALUES (%s, %s, %s, %s, %s, %s, %s, %s)""",
                (bookmark_uuid(d["id"]), user_id, d["url"], d["title"], d["description"] or None,
                 domain, created, created),
            )
            text = d["text"][:TEXT_CAP_CHARS]
            conn.execute(
                """INSERT INTO bookmark_contents
                     (bookmark_id, user_id, status, text, text_chars, content_hash, search, fetched_at)
                   VALUES (%s, %s, 'fetched', %s, %s, %s,
                     setweight(to_tsvector('english', coalesce(%s, '')), 'A') ||
                     setweight(to_tsvector('english', coalesce(%s, '')), 'B') ||
                     setweight(to_tsvector('english', %s), 'C') ||
                     setweight(to_tsvector('english', %s), 'D'),
                     now())""",
                (bookmark_uuid(d["id"]), user_id, text, len(text),
                 hashlib.sha256(text.encode()).hexdigest(),
                 d["title"], d["description"], d["url"], text),
            )
        conn.commit()
        print(f"seeded {len(docs)} bookmarks + contents for {args.email}")

        # Chunk + embed with the local model.
        from fastembed import TextEmbedding

        model = TextEmbedding(LOCAL_MODEL)
        rows = []
        for d in docs:
            for ord_, (chunk, embed_input) in enumerate(
                chunk_article(d["title"], d["description"], d["text"][:TEXT_CAP_CHARS])
            ):
                rows.append((bookmark_uuid(d["id"]), ord_, chunk, embed_input))
        t0 = time.monotonic()
        vectors = list(model.embed([r[3] for r in rows], batch_size=64))
        secs = time.monotonic() - t0
        with conn.cursor() as cur:
            cur.executemany(
                """INSERT INTO bookmark_chunks (bookmark_id, user_id, ord, text, model, embedding)
                   VALUES (%s, %s, %s, %s, %s, %s::vector)""",
                [(r[0], user_id, r[1], r[2], MODEL_SLUG, "[" + ",".join(f"{x:.6f}" for x in v) + "]")
                 for r, v in zip(rows, vectors)],
            )
        conn.commit()
        conn.execute("ANALYZE bookmark_chunks")
        conn.execute("ANALYZE bookmark_contents")
        conn.commit()
        print(f"embedded {len(rows)} chunks with {MODEL_SLUG} in {secs:.1f}s "
              f"({len(rows) / secs:.1f} chunks/s)")


if __name__ == "__main__":
    main()
