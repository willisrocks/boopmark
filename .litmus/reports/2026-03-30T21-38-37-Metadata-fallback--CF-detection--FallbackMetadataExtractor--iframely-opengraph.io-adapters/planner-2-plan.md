# Proof Plan: Metadata Fallback Feature

**Date:** 2026-03-30
**Feature:** Tiered metadata extraction with CF challenge detection and third-party fallback
**Driver:** curl (backend-only)

---

## Claim 1: MetadataExtractor trait is dyn-compatible (`Box<dyn MetadataExtractor>` compiles)

**What to verify:** The trait definition in `server/src/domain/ports/metadata.rs` is object-safe, and `FallbackMetadataExtractor` holds `Vec<Box<dyn MetadataExtractor>>` that compiles without error.

**Evidence source:** The trait has a single method returning `Pin<Box<dyn Future<...> + Send + '_>>` — a concrete return type, not `impl Trait`, which is the standard pattern for dyn-compatible async traits in Rust. `FallbackMetadataExtractor::new` accepts `Vec<Box<dyn MetadataExtractor>>` in its signature and the code compiles cleanly.

**Verification command:**
```
cargo build -p boopmark-server
```
**Pass criterion:** Zero compiler errors. The build artifact is produced.

**Gap:** None. The trait explicitly avoids `async fn` (which would be non-dyn-compatible) by returning `Pin<Box<dyn Future<...>>>`. Compilation is the authoritative proof.

---

## Claim 2: CF challenge detection works for known patterns and does not false-positive

**What to verify:** `is_cloudflare_challenge()` in `html.rs` returns `true` for challenge pages and `false` for normal pages.

**Evidence source:** Four unit tests already exist in `html.rs`:
- `detects_cloudflare_challenge_by_title` — HTML with `<title>Just a moment...</title>`
- `detects_cloudflare_challenge_by_verification_text` — HTML body containing "Performing security verification"
- `does_not_flag_normal_page_as_challenge` — plain blog HTML
- `does_not_flag_page_mentioning_moment_in_body` — "Just a moment..." in body text but not in `<title>`

Additionally, the `cf-mitigated: challenge` header path is exercised at the HTTP response level (not via `is_cloudflare_challenge`) and is tested implicitly through the `HtmlMetadataExtractor::extract` method path.

**Verification command:**
```
cargo test -p boopmark-server adapters::metadata::html::tests
```
**Pass criterion:** All 4 html tests pass.

**Gap — missing test:** There is no unit test for the `cf-mitigated: challenge` header detection path in `HtmlMetadataExtractor::extract`. The body-detection tests exercise `is_cloudflare_challenge` directly but a mock HTTP server test covering the header path would close this gap. This is a low-risk gap (the header check is a one-liner), but the proof plan notes it as an area for a future test.

---

## Claim 3: FallbackMetadataExtractor chains correctly (fallback on error, short-circuit on success)

**What to verify:** The composite extractor tries extractors in order, returns the first `Ok`, and only advances to the next extractor on `Err`.

**Evidence source:** Three unit tests in `fallback.rs`:
- `falls_back_to_second_extractor_on_error` — first extractor fails with `DomainError::Internal("blocked")`, second succeeds; result equals second extractor's output.
- `returns_first_success_without_trying_later` — first extractor succeeds; result equals first extractor's title, not second's.
- `returns_last_error_when_all_fail` — both fail; result is `Err`.

The loop implementation in `FallbackMetadataExtractor::extract` returns immediately on `Ok(meta)` and only updates `last_err` on `Err`, which is consistent with these tests.

**Verification command:**
```
cargo test -p boopmark-server adapters::metadata::fallback::tests
```
**Pass criterion:** All 3 fallback tests pass.

**Gap:** None. The three tests cover the critical branching paths.

---

## Claim 4: FallbackMetadataExtractor preserves CF errors even when later extractors fail differently

**What to verify:** If the first extractor returns a CF challenge error and all subsequent extractors also fail (with different errors), the final error message still contains `CF_CHALLENGE_MSG`, not the last non-CF error.

**Evidence source:** The implementation captures `cf_err` on first encounter of `CF_CHALLENGE_MSG` and at the end returns `Err(cf_err.unwrap_or(last_err))`. This means a CF error from extractor 1 survives even when extractor 2 fails with a different message.

**Gap — missing test:** The existing `returns_last_error_when_all_fail` test uses two identical `FailingExtractor` instances (both return "blocked", not a CF error). There is no test for the scenario: CF error from extractor 1 + generic error from extractor 2 → final error contains `CF_CHALLENGE_MSG`. This is the primary behaviorally significant gap in the test suite.

