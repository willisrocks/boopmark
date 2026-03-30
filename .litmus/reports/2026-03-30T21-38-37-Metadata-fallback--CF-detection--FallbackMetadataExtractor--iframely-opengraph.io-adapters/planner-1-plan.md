# Metadata Fallback: Proof Plan

**Feature:** Tiered metadata extraction with Cloudflare challenge detection and third-party API fallback.

**Scope:** Backend only. All evidence comes from `cargo test`, `cargo clippy`, and the Rust compiler.

**Test count baseline:** 129 tests across 18 files.

---

## Scenario 1: The codebase compiles without errors

**Claim:** All new modules (`metadata/html.rs`, `metadata/fallback.rs`, `metadata/iframely.rs`, `metadata/opengraph_io.rs`, `metadata/mod.rs`) and all modified files (`config.rs`, `app/bookmarks.rs`, `main.rs`) compile cleanly. The trait dyn-compatibility refactor (replacing `#[trait_variant::make(Send)]` with `Pin<Box<dyn Future>>`) does not break any call site.

**Evidence type:** Compilation

**Command:**
```
cargo build -p boopmark-server
```

**Expected result:** Zero errors. Exit code 0. No `error[E...]` lines in output.

**Confidence target:** PROVEN

---

## Scenario 2: Static analysis reports no warnings promoted to errors

**Claim:** No unused imports, unreachable patterns, dead code, or lint violations introduced by the new adapters or the refactored trait signature.

**Evidence type:** Static analysis

**Command:**
```
cargo clippy -p boopmark-server -- -D warnings
```

**Expected result:** Exit code 0. No `error:` lines. Warnings-as-errors means any latent issue is surfaced.

**Confidence target:** PROVEN

---

## Scenario 3: All 129 tests pass (full suite)

**Claim:** The metadata fallback feature does not regress any existing functionality. All 129 unit tests across all 18 test-bearing files pass.

**Evidence type:** Test output

**Command:**
```
cargo test -p boopmark-server 2>&1 | tail -5
```

