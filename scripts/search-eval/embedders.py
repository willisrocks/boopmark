"""Embedding providers for the evaluation: local fastembed and hosted APIs.

Spec strings:
  fastembed:BAAI/bge-small-en-v1.5      local ONNX (no cost)
  openai:text-embedding-3-small         needs OPENAI_API_KEY (paid)
  voyage:voyage-4-lite                  needs VOYAGE_API_KEY (paid)

Hosted calls append one line per request to spend.jsonl (model, inputs,
tokens, estimated USD) so the spec can report actual spend.
"""

import json
import os
import pathlib
import time

import httpx

# USD per 1M input tokens (list prices checked 2026-11-01, see the spec).
PRICES = {
    "openai:text-embedding-3-small": 0.02,
    "openai:text-embedding-3-large": 0.13,
    "voyage:voyage-4-lite": 0.02,
    "voyage:voyage-4": 0.06,
}
BGE_QUERY_PREFIX = "Represent this sentence for searching relevant passages: "
SPEND_LOG = pathlib.Path(__file__).parent / "spend.jsonl"


class Embedder:
    def __init__(self, spec: str):
        self.spec = spec
        self.provider, self.model = spec.split(":", 1)
        if self.provider == "fastembed":
            from fastembed import TextEmbedding

            self._local = TextEmbedding(self.model)
        elif self.provider == "openai":
            self._key = os.environ["OPENAI_API_KEY"]
        elif self.provider == "voyage":
            self._key = os.environ["VOYAGE_API_KEY"]
        else:
            raise ValueError(spec)

    def documents(self, texts: list[str]) -> list[list[float]]:
        return self._embed(texts, query=False)

    def queries(self, texts: list[str]) -> list[list[float]]:
        return self._embed(texts, query=True)

    def _embed(self, texts: list[str], query: bool) -> list[list[float]]:
        if self.provider == "fastembed":
            if query and "bge" in self.model.lower():
                texts = [BGE_QUERY_PREFIX + t for t in texts]
            if "nomic" in self.model.lower():
                texts = [("search_query: " if query else "search_document: ") + t for t in texts]
            return [list(map(float, v)) for v in self._local.embed(texts, batch_size=64)]
        out: list[list[float]] = []
        for i in range(0, len(texts), 96):
            out += self._hosted(texts[i:i + 96], query)
        return out

    def _hosted(self, batch: list[str], query: bool) -> list[list[float]]:
        if self.provider == "openai":
            url, body = "https://api.openai.com/v1/embeddings", {"model": self.model, "input": batch}
        else:
            url = "https://api.voyageai.com/v1/embeddings"
            body = {"model": self.model, "input": batch,
                    "input_type": "query" if query else "document"}
        for attempt in range(5):
            r = httpx.post(url, json=body, timeout=120,
                           headers={"Authorization": f"Bearer {self._key}"})
            if r.status_code != 429 and r.status_code < 500:
                break
            time.sleep(2 ** attempt)
        r.raise_for_status()
        data = r.json()
        tokens = data["usage"]["total_tokens"]
        with open(SPEND_LOG, "a") as f:
            f.write(json.dumps({
                "ts": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "model": self.spec,
                "inputs": len(batch), "tokens": tokens,
                "usd": tokens * PRICES.get(self.spec, 0.0) / 1e6,
            }) + "\n")
        return [d["embedding"] for d in sorted(data["data"], key=lambda d: d["index"])]