**Verification command (after adding the missing test):**
```
cargo test -p boopmark-server adapters::metadata::fallback::tests::preserves_cf_error_when_later_extractor_fails_differently
```
**Pass criterion:** The new test passes and the returned error's `to_string()` contains `"blocked by Cloudflare challenge"`.

**Suggested test to add to `fallback.rs`:**
```rust
struct CfExtractor;
impl MetadataExtractor for CfExtractor {
    fn extract(&self, _url: &str) -> Pin<Box<dyn Future<Output = Result<UrlMetadata, DomainError>> + Send + '_>> {
        Box::pin(async { Err(DomainError::Internal(CF_CHALLENGE_MSG.to_string())) })
    }
}

#[tokio::test]
async fn preserves_cf_error_when_later_extractor_fails_differently() {
    let fallback = FallbackMetadataExtractor::new(vec![
        Box::new(CfExtractor),
        Box::new(FailingExtractor), // returns "blocked", not CF
    ]);
    let err = fallback.extract("https://example.com").await.unwrap_err();
    assert!(err.to_string().contains(CF_CHALLENGE_MSG));
}
```

---

## Claim 5: IframelyExtractor parses API responses correctly

**What to verify:** `IframelyExtractor` deserializes the iframely JSON format into `UrlMetadata` with correct field mapping, and propagates HTTP errors as `Err`.

**Evidence source:** Two integration-style tests in `iframely.rs` that spin up a local Axum mock server:
- `parses_iframely_response` — mock returns `{"meta": {"title": ..., "description": ...}, "links": {"thumbnail": [{"href": ...}]}}`. Asserts `title`, `description`, and `image_url` are correctly mapped.
- `returns_error_on_api_failure` — mock returns HTTP 403. Asserts `result.is_err()`.

The `with_base_url` constructor (package-private) enables injection of the mock server address without any environment variables or real network calls.

**Verification command:**
```
cargo test -p boopmark-server adapters::metadata::iframely::tests
```
**Pass criterion:** Both iframely tests pass.

**Gap:** No test for partial responses (e.g., `links` absent, `thumbnail` empty array, `meta` absent). These paths fall back to `None` fields via `unwrap_or` and `.and_then` chaining, which is correct but untested. Low risk given Rust's type system enforces the `Option` unwrapping, but worth noting.

---

## Claim 6: OpengraphIoExtractor parses API responses correctly

**What to verify:** `OpengraphIoExtractor` deserializes the opengraph.io `hybridGraph` JSON format into `UrlMetadata`, and propagates HTTP errors as `Err`.

**Evidence source:** Two integration-style tests in `opengraph_io.rs` using a local Axum mock server with fallback routing (necessary because the URL is encoded into the request path):
- `parses_opengraph_io_response` — mock returns `{"hybridGraph": {"title": ..., "description": ..., "image": ...}}`. Asserts correct mapping.
- `returns_error_on_api_failure` — mock returns HTTP 500. Asserts `result.is_err()`.

**Verification command:**
```
cargo test -p boopmark-server adapters::metadata::opengraph_io::tests
```
**Pass criterion:** Both opengraph_io tests pass.

**Gap:** Same partial-response gap as iframely. Also, the `serde rename` of `hybridGraph` is verified only through the success test — a test with `hybridGraph: null` would confirm the `unwrap_or` default path, but this is low risk.

---

## Claim 7: URL validation rejects private/internal URLs before third-party forwarding

**What to verify:** `validate_public_url` in `mod.rs` rejects localhost, 127.0.0.1, ::1, `.local`, `.internal`, `10.*`, `192.168.*`, `172.16.*`, URLs with credentials, and non-http/https schemes. Accepts normal public URLs.

**Evidence source:** The function is implemented in `server/src/adapters/metadata/mod.rs` and is called by both `IframelyExtractor` and `OpengraphIoExtractor` before making outbound requests.

**Gap — missing tests:** `validate_public_url` has no unit tests at all. This is the largest testing gap in the feature. All the rejection logic is present in code but entirely untested.

**Verification command (after adding tests):**
```
cargo test -p boopmark-server adapters::metadata::tests
```

