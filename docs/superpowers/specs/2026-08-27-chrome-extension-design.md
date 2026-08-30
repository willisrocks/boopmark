# Boopmark Chrome Extension — Quick Spec

Status: Implemented and acceptance verified, including the post-change headed production checkpoint

Date: 2026-08-27

## Summary

Click the Boopmark toolbar icon to save the current page without leaving it. The extension opens a compact **Add Bookmark** popup matching the web app's modal, captures the current URL, and automatically fills available title, description, and tags before the user saves. Like the iOS Share Extension, enrichment uses the connected account's server-side AI configuration and never blocks editing or saving.

**MVP decision:** “Open the modal” means a toolbar-anchored extension popup, not an overlay injected into the website or a new Boopmark tab. Reuse the existing API and modal's visual language, not its server-rendered HTMX markup.

**Default AI behavior:** AI autofill is on by default for accounts with a provider API key and model configured in Boopmark Settings. Opening the popup automatically invokes the server's suggestion pipeline; no extension opt-in, Autofill click, or second provider-key entry is required. Respect an explicit account-level AI-off setting. The 2026-08-29 final production checkpoint verified a fresh automatic suggestion with nonempty tags before Save; under the deployed source invariant, this is source-backed evidence of successful LLM enrichment rather than direct provider-log proof.

**Primary development E2E harness:** standalone **agent-browser in headed mode**, running the sideloaded extension against the real `https://boopmark.com` API. Native-toolbar and scoped-popup adapters operate inside that same agent-browser-launched, dedicated-profile session; they do not constitute a separate harness. This replaces the proposed in-app-browser harness; local fixtures remain supplementary.

## Scope

- Chrome desktop, Manifest V3; one connected Boopmark account/server.
- Capture, review, optionally edit, and save one page at a time.
- Hosted Boopmark by default; configurable HTTPS server URL for self-hosting.
- Automatic, default-on AI autofill using the account's configured provider key/model; respect explicit server-side opt-out and fall back when AI is unavailable.
- Basic setup, draft recovery, loading, error, and success states.

Not in v1: browsing/searching the library inside the extension, editing existing bookmarks, batch capture, context menus, keyboard shortcuts, full-page clipping, screenshot capture by the extension, offline save queues, background browsing collection, automatic URL deduplication, or other browsers. Chrome Web Store submission is a separate release task.

## User flow

1. **Connect once.** On first use, show server URL (default `https://boopmark.com`) and a masked API-key field. Link to that server's Settings → API keys. Explain that opening the popup sends the page URL for metadata lookup, with AI processing if enabled. Grant access only to the selected server origin, then validate credentials with a read-only authenticated request. Failed validation keeps setup visible.
2. **Click the toolbar icon.** Snapshot the active tab's URL and title; do not follow subsequent tab navigation. Open the Add Bookmark popup immediately. URL is editable; browser title is a provisional fallback.
3. **Autofill immediately.** Request suggestions once for the captured URL, without requiring a blur, an opt-in toggle, or another click. With a provider key/model already configured and no account-level opt-out, AI runs automatically using those settings. Show “Fetching metadata…” while the form remains usable. AI suggestions take priority over scraped metadata when the server provides them. Without AI, use available scraped metadata and the browser title fallback. The Autofill button is a retry action, not a prerequisite.
4. **Review.** Allow title, description, and comma-separated tags to be edited. “Autofill” retries suggestions on demand. Only update fields the user has not edited; an intentional clear counts as an edit. If the URL changes, invalidate old requests and old generated values, retain user-authored values, and request fresh suggestions after the valid URL settles.
5. **Save.** Add Bookmark sends a snapshot of the visible form exactly once, even if enrichment is still running. Ignore late suggestions for that submission. Disable submission while saving; show “Saved to Boopmark” only after the server confirms creation, then close after a brief confirmation.
6. **Dismiss or return.** Before Save, Cancel explicitly discards the draft. Clicking outside the popup or losing focus retains a session-only draft keyed to connection, tab, and original captured URL; reopening the same page restores it without another automatic suggestion request. Clear it on confirmed save, Cancel, disconnect, or browser-session end. After Save is dispatched, close controls only dismiss the UI: they cannot cancel the server write or discard its operation record. Never submit a draft automatically.

