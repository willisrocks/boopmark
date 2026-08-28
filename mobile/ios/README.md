# Boopmark for iPhone

This directory contains the v1 SwiftUI iPhone client and its native Share
Extension. The app is intentionally thin: Boopmark's Rust server remains the
source of truth for bookmarks, metadata suggestions, tags, and search.

## What is included

- A four-screen iPhone experience: bookmark browsing with search, tag filters,
  and newest/oldest/title/domain sorting; bookmark detail and edit; capture;
  and settings.
- A native Share Extension that accepts a web URL or plain-text URL from Safari,
  Reddit, YouTube, and other apps, then immediately uses the connected account's
  server-side enrichment configuration to fill title, note, and tags. Autofill
  remains optional and never blocks saving or editing the form.
- Web-parity AI autofill in both capture and edit: **Autofill with AI** calls the
  signed-in account's production enrichment pipeline and fills title, note, and
  tags before the user chooses to save. Capture preserves fields the user
  already typed; edit refreshes fields when explicitly requested, matching the
  web app's suggestion behavior.
- A shared `BoopmarkShared` Swift package containing API models, a URLSession
  actor client, settings/keychain access, and a JSON-backed offline capture
  queue.
- App Group storage (`group.com.boopmark.shared`) so the containing app and
  extension can see the same pending captures.
- Keychain sharing configuration for the API key. Xcode expands
  `$(AppIdentifierPrefix)` to the developer Team ID when the target is signed.

## Generate the Xcode project

The repository commits `project.yml` rather than a machine-generated project.
Install [XcodeGen](https://github.com/yonaskolb/XcodeGen), then run:

```sh
cd mobile/ios
./generate-xcodeproj.sh
open Boopmark.xcodeproj
```

Select a Team for both targets and enable the App Groups and Keychain Sharing
capabilities with the identifiers already present in the entitlements. The
extension and app must use the same Team so they can share the API-key item.

The server URL can be a production HTTPS URL. `http://localhost`,
`http://127.0.0.1`, and `http://[::1]` are allowed for local development; all
other HTTP URLs are rejected because the app sends a bearer token.

In the running app, create an API key in Boopmark's Settings → API keys and
enter the server URL and key in the app's Settings. The key is stored in the
Keychain, not UserDefaults. Settings verifies the saved URL and key by reading
them back, then loads the account's bookmarks before closing; invalid keys,
connection failures, and Keychain or entitlement failures remain visible in
Settings instead of leaving an apparently empty library.

The server remains authoritative for the bookmark list. The app reloads the
active search, filters, and sort whenever it returns to the foreground, after
capture or Settings sheets close, and after successful create, update, or
delete requests. This is required for Share Extension saves because an iOS
extension runs in a separate process and cannot update the app's in-memory
list directly. Responses started for an older server connection are discarded
after new connection details are saved.

The app declares its launch screen in `Info.plist`. Besides providing a native
launch experience, this ensures iOS uses the full modern iPhone viewport rather
than a letterboxed compatibility window.

## Offline capture behavior

The Share Extension tries one create request. If there is no connection, or if
the app has not been configured yet, it writes a `PendingCapture` to the shared
JSON queue and dismisses. The containing app shows the queue count in Settings.
Queued captures are sent only after the user taps **Send queued captures**.
This is intentional: a create POST is not idempotent, so an automatic retry
could silently create duplicate bookmarks. App and extension queue mutations
use a lock file and atomic read/modify/write so simultaneous processes do not
overwrite each other's captures.

## Tests

The shared API and queue logic have unit tests and can be run from the command
line on a machine with a working Swift toolchain:

```sh
cd mobile/ios/BoopmarkShared
swift test
```

Run the shared suite and the iPhone UI suite with:

```sh
cd mobile/ios
DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer \
xcodebuild -project Boopmark.xcodeproj -scheme Boopmark \
  -destination 'platform=iOS Simulator,name=Boopmark E2E' test
```

Keep code signing enabled for the UI suite. Its Share Extension test exercises
the signed App Group entitlement and verifies the saved capture in the
containing app; disabling signing removes that cross-process guarantee.

Verified locally on August 27, 2026 with Xcode 26.2 and an iPhone 17 Pro
simulator running iOS 26.3.1:

- App and embedded Share Extension simulator build: passed.
- `BoopmarkShared`: 12 tests passed.
- `BoopmarkTests`: 3 tests passed, covering connection-save loading,
  foreground reconciliation of a bookmark created by another process, and
  protection against a slow old-server response overwriting a new connection.
- `BoopmarkUITests`: 7 normal tests passed (7 production-only tests skipped),
  including a full-device viewport assertion and sort/tag-filter controls.
- The UI suite navigates Safari to a query-bearing URL, opens the system Share
  Sheet, selects Boopmark, saves through the extension into the App Group
  queue, verifies the exact queued URL/count in the containing app, and removes
  the fixture.

Production-only tests are opt-in and skipped in normal regression runs. Set
`BOOPMARK_RUN_LIVE_E2E=1` in the UI test runner environment after provisioning
the app with `testProvisionLiveServerConnection`. They cover live capture,
Anthropic title/note/tag autofill in capture and edit, search/edit/delete, and
Safari Share Extension delivery. The tests use disposable fixtures or cancel
before saving and must never contain a committed credential.

`testLiveProductionSafariShareCapture` exercises the native iOS path end to
end: Safari's system Share Sheet lists Boopmark alongside normal share
destinations, launches the embedded extension, passes the exact URL, saves it
with the shared Keychain credential to `https://boopmark.com`, returns to the
already-running containing app, finds the unique bookmark without a force quit
or search-driven reload, verifies the exact stored URL, and then deletes the
production fixture. It does not substitute a direct API request for the system
sharing interaction.

`testLiveProductionShareAutofillsBeforeSave` opens a real article through
Safari's Share Sheet and verifies that the production server fills all three
Share Extension fields—title, note, and tags—before canceling without creating
a bookmark.

The production parity run uses a dedicated simulator Keychain credential and
the real `https://boopmark.com` API. It verifies the core web bookmark workflow:
live listing and search, tag filtering and sorting at the shared list API,
capture, explicit pre-save AI autofill, detail/edit with explicit AI refresh,
delete, rendered metadata/images/tags, and Safari Share Extension capture. The
AI assertion succeeds only when title, note, and tags are all populated by the
production suggestion response. Account administration, API-key creation,
bulk import/export, tag consolidation, and image-repair tools remain web
settings/operations rather than mobile bookmark workflows.

## API contract

The client uses the existing bearer-key endpoints:

- `GET /api/v1/bookmarks?search=…&tags=…&sort=…&limit=50&offset=0`
- `POST /api/v1/bookmarks?suggest=true`
- `POST /api/v1/bookmarks/suggest`
- `PUT /api/v1/bookmarks/{id}`
- `DELETE /api/v1/bookmarks/{id}`

Request and response fields use the server's snake_case JSON names through the
shared encoder/decoder. RFC3339 timestamps with or without fractional seconds
are supported.
