# Metadata Fallback Design

## Problem

Bookmark preview images for Cloudflare-protected sites (e.g., Medium) show the CF anti-bot challenge page instead of the article's og:image. The current `HtmlMetadataExtractor` fetches with `reqwest` using a custom User-Agent, which CF blocks. The screenshot fallback then captures the challenge page as the bookmark image.

## Design

### Approach: Tiered Metadata Extraction with Third-Party Fallback

Use the existing `MetadataExtractor` trait with new adapter implementations and a composite extractor that chains them with fallback behavior.

### New Components

**`FallbackMetadataExtractor`** — a composite `MetadataExtractor` that holds a `Vec<Box<dyn MetadataExtractor>>` and tries each in order, returning the first successful result. If all fail, returns the last error. Fallback only triggers on errors, not on missing fields — if the first extractor returns `Ok` with `image_url: None`, that result stands.

**`IframelyExtractor`** — implements `MetadataExtractor`, calls the iframely.com API with a configured API key. Returns `UrlMetadata` parsed from the iframely JSON response.

**`OpengraphIoExtractor`** — implements `MetadataExtractor`, calls the opengraph.io API with a configured API key. Returns `UrlMetadata` parsed from the opengraph.io JSON response.

### CF Challenge Detection

The existing `HtmlMetadataExtractor` is updated to detect Cloudflare challenge pages and return an error instead of silently returning empty metadata. Detection heuristics:

- `cf-mitigated: challenge` response header (checked before consuming the response body)
- Response body contains "Performing security verification" or "Just a moment..."
- Page `<title>` is "Just a moment..."

When detected, return an error so the `FallbackMetadataExtractor` knows to try the next extractor in the chain. Any error from an extractor triggers fallback to the next one.

### Challenge-Aware Screenshots

After the screenshot bytes are returned from `ScreenshotProvider::capture()`, `BookmarkService` checks whether the captured page was a CF challenge. Since the screenshot provider is an external sidecar (Playwright HTTP service), detection happens by inspecting the returned image — but a simpler approach is to check whether the metadata extraction already identified a CF challenge: if the `HtmlMetadataExtractor` returned a CF-blocked error, skip the screenshot entirely and leave `image_url` as `None` (the UI already renders a placeholder emoji). This avoids modifying the screenshot sidecar.

### Configuration

Follows the existing `*_BACKEND` env var convention:

- `METADATA_FALLBACK_BACKEND=iframely` or `opengraph_io` — selects which fallback adapter to use. Unset means no fallback (current behavior).
- `IFRAMELY_API_KEY` — required when `METADATA_FALLBACK_BACKEND=iframely`
- `OPENGRAPH_IO_API_KEY` — required when `METADATA_FALLBACK_BACKEND=opengraph_io`

### Wiring

At startup, build the extractor chain based on config:

1. Always start with `HtmlMetadataExtractor`
2. If `METADATA_FALLBACK_BACKEND` is set, append the corresponding extractor
3. Wrap in `FallbackMetadataExtractor`
4. Inject into `BookmarkService` as the single `MetadataExtractor`

The `Bookmarks` enum in `web/state.rs` and `EnrichmentService` currently hardcode `HtmlMetadataExtractor` as the generic type parameter. To keep the type uniform regardless of config, always wrap in `FallbackMetadataExtractor` — even when no fallback backend is configured (chain of one). This way the concrete type is always `FallbackMetadataExtractor` and the enum/service types only change once.

### What Doesn't Change

- The `MetadataExtractor` trait itself
- `BookmarkService` — still receives a single `MetadataExtractor`, unaware of the chain
- The domain layer
- The existing `UrlMetadata` struct

### Testing

- `FallbackMetadataExtractor` gets unit tests with fake extractors (first returns error, second succeeds — verify chaining)
- `IframelyExtractor` and `OpengraphIoExtractor` tested with mock HTTP responses
- `BookmarkService` tests unchanged — they already use a mock `MetadataExtractor`
- CF challenge detection tested with sample challenge HTML