## Popup mockup

Ready state after successful autofill; values are illustrative.

```text
┌──────────────────────────────────────────────────┐
│ Boopmark                              [Settings] │
│ Add Bookmark                                 [×] │
│                                                  │
│ URL                                              │
│ ┌──────────────────────────────────────────────┐ │
│ │ https://example.com/designing-better-tools   │ │
│ └──────────────────────────────────────────────┘ │
│ Metadata filled. Review before saving. [Autofill]│
│                                                  │
│ Title                                            │
│ ┌──────────────────────────────────────────────┐ │
│ │ Designing better tools                       │ │
│ └──────────────────────────────────────────────┘ │
│ Description                                      │
│ ┌──────────────────────────────────────────────┐ │
│ │ Practical ideas for building focused,        │ │
│ │ useful software.                             │ │
│ └──────────────────────────────────────────────┘ │
│ Tags                                             │
│ ┌──────────────────────────────────────────────┐ │
│ │ design, software, tools                      │ │
│ └──────────────────────────────────────────────┘ │
│ Comma separated                                  │
│                                                  │
│                         [Cancel] [Add Bookmark]  │
└──────────────────────────────────────────────────┘
```

Target width: roughly 400px. Match the current web modal: dark `#1e2235` surface, `#0f1117` inputs, subtle gray borders, rounded corners, and blue primary action. Use a short multiline description field. No image preview in v1; the server retains its normal preview-image pipeline. Before saving, Close (×) discards like Cancel; while saving, replace Cancel with Close and preserve operation status.

| State | UI behavior |
| --- | --- |
| Fetching | “Fetching metadata…” with spinner; fields and Add Bookmark remain available. |
| Suggestions received | “Metadata filled. Review before saving.” Only if some fields were actually filled. |
| Empty result / request failed | “No metadata available. Edit fields or save as-is.” / “Could not fetch metadata. Retry or save as-is.” |
| Saving | “Saving…”; freeze the submitted form and prevent repeated clicks. |
| Save failed | Inline actionable error; retain the draft and permit an explicit retry when the failure is definite. |
| Save outcome unknown | “Save could not be confirmed. Check Boopmark before retrying.” Never automatically resend. |
| Unsupported URL | “Open a web page to add a bookmark.” Disable enrichment and save until a valid HTTP(S) URL is provided. |
| Invalid/revoked credentials | Show “Reconnect Boopmark” and preserve the draft; never misreport this as an empty metadata result. |

The current suggestion response has no AI provenance or failure flag. Do not claim “AI filled” or “AI is off” based only on returned fields; fallback can be silent. AI settings continue to live in Boopmark's web settings, not a second extension toggle.

## Technical approach

Proposed location: `extensions/chrome/`. Keep the extension thin: packaged popup/settings UI, a small API client, and a service worker coordinating requests and session draft/save state. No content script is needed to capture tab URL/title. Do not load the HTMX modal into the extension or fetch remote executable code.

| Operation | Existing API contract |
| --- | --- |
| Validate connection | `GET /api/v1/bookmarks?limit=1` with `Authorization: Bearer <key>`. |
| Pre-save autofill | `POST /api/v1/bookmarks/suggest` with `{ "url": "…" }`; returns `title`, `description`, `tags`, `image_url`, and `domain`. |
| Save reviewed values | `POST /api/v1/bookmarks` with URL, visible title/description, parsed tags, and the durable operation ID in `Idempotency-Key`; success is `201` with the created bookmark. |