**Expected result:**
```
test result: ok. 129 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

**Confidence target:** PROVEN

---

## Scenario 4: CF challenge detection — title heuristic fires

**Claim:** `is_cloudflare_challenge` returns `true` when the response body contains `<title>Just a moment...</title>`.

**Evidence type:** Test output

**Command:**
```
cargo test -p boopmark-server -- html::tests::detects_cloudflare_challenge_by_title --nocapture
```

**Expected result:**
```
test adapters::metadata::html::tests::detects_cloudflare_challenge_by_title ... ok
```

**Confidence target:** PROVEN

---

## Scenario 5: CF challenge detection — verification text heuristic fires

**Claim:** `is_cloudflare_challenge` returns `true` when the body contains "Performing security verification" (even without the `<title>` match).

**Evidence type:** Test output

**Command:**
```
cargo test -p boopmark-server -- html::tests::detects_cloudflare_challenge_by_verification_text --nocapture
```

**Expected result:**
```
test adapters::metadata::html::tests::detects_cloudflare_challenge_by_verification_text ... ok
```

**Confidence target:** PROVEN

---

## Scenario 6: CF challenge detection — no false positives on normal pages

**Claim:** `is_cloudflare_challenge` returns `false` for a normal page and for a page that mentions "Just a moment..." only in the body prose (not in `<title>`).

**Evidence type:** Test output

**Command:**
```
cargo test -p boopmark-server -- html::tests::does_not_flag --nocapture
```

**Expected result:** Both `does_not_flag_normal_page_as_challenge` and `does_not_flag_page_mentioning_moment_in_body` pass.

```
test adapters::metadata::html::tests::does_not_flag_normal_page_as_challenge ... ok
test adapters::metadata::html::tests::does_not_flag_page_mentioning_moment_in_body ... ok
```

**Confidence target:** PROVEN

---

## Scenario 7: HTML extractor parses og:image, twitter:image, relative URLs

**Claim:** The HTML extractor correctly prioritizes `og:image` (property) over `twitter:image`, falls back from `property` to `name` attribute, and resolves relative URLs against the page base URL.

**Evidence type:** Test output

**Command:**
```
cargo test -p boopmark-server -- html::tests --nocapture
```

**Expected result:** All 12 tests in `html::tests` pass. Key tests:
- `extracts_og_image_from_property`
- `falls_back_to_name_attribute`
- `falls_back_to_twitter_image`
- `og_image_takes_priority_over_twitter_image`
- `resolve_url_handles_relative`
- `resolve_url_handles_absolute`
- `resolve_url_handles_data_uri`
- `no_image_meta_returns_none`

**Confidence target:** PROVEN

---

## Scenario 8: FallbackMetadataExtractor chains extractors and returns first success

**Claim:** When the first extractor in the chain errors, `FallbackMetadataExtractor` calls the second extractor and returns its result. When the first succeeds, the second is never tried.

**Evidence type:** Test output

**Command:**
```
cargo test -p boopmark-server -- fallback::tests --nocapture
```

**Expected result:** All 3 tests pass:
```
test adapters::metadata::fallback::tests::falls_back_to_second_extractor_on_error ... ok
test adapters::metadata::fallback::tests::returns_first_success_without_trying_later ... ok
test adapters::metadata::fallback::tests::returns_last_error_when_all_fail ... ok
```

**Confidence target:** PROVEN

---

## Scenario 9: FallbackMetadataExtractor preserves CF challenge signal through the chain

**Claim:** If any extractor in the chain returns an error containing `CF_CHALLENGE_MSG`, the `FallbackMetadataExtractor` propagates that specific error (not the last generic error) so `BookmarkService` can detect it and skip the screenshot.

**Evidence type:** Compilation + code review (no dedicated test for CF propagation in fallback)

**Supporting evidence:** The implementation in `fallback.rs` captures `cf_err` separately from `last_err` and returns `cf_err.unwrap_or(last_err)`. The `CF_CHALLENGE_MSG` constant is defined in `domain/error.rs` and used consistently in `html.rs`, `fallback.rs`, and `bookmarks.rs`. The compiler enforces the shared constant reference.

**Command:**
```
cargo build -p boopmark-server
```

**Expected result:** Clean build proves the CF signal propagation path compiles. The `fallback::tests::returns_last_error_when_all_fail` test provides indirect coverage (the error path is exercised). Full behavioral proof of CF propagation would require a dedicated test with a `CfFailingExtractor`.

**Confidence target:** PARTIAL — compilation proves the wiring; behavior of CF error preference over last-error is structurally verified but not tested by an assertion.

---

## Scenario 10: IframelyExtractor parses API response and handles HTTP errors

**Claim:** `IframelyExtractor` correctly maps iframely's `meta.title`, `meta.description`, and `links.thumbnail[0].href` to `UrlMetadata`, and returns an error on non-200 responses.

**Evidence type:** Test output (mock HTTP server via axum in-process)

**Command:**
```
cargo test -p boopmark-server -- iframely::tests --nocapture
```

**Expected result:**
```
test adapters::metadata::iframely::tests::parses_iframely_response ... ok
test adapters::metadata::iframely::tests::returns_error_on_api_failure ... ok
```

**Confidence target:** PROVEN

---

## Scenario 11: OpengraphIoExtractor parses API response and handles HTTP errors

**Claim:** `OpengraphIoExtractor` correctly maps `hybridGraph.title`, `hybridGraph.description`, and `hybridGraph.image` to `UrlMetadata`, and returns an error on non-200 responses. The fallback router handles the encoded URL in the path.

**Evidence type:** Test output (mock HTTP server via axum fallback router)

**Command:**
```
cargo test -p boopmark-server -- opengraph_io::tests --nocapture
```

**Expected result:**
```
test adapters::metadata::opengraph_io::tests::parses_opengraph_io_response ... ok
test adapters::metadata::opengraph_io::tests::returns_error_on_api_failure ... ok
```

**Confidence target:** PROVEN

---

## Scenario 12: URL validation rejects private/local addresses and non-http schemes

**Claim:** `validate_public_url` (called by both `IframelyExtractor` and `OpengraphIoExtractor` before forwarding URLs to third-party APIs) rejects localhost, private RFC-1918 ranges, `.local`/`.internal` hostnames, URLs with credentials, and non-http/https schemes.

**Evidence type:** Compilation (function exists and is called in both adapters)

**Supporting evidence:** The function is implemented in `metadata/mod.rs` and called as `super::validate_public_url(&url)?` in both `iframely.rs` and `opengraph_io.rs`. A dedicated unit test block for `validate_public_url` was not added in the implementation. The iframely and opengraph.io tests use `https://medium.com/some-article` as the input URL, exercising the happy path through the validator.

**Command:**
```
cargo test -p boopmark-server -- iframely::tests::parses_iframely_response opengraph_io::tests::parses_opengraph_io_response --nocapture
```

**Expected result:** Both tests pass, confirming the validator accepts valid public URLs. Private URL rejection behavior is structurally guaranteed by the implementation but lacks assertion tests.

**Confidence target:** PARTIAL — happy path is proven; rejection paths (private IPs, credentials, bad schemes) are not covered by tests in the current implementation.

---

