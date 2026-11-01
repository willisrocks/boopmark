"""Chunking used by the prototype; the spec's Rust chunker mirrors it.

Paragraph-aware word windows: ~200 words (~270 BERT tokens, inside the
512-token window of small local models and far inside 8k hosted limits),
30-word overlap, at most MAX_CHUNKS per article. Each chunk is embedded with a
short header (title, and description for the first chunk) so that passages
deep in an article still carry the article's topic.
"""

import re

WINDOW_WORDS = 200
OVERLAP_WORDS = 30
MAX_CHUNKS = 64


def chunk_article(title: str, description: str | None, text: str) -> list[tuple[str, str]]:
    """Return [(chunk_text, embedding_input)]."""
    paragraphs = [p.strip() for p in re.split(r"\n\s*\n|\n(?==)", text) if p.strip()]
    words: list[str] = []
    for p in paragraphs:
        words.extend(p.split())
        words.append("\n")  # paragraph marker, kept for readability of chunk text

    chunks: list[str] = []
    start = 0
    step = WINDOW_WORDS - OVERLAP_WORDS
    while start < len(words) and len(chunks) < MAX_CHUNKS:
        window = words[start:start + WINDOW_WORDS]
        chunk = " ".join(window).replace(" \n ", "\n").replace("\n ", "\n").strip()
        if len(chunk.split()) >= 20 or not chunks:
            chunks.append(chunk)
        start += step

    out = []
    for i, chunk in enumerate(chunks):
        header = f"{title}\n"
        if i == 0 and description:
            header += f"{description}\n"
        out.append((chunk, header + chunk))
    return out
