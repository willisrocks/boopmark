# Chrome extension development QA

The acceptance contract is [the extension spec](superpowers/specs/2026-08-27-chrome-extension-design.md). A local pass is **not** the required production E2E pass.

## Setup and supplementary local checks

```sh
npm ci
npx agent-browser install
npm run test:extension
node --test scripts/e2e/chrome-fixture.test.mjs
npm run extension:fixture
```

In another terminal:

```sh
npm run extension:browser
```

The launcher uses headed Chrome for Testing, `extensions/chrome/` as an unpacked extension, and an ignored dedicated profile under `.cache/boopmark-extension/`. Profiles must stay outside `test-results/`, which Playwright clears before regression runs; the launcher rejects that unsafe location. Set `AGENT_BROWSER_EXECUTABLE_PATH` for another extension-capable Chromium. Override `CHROME_EXTENSION_PROFILE` and `CHROME_EXTENSION_SESSION` to isolate concurrent runs. Never use an everyday browser profile. Use the same launcher for subsequent agent-browser commands, e.g. `npm run extension:browser -- snapshot -i`.

Connect the local fixture at `http://127.0.0.1:4011` with the deliberately public fixture key `extension-fixture-key`. This fixture does not connect to Boopmark or an AI provider. Its article is `/article`; add a unique query/fragment for capture tests.