## Scenario 13: MetadataFallbackBackend config parses all three variants

**Claim:** `METADATA_FALLBACK_BACKEND=iframely` maps to `MetadataFallbackBackend::Iframely`, `=opengraph_io` maps to `::OpengraphIo`, and any other value (including unset) maps to `::None`.

**Evidence type:** Test output

**Command:**
```
cargo test -p boopmark-server -- config::tests::metadata_fallback_backend --nocapture
```

**Expected result:**
```
test config::tests::metadata_fallback_backend_default_is_none ... ok
test config::tests::metadata_fallback_backend_parses_iframely ... ok
test config::tests::metadata_fallback_backend_parses_opengraph_io ... ok
```

**Confidence target:** PROVEN

---

## Scenario 14: BookmarkService skips screenshot when CF challenge is detected

**Claim:** In `BookmarkService::create`, when `metadata.extract()` returns an error containing `CF_CHALLENGE_MSG`, `cf_blocked` is set to `true` and the screenshot fallback branch is skipped, leaving `image_url` as `None`.

**Evidence type:** Code structure + compilation

**Supporting evidence:** The `cf_blocked` flag is set at line 69 of `bookmarks.rs` (`cf_blocked = e.to_string().contains(CF_CHALLENGE_MSG)`), and the screenshot branch guard at line 74 is `if input.image_url.is_none() && !cf_blocked`. No dedicated unit test exercises this path (the existing `BookmarkService` tests use `NoopMetadata` which always returns `Ok`). Proof is structural.

**Command:**
```
cargo build -p boopmark-server
```

**Expected result:** Clean build. The guard compiles and is reachable. Full behavioral proof would require a test with a `CfBlockedMetadata` mock that returns `Err(DomainError::Internal(CF_CHALLENGE_MSG.to_string()))`.

**Confidence target:** PARTIAL — the wiring is proven by compilation; the screenshot-skip behavior is not asserted by any test.

---

## Scenario 15: Main wiring — FallbackMetadataExtractor always wraps the chain

**Claim:** `main.rs` always constructs a `FallbackMetadataExtractor` regardless of config, using it as the single `MetadataExtractor` for both `BookmarkService` and `EnrichmentService`.

**Evidence type:** Compilation

**Command:**
```
cargo build -p boopmark-server
```

**Expected result:** Clean build. The `FallbackMetadataExtractor` type is used for both `metadata` and `metadata_for_enrichment` in `main.rs`. If the wiring were broken (wrong type, missing `Arc::clone`), the compiler would reject it.

**Confidence target:** PROVEN

---

## Summary

| Scenario | Confidence | Key command |
|---|---|---|
| 1. Compilation | PROVEN | `cargo build -p boopmark-server` |
| 2. Clippy clean | PROVEN | `cargo clippy -p boopmark-server -- -D warnings` |
| 3. Full 129-test suite | PROVEN | `cargo test -p boopmark-server` |
| 4. CF title heuristic | PROVEN | `cargo test ... html::tests::detects_cloudflare_challenge_by_title` |
| 5. CF verification text heuristic | PROVEN | `cargo test ... html::tests::detects_cloudflare_challenge_by_verification_text` |
| 6. CF no false positives | PROVEN | `cargo test ... html::tests::does_not_flag` |
| 7. HTML og:/twitter: parsing | PROVEN | `cargo test ... html::tests` |
| 8. Fallback chain ordering | PROVEN | `cargo test ... fallback::tests` |
| 9. CF signal propagation in chain | PARTIAL | Structural; no dedicated assertion |
| 10. Iframely adapter | PROVEN | `cargo test ... iframely::tests` |
| 11. OpengraphIo adapter | PROVEN | `cargo test ... opengraph_io::tests` |
| 12. URL validation rejects private | PARTIAL | Happy path only; rejection paths untested |
| 13. Config parsing all 3 variants | PROVEN | `cargo test ... config::tests::metadata_fallback_backend` |
| 14. Screenshot skip on CF block | PARTIAL | Structural; no mock test |
| 15. Main wiring always wraps | PROVEN | `cargo build -p boopmark-server` |

**PARTIAL gaps to consider:** Scenarios 9, 12, and 14 are the three behaviors that have no direct assertion. An executor could address them by adding:
- A `CfFailingExtractor` test in `fallback.rs` asserting the CF error is returned even when a subsequent extractor also fails generically.
- Unit tests for `validate_public_url` covering `localhost`, `192.168.x.x`, `file://`, and credential-bearing URLs.
- A `CfBlockedMetadata` mock in `bookmarks.rs` tests asserting `image_url` remains `None` after `BookmarkService::create`.