- The extension always initiates suggestions automatically for a fresh valid capture; the server decides AI eligibility. The existing settings service also checks the account's `enabled` flag, so key/model presence must not silently override an explicit off setting. Verify the configured production account is eligible without changing its settings. The service falls back to scraped metadata on AI failure. The extension's Boopmark bearer key is separate from the AI-provider key, which remains server-side.
- Do **not** add `suggest=true` to the final create request: pre-save suggestions have already been offered. Unlike the iOS client's current create call, this avoids a second AI request and saves the user's reviewed values. Send title/description as explicit strings (including `""` for intentional clears), and tags as an array (including `[]`); omitted/null text can be refilled by server metadata extraction. Leave preview-image acquisition to the server.
- Track request generation, URL, connection, and dirty fields. Ignore stale responses after URL/connection changes, cancellation, or submission. Trim tags and discard empty entries.
- Save coordination must survive popup closure where possible. Before dispatch, persist a minimal operation marker (connection, submitted URL, timestamp, state) separately from session drafts. Keep this local marker across browser restarts; interrupted/unconfirmed operations reopen as unknown, never as a retry loop. Clear it after the result is acknowledged or on disconnect. This is a status record, not an offline queue: never replay it. Send its opaque ID as `Idempotency-Key` on the create request. The server atomically deduplicates that key per account: a replay while the first operation is still pending returns 503 without another create; an identical replay after completion returns the original bookmark with 201; reuse with a different reviewed payload returns 409. Clients omitting the header remain backward-compatible, and ordinary user-created duplicate URLs remain allowed.
- The only MVP server-contract addition is optional idempotent create. Live fault injection showed that Chromium can internally replay a POST when a reused connection closes after commit but before response headers; a client-side lock alone cannot guarantee one stored bookmark. The REST suggest route still does not pass the user's existing tag vocabulary into enrichment; improving that parity and adding AI provenance remain follow-ups, not assumed capabilities.

## Permissions and privacy

- Use `activeTab` for user-invoked access to the current URL/title and `storage` for extension settings/state. No history, cookies, scripting, or persistent read access to visited sites.
- Request API host access only for the configured Boopmark origin during Connect, using optional host permissions. Send API requests from trusted extension contexts, not page scripts. Verify hosted and self-hosted access in Chrome; for v1 the loopback fixture is the self-hosted development representative when arbitrary HTTPS origin/port construction is also covered deterministically. A separate deployed custom-origin journey is an optional follow-up. Do not add wildcard server CORS as a workaround.
- Require HTTPS for the Boopmark server, with explicit loopback HTTP exceptions for local development. Bookmark targets may be HTTP or HTTPS; reject internal/file/data/javascript URLs and URLs containing embedded credentials before sending them.
- Persist the Boopmark API key and minimal save-operation markers only in extension-local storage, restricted to trusted contexts; never sync them, expose them to a page, or log secrets. Never put the API key in a URL. Local extension storage is not an OS keychain. Keep drafts in session storage; disconnect clears credentials, drafts, operation markers, and the former server's host permission. Never forward credentials to another origin through redirects.
- Before user invocation, do not collect or transmit tab data. After invocation, transmit only the selected URL and form data to the configured server; no page DOM, browsing history, cookies, or screenshots. The server can fetch page metadata and use its configured AI provider. Render all returned text as text, not HTML.

## Acceptance criteria