**Suggested tests to add to `mod.rs`:**
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_localhost() {
        assert!(validate_public_url("http://localhost/path").is_err());
    }

    #[test]
    fn rejects_127_0_0_1() {
        assert!(validate_public_url("http://127.0.0.1/path").is_err());
    }

    #[test]
    fn rejects_private_10_block() {
        assert!(validate_public_url("http://10.0.0.1/path").is_err());
    }

    #[test]
    fn rejects_private_192_168_block() {
        assert!(validate_public_url("http://192.168.1.1/path").is_err());
    }

    #[test]
    fn rejects_private_172_16_block() {
        assert!(validate_public_url("http://172.16.0.1/path").is_err());
    }

    #[test]
    fn rejects_dot_local() {
        assert!(validate_public_url("http://myhost.local/path").is_err());
    }

    #[test]
    fn rejects_dot_internal() {
        assert!(validate_public_url("http://service.internal/path").is_err());
    }

    #[test]
    fn rejects_url_with_credentials() {
        assert!(validate_public_url("http://user:pass@example.com/path").is_err());
    }

    #[test]
    fn rejects_non_http_scheme() {
        assert!(validate_public_url("ftp://example.com/file").is_err());
    }

    #[test]
    fn strips_fragment_from_public_url() {
        let result = validate_public_url("https://example.com/page#section").unwrap();
        assert_eq!(result, "https://example.com/page");
    }

    #[test]
    fn accepts_normal_public_url() {
        let result = validate_public_url("https://medium.com/some-article");
        assert!(result.is_ok());
    }
}
```

---

## Claim 8: Config parsing works for all MetadataFallbackBackend variants

**What to verify:** `MetadataFallbackBackend` correctly maps `"iframely"` → `Iframely`, `"opengraph_io"` → `OpengraphIo`, and anything else (including unset / `"none"`) → `None`.

**Evidence source:** Three unit tests in `config.rs`:
- `metadata_fallback_backend_default_is_none` — input `"none"` → `MetadataFallbackBackend::None`
- `metadata_fallback_backend_parses_iframely` — input `"iframely"` → `MetadataFallbackBackend::Iframely`
- `metadata_fallback_backend_parses_opengraph_io` — input `"opengraph_io"` → `MetadataFallbackBackend::OpengraphIo`

The tests exercise the match arm logic directly (not via `env::var`) to avoid test-environment coupling.

**Verification command:**
```
cargo test -p boopmark-server config::tests::metadata_fallback
```
**Pass criterion:** All 3 config tests pass.

**Gap:** No test for an unknown string (e.g., `"typo"`) — the `_` arm maps it to `None`, which is correct but implicit. Low risk.

---

## Claim 9: All 129 existing tests still pass (no regressions)

**What to verify:** The full test suite produces `test result: ok. 129 passed; 0 failed`.

**Verification command:**
```
cargo test --workspace
```
**Pass criterion:** Output line reads `test result: ok. 129 passed; 0 failed; 0 ignored`.

**Evidence source:** Test run at plan time confirmed this baseline. The feature adds new tests in `fallback.rs`, `html.rs`, `iframely.rs`, `opengraph_io.rs`, and `config.rs` — all existing tests must continue to pass alongside any new ones.

**Gap:** None for the regression claim. After adding the missing tests from Claims 4 and 7, the total count will rise above 129 — that is expected and correct.

---

## Claim 10: Clippy and fmt are clean

**What to verify:** `cargo clippy` emits no warnings or errors; `cargo fmt --check` reports no diffs.

**Evidence source from plan-time run:**

- **Clippy:** Clean — `Finished dev profile` with no warnings.
- **fmt:** FAILING — two files have formatting diffs:
  - `server/src/adapters/metadata/fallback.rs` lines 31-37: multi-line `if` condition should be collapsed to one line per rustfmt.
  - `server/src/adapters/metadata/mod.rs` lines 11-12 and 35-39: `url::Url::parse(url).map_err(...)` and the `if let ... &&` block need indentation adjustment per rustfmt.

**Verification commands:**
```
cargo clippy --workspace -- -D warnings
cargo fmt --check --all
```
**Pass criteria:**
- Clippy: exit code 0, no diagnostic output.
- fmt: exit code 0 (no diff).

**Action required:** Run `cargo fmt --all` to auto-fix the two formatting diffs before considering the feature done.

---

## Summary of Gaps and Required Actions

| # | Claim | Status | Action Required |
|---|-------|--------|-----------------|
| 1 | dyn-compatible trait | PASS | None |
| 2 | CF detection | MOSTLY PASS | Add test for `cf-mitigated` header path (low priority) |
| 3 | Fallback chaining | PASS | None |
| 4 | CF error preservation | GAP | Add `preserves_cf_error_when_later_extractor_fails_differently` test to `fallback.rs` |
| 5 | Iframely parsing | PASS | None |
| 6 | Opengraph.io parsing | PASS | None |
| 7 | URL validation | GAP | Add `#[cfg(test)] mod tests` block to `mod.rs` (11 test cases listed above) |
| 8 | Config parsing | PASS | None |
| 9 | No regressions | PASS | Rerun after adding new tests |
| 10 | Clippy + fmt | PARTIAL | Run `cargo fmt --all` to fix 2 files; clippy already clean |

**Blocking gaps before shipping:** Claims 4, 7, and 10 (fmt). Add the two missing test suites and run `cargo fmt --all`.
