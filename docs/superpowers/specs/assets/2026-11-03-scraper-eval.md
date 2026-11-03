# Scraper evaluation: Obscura and Firecrawl on the unfetchable bookmarks

Companion to [`2026-11-03-page-capture-design.md`](../2026-11-03-page-capture-design.md), section "Which adapter ships". Measured on 2026-11-02 from Chris's Mac. The harness is in [`scripts/eval/scraper/`](../../../../scripts/eval/scraper/).

## Result

Firecrawl returns the full article for 26 of the 84 pages. Obscura returns 6, and the server's own fetch returns 2. No candidate gets a member-only Medium story: Medium sends only a preview of about 1,900 characters to a reader that is not signed in. 49 of the 84 pages are member-only stories, so only the browser capture in the page-capture design can store their full text.

Recommendation: ship the Firecrawl adapter as the `PageScraper` fallback, and ship the extension capture for member-only stories. Do not ship Obscura.

## Inputs

The production Settings panel (`/settings/search-index`) reported 83 of 341 bookmarks without article text at the time of the run. No API lists these bookmarks, so `find_unfetched.py` uses the search API. The search returns a snippet for a bookmark only when the lexical leg matched it and its article text is stored (`search_repo.rs`, the `snippet` CASE). The script searches each bookmark by its own title. A hit without a snippet has no article text. `recheck.py` repeats the search for those hits with a quoted title phrase, which forces a lexical match.

The result is 84 bookmarks: the 83 of the panel and 1 false positive that the API cannot identify. The domain counts agree with the panel for its five listed domains (medium.com 43, generativeai.pub 5, agentnativedev.medium.com 3, levelup.gitconnected.com 3, builders.ramp.com 2). The URLs stay in `scripts/eval/scraper/data/`, which git ignores.

## Method

Each candidate fetched each page once, and the server's own Readability code extracted the text. The harness compiles `server/src/adapters/article/readability.rs` and its dependencies unchanged into a separate binary (`scripts/eval/scraper/extract/`, by `#[path]`).

| Candidate | How it ran |
|---|---|
| baseline | `HtmlArticleFetcher::fetch`, the production fetch with its SSRF guard, from the Mac |
| Obscura | `obscura --stealth fetch <url> --dump html --timeout 30`, nixpkgs build 0.2.2 with the `render` and `stealth` features, under `/usr/bin/time -l` for peak RSS |
| Firecrawl | `firecrawl scrape <url> --format rawHtml`, CLI 1.26.24, free plan, one request every 7 s (the plan allows about 10 a minute) |

Quality classes, in order of precedence:

| Class | Rule |
|---|---|
| Dead link | The server's fetch got HTTP 404 or 410, or the host does not resolve. No scraper can recover the page. |
| Block page | A Cloudflare challenge (the server's `is_cloudflare_challenge`), a challenge text, or HTTP 403 |
| Preview or paywall stub | The HTML has Medium's `"isLocked":true`, or the text is shorter than 1,500 characters |
| Full | Everything else with text |
| Empty or error | No text, or the candidate failed |

The lock marker is necessary. Without it, 49 Medium previews of 1,500 to 2,400 characters counted as full articles in the first Firecrawl pass. Most of these previews end in the middle of a sentence with "…".

## Summary

| Candidate | Full text /84 | Preview or paywall stub | Block page | Dead link | Empty or error | Median latency | p95 latency | Memory | Cost per page |
|---|---|---|---|---|---|---|---|---|---|
| Baseline (Mac IP) | 2 | 3 | 65 | 6 | 8 | 0.1 s | 1.0 s | in the app | none |
| Obscura | 6 | 3 | 69 | 6 | 0 | 5.3 s | 5.7 s | 122 MB median, 419 MB max per page; 19.5 MB idle | a second Fly machine, about $6.70 a month |
| Firecrawl | 26 | 51 | 0 | 6 | 1 | 3.2 s cold, 0.8 s cached | 5.3 s cold, 0.9 s cached | none on Fly | 1.2 credits: $0.0045 (Hobby), $0.0012 (Standard), $0 on the free plan |

The 51 Firecrawl previews are 49 member-only Medium stories and 2 YouTube videos. The Firecrawl error is a Google Drive file larger than its 50 MiB limit. In production the baseline gets 0 of these pages, because they are the pages that production failed to fetch. From the Mac it got the 2 openai.com pages.

## Per domain

Full-text counts. "Other" names the class of the remaining pages.

| Domain | Pages | Baseline full | Obscura full | Firecrawl full | Obscura other | Firecrawl other |
|---|---|---|---|---|---|---|
| medium.com | 43 | 0 | 0 | 10 | block 43 | preview 33 |
| generativeai.pub | 5 | 0 | 0 | 2 | block 5 | preview 3 |
| agentnativedev.medium.com | 3 | 0 | 0 | 0 | block 3 | preview 3 |
| levelup.gitconnected.com | 3 | 0 | 0 | 0 | block 3 | preview 3 |
| builders.ramp.com | 2 | 0 | 2 | 2 | - | - |
| drive.google.com | 2 | 0 | 1 | 1 | preview 1 | empty 1 |
| openai.com | 2 | 2 | 0 | 2 | block 2 | - |
| youtube.com | 2 | 0 | 0 | 0 | preview 2 | preview 2 |
| ai-engineering-trend.medium.com | 1 | 0 | 0 | 1 | block 1 | - |
| ai.gopubby.com | 1 | 0 | 0 | 1 | block 1 | - |
| ai.plainenglish.io | 1 | 0 | 0 | 0 | block 1 | preview 1 |
| blog.stackademic.com | 1 | 0 | 0 | 0 | block 1 | preview 1 |
| boopmark-ai-fallback-20260830.invalid | 1 | 0 | 0 | 0 | dead 1 | dead 1 |
| codefarm0.medium.com | 1 | 0 | 0 | 0 | block 1 | preview 1 |
| codingplainenglish.medium.com | 1 | 0 | 0 | 0 | block 1 | preview 1 |
| dl.acm.org | 1 | 0 | 0 | 1 | block 1 | - |
| georgeguimaraes.com | 1 | 0 | 0 | 0 | dead 1 | dead 1 |
| github.com | 1 | 0 | 0 | 0 | dead 1 | dead 1 |
| githubdaily.medium.com | 1 | 0 | 0 | 0 | block 1 | preview 1 |
| guycoding.medium.com | 1 | 0 | 0 | 0 | block 1 | preview 1 |
| ishmeetbindra.com | 1 | 0 | 0 | 0 | dead 1 | dead 1 |
| navanathjadhav.medium.com | 1 | 0 | 0 | 1 | block 1 | - |
| pub.towardsai.net | 1 | 0 | 0 | 0 | block 1 | preview 1 |
| railcode.dev | 1 | 0 | 1 | 1 | - | - |
| repofortify.com | 1 | 0 | 1 | 1 | - | - |
| rizqimulki.com | 1 | 0 | 0 | 0 | dead 1 | dead 1 |
| skillsmp.com | 1 | 0 | 0 | 0 | dead 1 | dead 1 |
| testing.googleblog.com | 1 | 0 | 1 | 1 | - | - |
| towardsaws.com | 1 | 0 | 0 | 1 | block 1 | - |
| www.perplexity.ai | 1 | 0 | 0 | 1 | block 1 | - |

## Findings

1. Cloudflare blocks Obscura on every Medium-family page (65 of 65) and on openai.com, dl.acm.org and perplexity.ai. The stealth build and a 15 s settle wait did not change the result on a test page.
2. Firecrawl passes every Cloudflare challenge in the set. On the Medium family it returns the full story when the story is free (16 pages) and the preview when it is member-only (49 pages).
3. 49 of the 84 pages are member-only stories. A scraper sees what a signed-out reader sees, so no scraper can store their full text. The extension capture runs in the member's own browser and gets the full text.
4. 6 pages are dead links and 4 are not articles (2 YouTube videos, 2 Google Drive files). Thus the ceiling for any server-side scraper is 74 pages, of which 25 are free articles. Firecrawl gets all 25, plus the text of 1 Google Drive document.
5. The direct fetch can also receive a member-only preview with HTTP 200. From the Mac it got 2 of them, of 1,050 and 1,140 characters. The fallback design accepts a direct result of 300 characters or more, so it stores such a preview as a full article.

## Memory and the Fly machine

The app runs on one `shared-cpu-1x` machine with 512 MB (`fly.toml`). Obscura needs 122 MB per page at the median and 419 MB on a YouTube page, so it cannot share that machine. It needs a separate Fly app. A `shared-cpu-1x` machine with 1 GB costs $6.70 a month at the base price (`iad`), and `sjc` costs more. A machine that stops when idle costs less, but every first scrape then waits for a cold start. At this account's volume (the 84 pages, then the new saves that the direct fetch cannot get), the fixed cost is several cents per page, which is more than Firecrawl.

Firecrawl runs nothing on Fly. The free plan gives 1,000 credits a month, about 840 pages at 1.2 credits per page.

## Limitations

- The Mac has a residential IP, and Fly has a datacenter IP. Cloudflare treats a datacenter IP more strictly, so the Obscura and baseline counts are upper bounds for production. The baseline confirms this: from the Mac it fetched 2 openai.com pages that production failed to fetch. Firecrawl fetches from its own network, so its numbers do not depend on the host.
- No Fly machine was created for this evaluation.
- nixpkgs packages Obscura 0.2.2. The latest release is 0.2.6.
- The second Firecrawl pass, with the lock marker, gets Firecrawl's cache. The cold latency comes from the first pass.
- One of the 84 bookmarks has article text in production, but the search API cannot identify it.

## Recommendation

1. Choose Firecrawl for `PageScraper`. It got no block page on any of the 84 pages, it gets the full text of all 25 reachable free articles, and it needs no second machine. The free plan covers this account's volume. On a paid plan the cost is about $4.50 per 1,000 pages (Hobby).
2. Do not choose Obscura. It gets 6 of 84 pages from a residential IP and fewer from Fly, and it needs a second Fly app.
3. Make sure that the page-capture design treats a member-only preview as a preview, in both the direct path and the scraper path. Detect `"isLocked":true` in the HTML, before the 300-character check. A preview can stay as article text until a capture replaces it, but it must not count as a full fetch.
4. Use the extension capture for the 49 member-only stories. No server-side option gets their full text.

## Spend

Firecrawl used 201 credits of the free plan (1,025 before, 824 after) for 171 scrapes: two passes over 84 pages and three test scrapes. The cash cost is $0. The search API calls in `find_unfetched.py` (about 410) each embedded a short query with OpenAI. That cost is less than one cent.

## How to run again

Run the commands from `scripts/eval/scraper/`. The prod API key goes into the environment of one command only:

```sh
BOOPMARK_API_KEY=... python3 fetch_inputs.py      # bookmarks and the Settings panel
BOOPMARK_API_KEY=... python3 find_unfetched.py    # data/unfetched.json
BOOPMARK_API_KEY=... python3 recheck.py
(cd extract && cargo build --release)
export OBSCURA=$(nix build nixpkgs#obscura --no-link --print-out-paths)/bin/obscura
python3 run.py baseline; python3 run.py obscura; python3 run.py firecrawl; python3 run.py idle-rss
python3 report.py                                  # the tables in this file
```