| ID | Given / when | Then |
| --- | --- | --- |
| AC1 | Connected user clicks the icon for a fresh capture on a web page. | Popup opens on that page, URL preserves query and fragment, browser title appears provisionally, and one automatic suggestion request starts without blur. No bookmark exists yet. |
| AC2 | A production account already has its provider key/model configured and has not explicitly disabled AI; a fresh public page is captured through the sideloaded extension. | Opening the popup alone triggers live AI enrichment by default: title, description, and tags populate before Save, with no extension opt-in, Autofill click, or provider-key re-entry. Verify actual AI use, not merely successful scraping. No direct provider request leaves the extension. |
| AC3 | Account AI is explicitly off (even with a stored key/model), unconfigured, failing, or returning only partial metadata. | Available metadata fills; browser title remains a fallback; missing fields stay optional. Explicitly off/unconfigured accounts make no provider call. Form remains editable/saveable without a modal error or false AI-success claim. Exercise these variants in isolated tests, not by changing the user's production settings. |
| AC4 | User types or clears a field while suggestions are pending, then results arrive. | User edits/clears survive. Changing the URL or connection prevents old results from affecting the new draft. |
| AC5 | Valid draft is submitted during or after autofill. | One logical create operation uses the submitted field snapshot and durable idempotency key, with no second AI request and no late suggestion overwrite. Repeated clicks send nothing further. Any browser/network transport replay carries the same key and stores exactly one bookmark. |
| AC6 | Server confirms `201`. | Show success, clear the draft, and close. The saved bookmark is visible after refreshing the web library with the expected URL, text, and tags. |
| AC7 | Save receives a definite error or an ambiguous network interruption. | Preserve the draft, show the appropriate error/unknown state, and never claim success or initiate a retry. A transport-level replay with the same operation key cannot create a second bookmark. |
| AC8 | User cancels, clicks ×, or dismisses without saving. | Cancel/× discard; outside dismissal restores the draft on reopen for the same page. None of these actions creates a bookmark. |
| AC9 | Popup closes after Save is clicked and is reopened, including after browser restart. | Show the recorded result or pending/unknown state; do not initiate another logical create. Any transport replay from the original dispatch carries the same key and stores one bookmark. Closing cannot cancel or erase a dispatched operation. |
| AC10 | Connection is absent, invalid, revoked, or host permission is denied. | Setup/reconnect explains the problem; no save proceeds until connected. API key is not exposed, and a same-connection draft is retained. |
| AC11 | Active URL is unsupported or missing, or the edited URL is invalid. | No lookup/create request is sent for it; the explanation is visible and Save is disabled. |
| AC12 | Extension is installed but not invoked, or the user disconnects. | No browsing data is collected before invocation; disconnect clears credentials/drafts. Only the selected server has API host access. |
| AC13 | User navigates using keyboard or assistive technology. | Every field/action has a label, focus order follows the form, loading/errors are announced, and primary actions remain reachable without horizontal scrolling. |
| AC14 | A development build is ready for E2E testing, including after a relevant extension change. | Explicitly reload the unpacked extension in its isolated headed agent-browser profile, verify the running worker reflects the build under test, connect to the real production API, and exercise the actual toolbar action, autofill, review/edit, save, and refreshed production library. A browser relaunch or printed build hash alone is insufficient because Chrome may retain a same-version MV3 worker. Record the build and result for each development checkpoint. A mocked API, local-only run, headless run, or popup opened directly as a tab does not satisfy this gate. |
| AC15 | A production E2E run creates a disposable bookmark. | Record the exact created ID and verify its values. Do not alter unrelated bookmarks, tags, credentials, or account settings. Cleanup is separately authorized, targets only that run's records, and is verified; uncertain writes are reconciled before any retry. |

## QA plan

### Primary: sideloaded extension + agent-browser + production

Use this during development, not only before release. The extension itself must make the real production requests; direct API calls may verify results but cannot replace the capture/save journey.

**Development loop:** change → deterministic checks → explicitly reload the sideloaded extension → verify current-worker behavior → headed production smoke journey → record evidence. Run the live journey at relevant development checkpoints, keeping captures bounded rather than generating production bookmarks on every file save. Use the repository's `npm run extension:browser -- <command>` launcher for the same named session throughout; it supplies the unpacked extension, dedicated profile, browser executable, and headed mode. See the [QA runbook](../../chrome-extension-qa.md#production-gate) for production-session commands. The launcher's default article is a local fixture, so production runs must explicitly select their public test URL and production connection.

