#!/usr/bin/env python3
"""Generate a messy, labeled bookmark library for tag-management experiments.

Writes two files next to this script:
- seed.sql: idempotent SQL that (re)creates the lab user's bookmarks
- ground_truth.json: raw tag -> concept, plus the ambiguous concept pairs

Usage:
  python3 scripts/tag-lab/seed.py [--email e2e@boopmark.local] [--bookmarks 600]
  psql "$DATABASE_URL" -f scripts/tag-lab/seed.sql

The output is deterministic (fixed RNG seed), so measurements are repeatable.
The coverage pass adds each unused spelling, but by replacing the concept's spelling on a
bookmark it can drop the last copy of another one: with the default seed, 600 of the 627
spellings in concepts.py land on a bookmark. ground_truth.json lists exactly the ones used.
The committed measurements are on this library, so the pass stays as it is.
"""
import argparse
import json
import pathlib
import random

from concepts import AMBIGUOUS, CONCEPTS, concept_of

HERE = pathlib.Path(__file__).resolve().parent

# Topic groups keep co-occurring tags plausible (a Rust post gets rust + tokio,
# not rust + recipes), which matters for sample titles shown to an LLM.
GROUPS = {
    "ai": ["ai", "coding-agents", "ai-agents", "llm", "local-ai", "local-llm", "ml", "deep-learning",
           "prompt-engineering", "rag", "embeddings", "vector-db", "openai", "anthropic", "claude",
           "claude-code", "chatgpt", "mcp", "fine-tuning", "evals", "ai-safety", "computer-vision",
           "gpu", "cuda", "llama", "llama-cpp", "ollama", "hugging-face", "transformers",
           "stable-diffusion", "image-generation", "python"],
    "rust": ["rust", "axum", "tokio", "async-rust", "wasm", "tauri", "compilers", "type-systems",
             "concurrency", "performance", "cli", "sqlite", "postgres", "open-source"],
    "langs": ["go", "python", "javascript", "java", "typescript", "cpp", "csharp", "elixir", "zig",
              "haskell", "kotlin", "swift", "ruby", "rails", "php", "sql", "functional-programming",
              "type-systems", "programming", "compilers", "regex", "unicode"],
    "web": ["react", "react-native", "vue", "svelte", "sveltekit", "nextjs", "htmx", "css", "tailwind",
            "html", "frontend", "backend", "web-dev", "web-performance", "accessibility", "browsers",
            "web-components", "javascript", "typescript", "apis", "api-design", "vercel", "cloudflare",
            "electron", "rss", "indieweb", "seo", "google"],
    "design": ["ux", "ui", "ui-ux", "design", "design-systems", "typography", "fonts", "figma", "color",
               "accessibility", "frontend", "css"],
    "infra": ["devops", "docker", "containers", "kubernetes", "aws", "cloud", "postgres", "databases",
              "sqlite", "redis", "linux", "nixos", "nix", "cli", "terminal", "git", "github", "gitlab",
              "ci-cd", "observability", "monitoring", "self-hosting", "homelab", "networking", "dns",
              "security", "privacy", "cryptography", "distributed-systems", "serverless",
              "microservices", "fly", "cloudflare", "operating-systems", "system-design"],
    "eng": ["software-engineering", "programming", "career", "management", "eng-management",
            "leadership", "productivity", "remote-work", "hiring", "interviews", "startups",
            "business", "open-source", "testing", "unit-tests", "tdd", "refactoring", "architecture",
            "documentation", "debugging", "tutorials", "cheatsheets", "tools", "dev-tools", "vim",
            "neovim", "emacs", "vscode", "algorithms", "data-structures", "system-design", "github",
            "git", "books", "writing", "blogging"],
    "apple": ["ios", "macos", "apple", "android", "mobile", "swift", "kotlin", "react-native",
              "hardware", "keyboards"],
    "data": ["data-science", "data-engineering", "analytics", "statistics", "math", "python", "sql",
             "ml", "databases", "postgres"],
    "media": ["books", "podcasts", "videos", "youtube", "news", "reference", "blogging", "writing",
              "rss", "humor", "history", "philosophy"],
    "life": ["physics", "math", "economics", "statistics", "history", "science", "climate", "energy",
             "health", "fitness", "cooking", "recipes", "travel", "music", "games", "game-dev",
             "philosophy", "humor", "finance", "personal-finance", "space"],
    "maker": ["hardware", "electronics", "3d-printing", "keyboards", "raspberry-pi", "arduino",
              "embedded", "robotics", "homelab", "linux", "self-hosting"],
    "biz": ["startups", "business", "marketing", "product", "saas", "seo", "email", "no-code",
            "automation", "web3", "blockchain", "finance", "productivity", "google"],
}