Connection permissions use an explicit effective port (including `:443`/`:80`) because [Chrome match patterns otherwise include every port](https://developer.chrome.com/docs/extensions/develop/concepts/match-patterns). Verify the granted pattern and that another port is not included. Use a fresh profile for this check: a broader grant from an older development build is not narrowed merely by requesting a subset.

Pin Boopmark in the dedicated test browser. On macOS with existing Accessibility authorization, the native toolbar bridge is:

```sh
osascript scripts/e2e/chrome-toolbar.applescript Boopmark
```

It targets Chrome for Testing's native toolbar, not webpage buttons. If multiple test processes exist, pass an exact verified browser PID as the second argument; resolve it from this run's dedicated `--user-data-dir`, never a regular browser profile. The helper validates Chrome for Testing's name/bundle and refuses ambiguous process selection. If unavailable, record a manual toolbar click in the same headed browser. Do not substitute a popup opened as a tab. Capture the actual action popup's agent-browser target and use snapshot/label interactions there.

For a true outside-click dismissal check, use the bounded native page helper with that same exact PID while the real action popup is open:

```sh
CHROME_EXTENSION_SESSION=boopmark-extension-local-fdcc \
CHROME_EXTENSION_PROFILE="$PWD/.cache/boopmark-extension/local-qa-profile" \
node scripts/e2e/chrome-dismiss-popup.mjs EXACT_BROWSER_PID
```

The wrapper verifies this checkout's exact local QA session/profile, Chrome command line, unpacked-extension path, and static loopback fixture URL before invoking the native click. The AppleScript then clicks the page area (or the center of that exact main window when Chrome hides the page AX node while the popup owns focus). Reopen with the native toolbar action and verify the draft and request counts. Do not call the AppleScript directly, and never use this helper on production or an interactive page: a native click can activate content at its click point. A synthetic click sent to the underlying page target does not prove popup dismissal.

Enable **Developer mode** in `chrome://extensions` in this dedicated profile and verify “On, extension enabled” after reloading. A command-line-loaded extension can initially appear enabled but become disabled on reload if Developer mode is off. Do not change an everyday browser profile.

For **supplementary** UI/lifecycle testing when native control is unavailable, `node scripts/e2e/chrome-popup.mjs` invokes Chrome's documented `action.openPopup()` on the active extension worker. Reload the extension first and return to the article before invoking it. This opens a real popup, not a popup tab, but it is still **not** an actual toolbar click and does not satisfy AC14.

After any extension code change, explicitly reload Boopmark with its `chrome://extensions` Reload control in the dedicated profile (Developer mode on). Closing and relaunching is still required when startup flags change, but is not sufficient proof for a same-version code change: Chrome may retain the previous MV3 worker. Verify a changed runtime behavior or marker before recording evidence.

When startup flags change, relaunch with:

```sh
npm run extension:browser -- close
npm run extension:browser
```

## Deterministic fault injection

The local-only fixture exposes `GET /__state`: `requests` counts every API attempt before authentication (method/path and HTTP status, pending, or aborted only); `events` retains authenticated write query strings/submitted payloads; `bookmarks` contains this process's saved fixtures. Thus rejected credentials and read-only validation requests are observable without logging authorization headers. `POST /__control` accepts `{ "mode": "…", "reset": true }`; reset only clears this process's in-memory fixtures and both logs. Modes: `normal`, `slow-suggest`, `suggest-error`, `empty`, `partial`, `revoked`, `save-error`, `save-unknown`, `slow-save`.

Optional `suggestionDelayMs` and `saveDelayMs` accept integer milliseconds from 0 to 15000 (defaults 1500 and 2500). Use about 10000 for multi-command manual/agent-browser races. Slow modes delay only their matching suggestion/create operation; each request retains the mode/delay captured at dispatch even if controls change later. Invalid controls fail before any reset or settings change. Restart the local fixture after editing its script; an existing process does not hot-reload.

Use these modes for edits/clears during enrichment, save during enrichment, duplicate clicks, definite errors, ambiguous writes, popup close/reopen, browser restart, and disconnect cleanup. The unknown mode stores the fixture then drops the response before headers, which can cause Chromium itself to replay the POST. Verify every attempt carries the same idempotency key, the server/fixture stores exactly one bookmark, a completed identical replay returns the original record, and popup reopen/restart initiates no additional request. The real server returns 503 for a matching replay while the original is still pending, 201 with the original record after completion, and 409 for a mismatched payload. Count `/api/v1/bookmarks` separately from `/suggest`, and assert the create query is empty. Evidence may record the presence and equality of a sanitized key fingerprint, never credentials or authorization headers.

The migrated Postgres repository regression is intentionally ignored by the ordinary suite. Run it explicitly against the checkout-owned devproxy database:

```sh
set -a; source .env; set +a
cargo test -p boopmark-server adapters::postgres::bookmark_repo::idempotency_tests::durable_claims_are_atomic_scoped_and_headerless_creates_remain_distinct -- --ignored --exact
```

This proves atomic claim ownership, pending behavior, completed replay, mismatch rejection, account scope, and headerless compatibility at the repository layer. The Chromium fixture independently proves the two-wire transport behavior. `tests/e2e/idempotency.spec.js` exercises the real local HTTP API with bearer authentication and verifies keyed create/replay, conflict, invalid-header, and headerless behavior. These local layers still do not substitute for the post-deployment production gate.

For existing web regressions, copy the main checkout's `.env` as instructed in `AGENTS.md`, start the existing Docker/devproxy runtime, then run:

```sh
npx playwright test tests/e2e/suggest.spec.js
```

The dedicated web server remains on port 4010. Bootstrap reuses a verified devproxy-managed database belonging to this checkout, because devproxy 0.4.x `up` is not idempotent. Before starting the server it removes only the disposable `e2e@boopmark.local` identity and its dependent test data, making repeated runs deterministic without touching normal local accounts. An unrelated listener on 5434 causes a failure, not silent reuse. Never point existing API-key suites at production: their teardown is broad.

## Production gate

Use the same headed sideloaded extension, but a separate dedicated profile and an authorized Boopmark bearer key. Enter credentials securely; never put them in shell arguments, screenshots, traces, committed files, or the report. Do not extract simulator credentials or create/rotate keys to bypass missing provisioning.

Preflight an unlocked desktop and a usable native or explicitly recorded manual toolbar-click path. If either is unavailable, mark the headed gate blocked; do not unlock the desktop or change OS permissions as part of the test.

From the repository root, in a terminal reserved for this production QA session:

```sh
export CHROME_EXTENSION_SESSION=boopmark-extension-prod-qa
export CHROME_EXTENSION_PROFILE="$PWD/.cache/boopmark-extension/production-profile"
export CHROME_EXTENSION_HEADED=true
npm run extension:browser -- open 'https://example.com/?boopmark-qa=REPLACE_WITH_UNIQUE_RUN_ID#capture'
```

Use a unique session name/profile if another worktree or run is active. Replace the example with a public article and unique run marker suitable for metadata enrichment. Connect the extension to `https://boopmark.com`, not the local fixture. The fixture process is not required for this production journey. Keep these environment values in the same terminal for subsequent `snapshot -i`, `close`, and `open` commands so they target this dedicated browser.

At each relevant development checkpoint, run `npm run test:extension`, explicitly reload Boopmark in `chrome://extensions`, and verify the current worker before repeating the live journey. Relaunch only as additionally needed for changed startup flags:

```sh
npm run extension:browser -- close
npm run extension:browser -- open 'https://example.com/?boopmark-qa=REPLACE_WITH_NEW_RUN_ID#capture'
```

Verify the unpacked path/version in the browser and record the launcher's on-disk build hash; the printed hash and a same-version browser relaunch do not prove that a previously running worker was replaced. Use the actual toolbar action on the article, then inspect/interact with its popup. Bound production captures to these checkpoints; fault injection remains local. The Codex in-app browser, headless mode, static previews, and popup-as-tab navigation are not substitutes for this gate.

Follow the spec's complete live journey: read-only AI-settings verification; unique public fixture query/fragment; actual toolbar capture; automatic pre-save enrichment; edit plus intentional clear; one logical idempotent create without `suggest=true`; confirmed 201 and exact bookmark ID; refreshed production library with exactly one fixture record; dismiss/reopen; explicitly authorized exact-ID cleanup. Record only that an idempotency key was present, not its value. AI evidence must distinguish enrichment from scrape-only fallback. The spec permits a fresh nonempty `/suggest` tags response as a source-backed inference because the inspected server produces suggestion tags exclusively from successful LLM output; record the deployed-source limitation rather than claiming direct provider logs. Missing evidence is not a local-test substitution.

## Evidence checklist

- Build/commit and content hash, extension version and ID, CLI and Chrome versions.
- Test fixture URL, timestamps, before/after screenshots (no credentials).
- Actual toolbar action method; automatic suggestion count and AI provenance.
- Reviewed values, logical create operation, sanitized idempotency-key presence/replay equality, wire-attempt count/query/status, exact bookmark ID, and refreshed library result.
- Dirty/stale/clear and save lifecycle tests; keyboard/announcements/layout review.
- Cleanup approval/result and residual records, or explicit no-production-mutations statement.
- AC1–AC15 verdicts with their actual evidence; never mark unexecuted checks passed.

For an offline design/markup check, run `node scripts/e2e/chrome-render-preview.mjs`. It renders the real HTML/CSS with illustrative fields (no extension JavaScript), asserts a labeled form, keyboard order, no horizontal overflow at 400px, and a visible Save action. Output is under ignored `test-results/chrome-visual-preview/`. This is static visual evidence, not a capture/save E2E pass.

For the actual open action popup, `node scripts/e2e/chrome-popup-control.mjs accessibility` reads Chrome's accessibility tree and emits only the capture form's known labels, roles, focused/live properties, and status text. It refuses setup/key screens and omits field values. Combine this with `press Tab` / `press Shift+Tab` and observations before/after loading and error transitions. This is programmatic accessibility evidence, not a claim that spoken screen-reader output was observed. The command's projection tests alone do not satisfy AC13.

## Current live production evidence (2026-08-28)

This is the authoritative live record for the current headed sideloaded run. It supersedes the prerequisite conclusions in the historical section below without rewriting that history.

- The production headed build hash is `b5a2e845b5d25f43`; agent-browser is `0.35.1`; Chrome for Testing is `152.0.7977.64`; and the unpacked extension ID is `ggfienpplnccomboiahcllfpakbopane`.
- The native toolbar bridge clicked the exact Boopmark action in the dedicated production-profile Chrome process. The real action popup was inspected through its scoped CDP target; it was not opened as a tab.
- The user-authorized credential passed a read-only `GET https://boopmark.com/api/v1/bookmarks?limit=1` with HTTP 200 and the expected list shape. A read-only `GET /settings` confirmed AI enabled, provider key configured, and model configured; no settings changed. The authorized temporary secret cache was mode `0600` and outside the repository. Its path, key, and authorization headers are intentionally omitted from this record and were not put in command arguments, screenshots, traces, or committed files.
- Fixture 1 remains untouched and must be preserved: exact-ID read verification found `b1771eee-0367-4184-8855-24242824cfd0` at `https://example.com/?boopmark-qa=20260828-1#capture`. Its original create request/status and pre-edit AI response were not observed; do not reconstruct them from the stored record.
- Fixture 2, `https://example.com/?boopmark-qa=20260828-2#capture`, was observed from `15:00:59.113Z` through `15:05:08.285Z`. The initial title was the browser fallback `Example Domain`; description and tags were empty. One automatic `/suggest` returned HTTP 200 with nonempty tags `documentation`, `reference`, `example`, and `reserved-domain`. A title edit survived native Escape and reopen without another suggestion. Cancel produced zero creates and an exact-URL search found zero records.
- Fixture 3, `https://example.com/?boopmark-qa=20260828-3#capture`, was observed from `15:07:18.642Z` through `15:09:33.857Z`. One automatic `/suggest` returned HTTP 200 with fresh nonempty tags. The reviewed title was `Boopmark Chrome E2E 20260828-3`; description was intentionally cleared to `""`; tags input was `documentation, , example`. Exactly one Save produced one HTTP 201 create with an empty query string, bookmark ID `5c25938c-9fd8-426b-b374-b75f0a656240`, visible `Saving` then `Saved`, and automatic close. Exact-ID and exact-URL reads verified the title, URL, empty description, and parsed tags `["documentation", "example"]`.
- The production web library was loaded and reloaded in the same standalone agent-browser profile. The authenticated document request count was 2; the exact card had `present`, matching title/link, and cleared-description checks true, with tags `documentation` and `example`. Evidence is saved as `test-results/chrome-production/capture-3-library.png`.
- After explicit user authorization, only fixture 3 was cleaned up: its exact ID was reverified, `DELETE` returned 204, the exact-ID read returned 404, and exact-URL search found zero remaining records. Fixture 1 was not deleted or modified.
- The fresh nonempty suggestion tags are the source-backed successful-LLM inference permitted by the spec: no deployed-source hash or direct provider logs were verified, so this record does not claim either. The server-side provider key remained server-side.
- The real popup fit 400px without horizontal overflow; Save was visible; all four capture fields had labels; metadata/save/connection status regions exposed `role="status"` and `aria-live="polite"`; and the fatal error region exposed `role="alert"`. Keyboard and accessibility-tree behavior remain pending. Capturing the unsupported `chrome://version` page displayed a URL error and disabled Save and Autofill, as intended; this was not a browser-compatibility failure.
- At this historical production checkpoint, the granted host access was `https://boopmark.com:443/*`; `contains443` was true and `contains8443` was false. The then-remaining isolated lifecycle checks were completed in the later local checkpoint below. Keyboard, accessibility-tree, and layout evidence was also completed later; a spoken screen-reader observation remains an explicit optional limitation.
- Credential-free screenshots are under `test-results/chrome-production/`: `manual-save-reopened.png`, `capture-2-autofilled.png`, `capture-3-before-autofill.png`, `capture-3-after-autofill.png`, `capture-3-reviewed.png`, `capture-3-saved.png`, and `capture-3-library.png`.

### Latest continuation checkpoint

The desktop subsequently locked again (`CGSSessionScreenIsLocked=Yes`). A separate local fault-test session/profile was launched and Boopmark pinned through Chrome's extension manager, but native toolbar control reported no accessible window. The supplementary popup helper also could not resolve an active worker. No unlock or OS-permission changes were attempted, and no local popup/fault pass is claimed. The user-authorized private credential cache is retained for remaining QA and must be deleted when testing finishes; its contents are never included in evidence. The latest extension test run passed all 64 tests, including observer, library-authentication, and local-control safety guards. `git diff --check` also passed.

A subsequent continuation identified Developer mode as the reason the local extension's worker was absent after reload. Enabling it in the isolated local profile restored “On, extension enabled” and the worker. The documented popup API was then retried after its normal focus request and Chrome rejected it with “Cannot show popup for an inactive window.” The desktop remained locked. This resolves the harness configuration issue but not the native-popup availability blocker; neither an unlock nor an OS-permission change was attempted.

Preparation completed during that continuation: request counting before authentication, bounded independently captured fixture delays, and sanitized real-popup accessibility-tree inspection. All 66 extension tests and 4 separate loopback-fixture tests passed, and `git diff --check` passed. These are harness/regression results, not live popup fault/accessibility passes. The extension runtime build is unchanged.

The third consecutive continuation revalidated the locked desktop and the advisor confirmed no further meaningful implementation work can advance the outstanding live gates without the actual popup. The goal is blocked, not complete, pending an unlocked desktop. The private credential cache remains owner-only for resuming QA; no further 1Password prompt is currently needed. Production happy-path and cleanup evidence above remain valid.

That blocker is now historical. The desktop subsequently unlocked and local popup QA resumed in the same isolated, agent-browser-launched headed profile. Native-toolbar automation and the scoped CDP popup helper are adapters within that session, not substitutes for agent-browser or for the real production gate. After the origin-switch permission safeguard was implemented, the unpacked extension was closed and relaunched from on-disk build `96d7df5870ac591b`; the exact Boopmark toolbar action opened the real popup for a loopback fixture, one automatic suggestion populated title, description, and tags, and the scoped accessibility projection confirmed labeled URL/title/description/tags controls plus a polite live metadata status. A delayed-suggestion run exposed “Fetching metadata…” through that polite live region while the editable fields and Add Bookmark remained available, then populated the fields. A suggestion-error run retained the browser-title fallback, kept the form editable/saveable, and exposed “Fixture unavailable” through the same live region. Each popup was discarded without a create, and the fixture was restored to normal mode. The full extension suite passed 69 tests and the loopback fixture suite passed 4 tests. Independent review found no remaining high-priority issue in the verified old-origin removal and rollback behavior. This local reloaded-build check does not replace AC14's next bounded production checkpoint, and the historical locked-desktop paragraphs above remain evidence of those earlier attempts only.

Further real-popup checks on that build verified an edited `javascript:` URL displayed the validation explanation, disabled Autofill and Save, cleared generated fields, and produced zero fixture requests. Saving while a suggestion was pending aborted the suggestion and produced one create with the exact reviewed title, intentional empty description, parsed tags, and empty query string. A delayed-save repeated-click attempt kept one create. The ambiguous-response fixture then exposed an important transport issue: one application Save could arrive twice when Chromium replayed a reusable HTTP/1.1 POST after the fixture committed and destroyed the socket before response headers. The worker itself dispatched one fetch and correctly retained an unknown marker with Save disabled and no replay on reopen, but two fixture bookmarks were stored. This is a failing exactly-once risk, not passing AC7 evidence or merely a fixture-counting anomaly. The fixture records were disposable and reset. Server-side idempotency using the durable extension operation ID is required before this fault case and the final production checkpoint can pass.

The idempotency implementation was then added and tested on on-disk build `d9cf924682dc1ce6`. A same-version browser relaunch initially kept the stale worker, which was detected because the sanitized fixture record lacked an operation group. After an explicit `chrome.runtime.reload()` of the exact Boopmark extension, a native toolbar click opened the real popup and one application Save caused two same-millisecond wire POST attempts when the first response was destroyed before headers. Both attempts carried the same sanitized operation group; the fixture completed one logical operation, returned HTTP 201 on replay, and stored exactly one bookmark with the reviewed title, description, tags, and URL. Reopening the real action popup initiated a fresh draft rather than another create. This passes the local ambiguous-transport portion of AC5/AC7 and establishes the explicit-reload requirement, but it does not satisfy AC14 until the server migration/code is deployed and the bounded production journey is rerun.

The same explicitly loaded build then closed the remaining local headed checks. Empty suggestions kept `Browser fallback title`, left optional fields empty, displayed the neutral no-metadata state, and kept Save enabled; partial suggestions kept the fallback title and filled only `Partial description`. In a 15-second delayed suggestion, the authored title `Authored delayed title` and an intentionally cleared description survived while untouched tags populated. On the verified static fixture/PID, the native outside-page helper dismissed that popup; reopening through the exact toolbar action restored those values, retained a single suggestion request, and produced zero creates. After a fixture reset and whole-browser restart, loading a fixture article without invoking Boopmark left events, requests, and bookmarks empty. An ambiguous Save was then recorded as unknown; closing and restarting the entire isolated browser preserved the disabled unknown state and submitted URL, sent no new request, and left one sanitized operation group/one bookmark. The minimal durable marker intentionally does not persist reviewed metadata across restart. Finally, a live 401 moved the popup to Reconnect with Save unavailable; restoring the fixture allowed reconnect, and Disconnect left no configured settings, operation, session draft, session key, or granted origin. The credential/storage portion of inspection returned only booleans/counts and never returned a credential value; the same bounded report also included known labels, status text, focus/layout, and granted-origin names.

At this checkpoint, `npm run test:extension` passed 77/77 tests, the standalone Chromium fixture suite passed 5/5 when granted local-listener access, the regular workspace `cargo test` passed 198 tests with the database regression correctly reported as one ignored test, the ignored migrated-Postgres regression passed 1/1 when run explicitly, and the complete committed Playwright suite passed 60/60 including the new HTTP idempotency regression. A consecutive state-sensitive transfer-suite run passed 17/17 after the bootstrap reset only the dedicated E2E identity, proving database reuse is repeatable rather than dependent on a one-time clean database. These are separate evidence layers; the local HTTP test does not inject a socket drop into the real Axum server, and no production replay is claimed. Two resilience limitations remain documented follow-ups: pending claims have no lease/owner/fencing recovery and can be stranded by a server crash, and deleting a completed bookmark nulls the operation's bookmark reference so a later replay of that cleaned-up key cannot return the original record. Both preserve duplicate prevention, but neither should be described as indefinite replay recoverability.

A read-only release-state check found `origin/main` and this detached worktree still at `68d7cbb`; no extension/idempotency branch exists remotely. Railway's active production deployment is successful and running but was created on 2026-08-27, before the 2026-08-28 idempotency implementation. Production health therefore does not prove migration `010` or the current create contract is deployed. No commit, push, deployment, migration, or new production bookmark was performed by this check.

## Initial implementation-run evidence (2026-08-27)

- `cargo build`: passed.
- `cargo test`: passed (16 CLI + 180 server tests after new backend tests).
- Eight new backend tests cover enabled/off/unconfigured/failing/partial enrichment and explicit clears on create.
- `npx playwright test tests/e2e/suggest.spec.js`: passed (1 test, dedicated port-4010 server).
- `npm run test:extension`: passed (57 tests, including parameterized empty/partial/failed enrichment, storage-access failure, and foreign-sender rejection).
- agent-browser 0.35.1 and Chrome for Testing 152.0.7977.64 installed; isolated headed local article loaded successfully.
- Popup DOM-mock regressions cover edit races, permission handling, reconnect/save errors, and immediate Save/Cancel dispatch. Worker regressions cover concurrent saves, restart recovery, canceled/reopened same-ID drafts, URL/connection changes, and disconnect removal failure/retry. Use the latest `npm run test:extension` output for the current count.
- Static capture preview passed design review against the exact web/iOS palette and original logo. The markup fixture passed 400px overflow, visible-action, field-label, and nine-step keyboard-focus assertions. Actual popup accessibility remains unverified.
- Headless popup-as-tab rendered the setup screen; the real permission prompt could not complete while the desktop was locked. That supplementary browser was closed; no connection pass is claimed.
- The unpacked extension was registered/enabled in the initial dedicated test profile, ID `ggfienpplnccomboiahcllfpakbopane`. After moving profiles outside Playwright's disposable output, a fresh headed launch of on-disk build `b5a2e845b5d25f43` loaded the local article and `chrome://extensions` showed “Boopmark” and “On, extension enabled.” Full functional sideload QA is still pending.
- The macOS GUI session is confirmed locked (`CGSSessionScreenIsLocked=1`). Native toolbar visibility is inconsistent; developer-API popup opening also reports an inactive window, including after a supported focus request. Actual headed toolbar/popup coverage is blocked until the user unlocks the desktop; no unlock or OS-permission change was attempted. Initial accessibility preflight was not proof of working toolbar control.
- Production bearer provisioning and AI-success evidence unavailable so far. No production requests, bookmarks, settings, or credentials changed by this run.

This section is an interim historical record from 2026-08-27, not a completion claim. Its locked-desktop, unavailable-bearer, unavailable-AI-evidence, and pending-local-check conclusions were true at that checkpoint and are superseded by the current live evidence above. The current remaining gate is the post-deployment production checkpoint; spoken screen-reader output remains an explicitly recorded optional limitation.

The resume instructions from that historical checkpoint are no longer operative. Its blocked and pending statements are historical; use the current live record and acceptance table above for completed isolated lifecycle/accessibility evidence and the one remaining post-deployment production gate.

## Acceptance audit in progress

| Criteria | Current evidence | Still required |
| --- | --- | --- |
| AC1 | The exact native toolbar action opened fresh production captures with query/fragment preserved; fixtures 2 and 3 each started one automatic suggestion and no pre-save bookmark. | No additional AC1 capture evidence is outstanding; fixture 1 remains preserved. |
| AC2 | Fixtures 2 and 3 each produced one live HTTP 200 `/suggest` with fresh nonempty tags. Per the inspected source, this is consistent with successful LLM enrichment; no deployed-source hash or direct provider logs were verified. | Confirm the deployed revision preserves the tags-only-from-successful-LLM invariant or obtain approved redacted provider-success evidence; until then treat this as a source-backed inference, not definitive provider proof. |
| AC3 | Counted backend tests prove off/unconfigured = zero provider calls; worker empty/partial/failed suggestion cases preserve fallback fields and save successfully. In the real popup, error retained the browser-title fallback and announced failure; empty retained the fallback title with empty optional fields and a neutral status; partial filled only the available description. All remained editable/saveable without a false success claim. | No additional AC3 behavior is outstanding; off/unconfigured provider-call evidence remains isolated by design rather than changing production settings. |
| AC4 | Fixture 2 title editing survived native Escape/reopen without a second suggestion; fixture 3 preserved the intentional description clear and reviewed tags. In a real-popup 15-second delay, an authored title and intentional description clear survived the response while untouched tags populated. Deterministic A→B→A and connection-generation tests reject stale responses. | No additional AC4 behavior is outstanding. |
| AC5 | Fixture 3 had one Save, one HTTP 201 create, an empty create query string, and the exact reviewed title/empty description/parsed tags. A local real-popup run saved during pending enrichment with the exact reviewed snapshot and aborted the late suggestion; a delayed-save repeated-click attempt produced one create. On explicitly reloaded build `d9cf924682dc1ce6`, the ambiguous transport case produced two wire attempts with the same operation group and exactly one stored bookmark. Deterministic tests cover one-key replay, payload mismatch, account isolation, and headerless compatibility. | Repeat the bounded journey after the server idempotency migration/code is deployed; do not infer deployed behavior from the local pass. |
| AC6 | Fixture 3's HTTP 201 ID was verified by exact-ID and exact-URL reads; the refreshed production library matched title/link/cleared description/tags, with two authenticated document requests. | No additional fixture-3 library evidence is outstanding. |
| AC7 | API and worker tests distinguish definite/ambiguous errors, retain reviewed drafts, and require explicit retry. The real popup displayed the unknown warning, disabled Save, retained the reviewed fields, and reopened without an application replay. After explicit reload, the same pre-header disconnect produced two same-key wire arrivals and exactly one stored fixture bookmark. | Production replay behavior remains unverified until deployment. A crash-stranded pending server claim has no lease/fencing recovery in v1; the client does not auto-replay it, and an explicit retry uses a new operation key. |
| AC8 | Fixture 2 native Escape/reopen retained the edit without another suggestion; Cancel produced zero creates and zero exact-URL records; × discarded a fresh draft without another create. A bounded native page click dismissed the delayed-edit popup; the exact toolbar action reopened the authored/cleared values with one suggestion and zero creates. | No additional AC8 behavior is outstanding. |
| AC9 | Worker tests recover pending markers and edited URLs after draft loss as unknown, with no replay or duplicate create. In the headed profile, an ambiguous Save was followed by a complete browser close/restart; the popup reopened unknown with Save disabled and the submitted URL preserved, while sanitized fixture counts remained one create/one bookmark with no restart request. The minimal marker intentionally omits reviewed metadata. | No additional AC9 behavior is outstanding. |
| AC10 | The production grant was exactly `https://boopmark.com:443/*`; `contains443=true` and `contains8443=false`, while the isolated fixture is the v1 self-hosted development representative. Deterministic tests cover arbitrary HTTPS origin/port construction, denied grants, and reconnect/rollback. A real fixture 401 moved the popup to Reconnect with Save unavailable, then normal mode reconnected the retained URL and Disconnect removed the origin. | No additional AC10 behavior is outstanding. A separate deployed HTTPS custom-origin journey and interactive permission denial remain optional browser-UI follow-ups, not release gates. |
| AC11 | URL/API tests reject unsupported protocols and embedded credentials; actual toolbar capture of `chrome://version` displayed the unsupported-URL error and disabled Save/Autofill. In the reloaded real popup, editing the URL to `javascript:alert(1)` displayed the explanation, disabled Save/Autofill, and left sanitized fixture request/create counts at zero. | No additional AC11 behavior is outstanding. |
| AC12 | Minimal manifest/privacy checks and deterministic disconnect cleanup tests pass; the production origin grant is port-scoped and fixture 3 cleanup was exact-ID only. After a whole-browser restart, navigating a fixture page without invoking Boopmark produced zero events/requests/bookmarks. Actual Disconnect left `settingsConfigured=false`, the settings value cleared, no operation, zero drafts/session keys, and zero granted origins. | No additional AC12 behavior is outstanding. |
| AC13 | The real popup fit 400px without overflow; Save was visible; all four capture fields were labeled; status regions exposed `role="status"`/`aria-live="polite"`; fatal errors exposed `role="alert"`. Keyboard traversal reached close, URL, Autofill, title, description, tags, Cancel, Save, and Settings in form order. The reloaded real popup exposed labeled controls and the polite live status during ready, loading (“Fetching metadata…”), and suggestion-error (“Fixture unavailable”) states. | No spoken screen-reader session was observed; retain that limitation rather than inferring it from the accessibility tree. |
| AC14 | Production build `b5a2e845b5d25f43`, agent-browser 0.35.1, Chrome 152.0.7977.64, fixed extension ID, native toolbar capture, enrichment, review, save, refreshed library, and exact cleanup were observed. Current local build `d9cf924682dc1ce6` passed real-toolbar automatic suggestion, reviewed save, accessibility, invalid-URL, save-during-delayed-suggestion, repeated-click, ambiguous-transport, browser-restart, revocation/reconnect/disconnect, and outside-dismiss checks after an explicit extension reload. | Deploy the server idempotency migration/code, confirm the deployed AI provenance invariant, explicitly reload the resulting extension build, then run one bounded production checkpoint and exact-ID cleanup. |
| AC15 | Fixture 3 ID/value verification, user-authorized DELETE 204, exact-ID 404, and zero exact-URL records remaining were observed. Fixture 1 was explicitly preserved. | No cleanup is authorized or required for fixture 1; report any future run's IDs separately. |

No row above substitutes mocked or static evidence for a required live check.