1. **Prepare the harness.** Install/verify standalone agent-browser and an extension-capable Chromium browser. Use `--headed`, `--extension <absolute-unpacked-build-directory>`, and a dedicated `--profile <test-profile-directory>` so extension connection state persists without using the user's everyday browser profile. Keep profiles and secrets out of git. Pin and record the CLI/browser versions and extension build. After each relevant rebuild, use the dedicated profile's `chrome://extensions` Reload control for Boopmark (Developer mode on), then verify a changed runtime behavior or marker from the build under test. Closing/relaunching alone is not proof that Chrome replaced a same-version service worker.
2. **Connect safely.** Configure the extension for `https://boopmark.com` with an authorized Boopmark bearer key. The existing AI-provider key/model stays in account Settings; inspect its enabled/configured state without revealing or changing secrets. If access is missing, request user provisioning rather than creating/rotating keys. Do not place credentials in shell arguments, screenshots, traces, or committed fixtures.
3. **Prove automatic pre-save AI.** Open a public test article with a unique run marker in its query string and a fragment. Verify no bookmark for that exact fixture exists first. Click the actual extension toolbar action and observe the captured URL, provisional title, one live suggestion request, and populated title/description/tags before Save—without clicking Autofill. Record evidence of successful AI use as described below. Also do a cancel-only run and verify it creates nothing.
4. **Review and save.** Edit one returned value and intentionally clear another. Save once; verify the submitted snapshot, presence (but not the recorded value) of the operation-derived `Idempotency-Key`, no `suggest=true` on create, no second suggestion request caused by Save, and the server's `201` plus bookmark ID. Reopen the extension and refresh the production web library; verify the exact saved URL/text/tags and only one new record for the fixture.
5. **Check lifecycle and clean up.** Verify outside-dismiss/reopen and normal post-save reopening in the same headed harness. Reconcile any unexpected unconfirmed write by its fixture before retrying. With action-specific approval, delete only the exact bookmark IDs created by this run, then verify absence. Do not delete shared tags or other records; report any residual test data. Run a bounded number of public-page captures to avoid unnecessary AI charges or production load.

**Real toolbar coverage:** agent-browser's documented extension-loading support does not by itself prove it can automate browser-chrome controls. The initial harness spike must verify toolbar/popup interaction. If an actual toolbar click needs user assistance, record that step as manual in the same headed browser; do not replace it with a direct popup-tab navigation and claim equivalent coverage. If the path cannot be exercised, report the E2E gate as blocked.

**AI evidence:** the current suggestion JSON has no general provenance flag, and populated title/description alone can come from scraping. A fresh live `/suggest` response with nonempty tags provides code-backed evidence of successful AI enrichment: the inspected `UrlMetadata` has no tags, this route supplies no existing-tag vocabulary, and the suggestion service obtains tags only from successful LLM output. Record the initially empty draft, exact fixture request, sanitized response tags/status/time, and corresponding pre-edit popup values. Label this a source-backed inference, not direct provider logs; record any uncertainty about the deployed source revision. Approved redacted provider-success logs are an alternative, not a mandatory new telemetry project. Empty tags do not prove AI failed, and configured settings or fixture responses alone prove neither outcome. Do not expose provider keys, prompts containing private data, or unrelated account activity.

**Run record:** capture build/commit, CLI/browser versions, extension ID, fixture URL/run marker, timestamps, before/after-autofill screenshots, sanitized request counts/statuses, AI evidence, submitted values, created ID, library verification, manual steps, and cleanup outcome. Save no authorization headers or credentials. Never mark an unexecuted, blocked, or unverified check as passed.

### Supplementary deterministic coverage