TITLE_TEMPLATES = [
    "A practical guide to {a}",
    "{a} in 2026: what actually changed",
    "Notes on {a} and {b}",
    "Why we moved from {b} to {a}",
    "How I use {a} every day",
    "The hidden costs of {a}",
    "{a} for people who already know {b}",
    "Lessons from a year of {a}",
    "An opinionated intro to {a}",
    "What nobody tells you about {a}",
    "Building a small {a} project with {b}",
    "{a}: a field guide",
]

DOMAINS = ["github.com", "news.ycombinator.com", "lobste.rs", "blog.example.dev", "medium.com",
           "substack.com", "youtube.com", "arxiv.org", "dev.to", "simonwillison.net", "lwn.net"]


def display(concept):
    return CONCEPTS[concept][0]


def pick_variant(rng, concept):
    raws = CONCEPTS[concept]
    # Zipf-ish: the first spelling dominates, the rest are long-tail noise.
    weights = [1.0 / (i + 1) ** 1.3 for i in range(len(raws))]
    return rng.choices(raws, weights=weights)[0]


def generate(n_bookmarks, rng):
    group_names = list(GROUPS)
    bookmarks = []
    for i in range(n_bookmarks):
        group = rng.choice(group_names)
        k = rng.choice([2, 3, 3, 4, 4, 5])
        concepts = rng.sample(GROUPS[group], min(k, len(GROUPS[group])))
        tags = []
        for concept in concepts:
            raw = pick_variant(rng, concept)
            if raw not in tags:
                tags.append(raw)
        a, b = display(concepts[0]), display(concepts[-1])
        title = rng.choice(TITLE_TEMPLATES).format(a=a, b=b)
        domain = rng.choice(DOMAINS)
        bookmarks.append({
            "url": f"https://{domain}/lab/{i:04d}-{concepts[0]}",
            "title": title,
            "domain": domain,
            "tags": tags,
            "concepts": concepts,
        })

    # Coverage pass: give each unused spelling a bookmark (incomplete; see the docstring).
    used = {t for b in bookmarks for t in b["tags"]}
    groups_of = {}
    for g, cs in GROUPS.items():
        for c in cs:
            groups_of.setdefault(c, g)
    for concept, raws in CONCEPTS.items():
        for raw in raws:
            if raw in used:
                continue
            candidates = [b for b in bookmarks if concept in b["concepts"]] or \
                         [b for b in bookmarks if set(b["concepts"]) & set(GROUPS[groups_of[concept]])]
            target = rng.choice(candidates)
            # Replace the concept's current spelling, or add the tag.
            target["tags"] = [t for t in target["tags"] if concept_of()[t] != concept] + [raw]
            if concept not in target["concepts"]:
                target["concepts"].append(concept)
            used = {t for b in bookmarks for t in b["tags"]}
    return bookmarks


def sql_literal(value):
    return "'" + value.replace("'", "''") + "'"


def sql_array(values):
    return "ARRAY[" + ",".join(sql_literal(v) for v in values) + "]::TEXT[]"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--email", default="e2e@boopmark.local")
    parser.add_argument("--bookmarks", type=int, default=600)
    parser.add_argument("--seed", type=int, default=20261031)
    args = parser.parse_args()

    rng = random.Random(args.seed)
    bookmarks = generate(args.bookmarks, rng)
    mapping = concept_of()

    lines = [
        "BEGIN;",
        f"INSERT INTO users (email, name) VALUES ({sql_literal(args.email)}, 'Tag Lab') "
        "ON CONFLICT (email) DO NOTHING;",
        f"DELETE FROM bookmarks WHERE user_id = (SELECT id FROM users WHERE email = {sql_literal(args.email)});",
    ]
    for i, b in enumerate(bookmarks):
        lines.append(
            "INSERT INTO bookmarks (user_id, url, title, domain, tags, created_at) VALUES ("
            f"(SELECT id FROM users WHERE email = {sql_literal(args.email)}), "
            f"{sql_literal(b['url'])}, {sql_literal(b['title'])}, {sql_literal(b['domain'])}, "
            f"{sql_array(b['tags'])}, now() - interval '{i} hours');"
        )
    lines.append("COMMIT;")
    (HERE / "seed.sql").write_text("\n".join(lines) + "\n")

    distinct = sorted({t for b in bookmarks for t in b["tags"]})
    (HERE / "ground_truth.json").write_text(json.dumps({
        "concept_of": {t: mapping[t] for t in distinct},
        "ambiguous": sorted(sorted(p) for p in AMBIGUOUS),
    }, indent=1, sort_keys=True) + "\n")
    print(f"bookmarks={len(bookmarks)} distinct_raw_tags={len(distinct)} "
          f"concepts={len({mapping[t] for t in distinct})}")


if __name__ == "__main__":
    main()
