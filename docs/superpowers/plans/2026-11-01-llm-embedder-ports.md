# Shared LlmClient and Embedder Ports Implementation Plan

> **For agentic workers:** steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the provider-neutral `LlmClient` and `Embedder` ports, their
vendor adapters and routers, and construct both in `main.rs` and `AppState`,
so the tag-management and hybrid-search work can build on them. No feature
behaviour changes.

**Specs:**
- `docs/superpowers/specs/2026-10-31-tag-management-design.md`, section
  "6. Shared LLM port" (branch `spec-tag-management`).
- `docs/superpowers/specs/2026-10-31-hybrid-article-search-design.md`,
  section "Ports and adapters" (branch `spec-hybrid-search`).

**Starting point:** the prototypes on those branches
(`server/src/domain/ports/{llm_client,embedder}.rs`,
`server/src/adapters/{llm_client,embedder}.rs`).

## Decisions (made under auto-pilot, with the evidence)

1. **Interface names are fixed** by the task brief, because other workers
   build against them now: `BoxFuture`, `LlmClient::complete_structured`,
   `StructuredRequest` (7 fields), `StructuredResponse`, `TokenUsage`,
   `Embedder` (`model_id`, `dims`, `enabled`, `embed`), `EmbedKind`,
   `Embeddings`. `embedder.rs` re-exports `BoxFuture` from `llm_client.rs`
   (the search spec says to do this once both are on main).
2. **Effort stays adapter configuration** (`ANTHROPIC_EFFORT`, default
   `low`), not a request field. The brief allows an `effort` option, but a
   new field on `StructuredRequest` would break the struct literals the other
   workers are writing now. Unknown values fall back to `low` with a warning.
3. **Error classes.** `DomainError` has no retry flag, so the two existing
   variants carry it, as both specs and the brief say:
   - `InvalidInput` = do not retry: HTTP 4xx (including 401/403: a rejected
     key does not fix itself), a refusal, an empty or impossible request, a
     disabled backend.
   - `Internal` = retryable: network failures and timeouts, HTTP 408, 409
     and 429 (rate limits are transient by definition; the search spec also
     lists 429 as retryable), 5xx (including Anthropic's 529), malformed
     JSON, a response that does not match the schema, truncation at
     `max_tokens`.
   The brief says "4xx non-retryable"; 408/409/429 are the standard
   transient 4xx codes, and treating a rate limit as permanent would make
   the ingest worker drop work under load. This is called out in the PR.
4. **Schema check.** Adapters validate the parsed answer against the
   request's schema (the subset structured outputs use: `type`, `enum`,
   `const`, `properties`, `required`, `additionalProperties: false`,
   `items`, `anyOf`). Bounds stay the caller's job (`strip_unenforced`
   removes them before sending, as the spec says). A mismatch is
   `Internal` (a retry can produce a valid answer). No new crate.
5. **`EMBEDDING_BACKEND=local`** is the name in the brief; the search spec
   says `fastembed`. Both are accepted. An unknown value is a startup error,
   not a silent fallback. `model_id` keeps the `fastembed:` backend prefix,
   because it names the vector space.
6. **`main.rs` fails fast on embedder misconfiguration** (`expect`, like the
   other `*_BACKEND` settings), but the default is `disabled`, so existing
   deployments start unchanged. `LlmClientRouter` has no failure mode.
7. **LlamaCppLlmClient stays**: it is about 50 lines, used by the tag lab,
   and gets mock-server tests like the hosted adapters.
8. **Nothing is logged** from the adapters except status codes; keys,
   prompts and bodies never reach a log line or an error message.
9. **`openai.rs` is not touched.** `OpenAiLlmClient` parses the Responses
   payload itself, because it must also detect refusals and incomplete
   responses, which `extract_response_text` does not.

## Steps

- [ ] **1. Ports.** Add `domain/ports/llm_client.rs` and
  `domain/ports/embedder.rs`; register them in `ports/mod.rs`. Doc
  comments state the error contract.
- [ ] **2. LLM adapters.** `adapters/llm_client.rs`: shared `post_json`
  with status mapping, `strip_unenforced`, `validate_schema`,
  `AnthropicLlmClient`, `OpenAiLlmClient`, `LlamaCppLlmClient`,
  `LlmClientConfig::from_env`, `LlmClientRouter`. Tests against a local
  axum fake vendor for Anthropic and OpenAI: success (with usage and the
  request body), 401, 429, 5xx, malformed JSON, schema mismatch, refusal,
  truncation; llama.cpp success; router dispatch; config parsing; body
  never leaks into an error.
- [ ] **3. Embedder adapters.** `adapters/embedder.rs`: `OpenAiEmbedder`
  (default `text-embedding-3-large@1024`), `DisabledEmbedder`,
  `FastembedEmbedder` behind `local-embeddings`, `EmbeddingSettings`,
  `EmbedderRouter::from_env`. Add the optional `fastembed` dependency and
  the feature in `server/Cargo.toml`. Tests: settings, router, dimensions,
  batching and order, 401, 429, 5xx, malformed JSON, wrong vector
  count/size; the fastembed test is `#[ignore]` (downloads a model).
- [ ] **4. Wiring.** Build `LlmClientRouter::from_config(&LlmClientConfig::from_env())`
  and `EmbedderRouter::from_env()` in `main.rs`; add `llm_client:
  Arc<dyn LlmClient>` and `embedder: Arc<dyn Embedder>` to `AppState`.
  Add the env vars to `.env.example` and the self-hosting docs.
- [ ] **5. Verify.** `cargo test -p boopmark-server llm_client embedder`,
  `cargo clippy --all-targets -- -D warnings` with and without
  `--features local-embeddings`, `cargo test` for the whole workspace.