- **Unit:** URL validation, tag parsing, dirty-field merges including intentional clears, stale-response guards, request serialization, and draft/save transitions.
- **Integration:** counted stub-provider calls for enabled/disabled/unconfigured/failing AI, partial responses, URL A→B→A, connection changes, delayed suggestions after Save, repeated clicks, permission/auth failures, and definite versus ambiguous save outcomes. Assert zero provider calls when AI is off and no extra provider call on final create. Verify sequential and concurrent same-key create replays return one bookmark, key reuse with a different payload is rejected, keys are account-scoped, and requests without a key retain existing behavior.
- **Lifecycle/fault injection:** worker termination, browser restart after dispatch, offline saves, and expired credentials in an isolated test environment. Verify session drafts expire while unresolved operation markers survive without replay. Do not manufacture failures by changing the user's production settings, revoking their key, or disrupting production services.
- **Privacy/accessibility:** no pre-invocation collection, origin-scoped permissions, no credential forwarding across origins, inert rendering of malicious metadata, local/session rather than sync storage, disconnect cleanup, keyboard/focus behavior, and announced loading/errors.
- **Local web regressions:** retain `tests/e2e/suggest.spec.js` and the dedicated port-4010 server, not the port-4000 dev server. Local checks supplement but do not replace the live production extension gate. Bootstrap must use the repository's required `devproxy up` workflow or reuse a verified checkout-owned devproxy database; never use direct `docker compose up`. When reusing that database, reset only the dedicated `e2e@boopmark.local` identity before the server starts so consecutive runs cannot inherit test bookmarks, keys, sessions, or settings.

### Completion gate and current status

Every AC must have recorded passing evidence from its appropriate test layer, with the real production journey mandatory for AC2/6/14. No unresolved lost-edit, duplicate-submit, credential-leak, or false-success defects. AI verification and cleanup status must be explicit. Missing provider evidence, toolbar support, credentials, or browser capability is a reported blocker, not a silent test substitution.

Implementation lives in [`extensions/chrome/`](../../../extensions/chrome/README.md), with a pinned standalone agent-browser harness, durable per-account create idempotency, and deterministic regression tests. The deployed server contract and final sideloaded build passed the bounded 2026-08-29 post-change production checkpoint: explicit extension reload, exact native toolbar action, automatic source-consistent AI enrichment, reviewed-value save with a present valid UUID idempotency header, one HTTP 201 create, refreshed-library verification, normal reopen, and exact-ID cleanup with zero residual fixture records. The isolated suites separately prove empty/partial/failing enrichment, delayed-result edit protection, outside-dismiss recovery, zero pre-invocation collection, auth and permission failures, disconnect cleanup, browser-restart unknown-operation recovery, and the two-wire-attempt ambiguous transport case storing one bookmark. AC1–AC15 are satisfied by their appropriate evidence layers. A claim that remains pending forever after a server crash is a documented resilience follow-up; the v1 client does not automatically replay an unknown operation and an explicit user retry receives a new operation key. Nonempty production suggestion tags remain a source-backed successful-LLM inference rather than a claim of direct provider logs. See the [QA runbook and evidence record](../../chrome-extension-qa.md) for the complete evidence. Connection-screen privacy copy explains automatic metadata/AI processing. Store publishing, OAuth-style connection, and existing-tag-aware suggestions remain follow-ups.

### Reference points

- [iOS sharing behavior](../../../mobile/ios/README.md) and [Share Extension implementation](../../../mobile/ios/BoopmarkShareExtension/ShareViewController.swift).
- [Web Add Bookmark modal](../../../templates/bookmarks/add_modal.html), [bookmark REST API](../../../server/src/web/api/bookmarks.rs), and [enrichment service](../../../server/src/app/enrichment.rs).
- Chrome: [action popups](https://developer.chrome.com/docs/extensions/develop/ui/add-popup), [activeTab](https://developer.chrome.com/docs/extensions/develop/concepts/activeTab), [cross-origin requests](https://developer.chrome.com/docs/extensions/develop/concepts/network-requests), and [extension storage](https://developer.chrome.com/docs/extensions/reference/api/storage).
- agent-browser: [Chrome engine and extension loading](https://agent-browser.dev/engines/chrome), [command options](https://agent-browser.dev/commands), and [configuration](https://agent-browser.dev/configuration).
