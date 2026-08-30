# Boopmark for Chrome

A small Manifest V3 extension for the same capture → review → save flow as Boopmark on iOS. The popup uses the main app's colors and logo. No bundler or runtime dependencies.

## Install for development

1. Open `chrome://extensions` in Chrome and enable Developer mode.
2. Choose **Load unpacked** and select this `extensions/chrome` directory.
3. Pin Boopmark in the toolbar and click its icon on a web page.
4. Connect to `https://boopmark.com` (or your HTTPS server) using a Boopmark API key from your server's Settings → API keys. Grant access to that server when Chrome asks.

Your AI-provider key/model stays in Boopmark Settings. With an eligible account, opening the popup automatically requests metadata and AI suggestions. No separate extension AI toggle or provider key is needed. You can edit or save while metadata loads; edits and intentional clears are preserved.

Review the URL, title, description, and comma-separated tags, then choose **Add Bookmark**. Saving sends your reviewed values without another AI request. Success is shown only after confirmed creation.

Cancel/× discards an unsaved draft; clicking outside retains it for the browser session. A save already sent cannot be canceled by closing the popup. If its outcome is unknown, check the web library before an explicit retry—there is no automatic replay or offline queue.

## Privacy

The extension reads the active tab URL/title only when you invoke it. It sends the selected URL and form values to your configured server, which may fetch metadata and call its AI provider. It does not read page contents, cookies, history, or screenshots. The Boopmark bearer key stays in trusted extension-local storage (not sync and not an OS keychain). Drafts are session-only; minimal save-status markers survive restart. Disconnect clears connection data, drafts, operation markers, and the former server's host access.

## Development and verification

From the repository root:

```sh
npm ci
npm run test:extension
npm run extension:fixture
# In a second terminal, after npx agent-browser install:
npm run extension:browser
```

See [the QA runbook](../../docs/chrome-extension-qa.md) and [acceptance spec](../../docs/superpowers/specs/2026-08-27-chrome-extension-design.md). The primary E2E gate is the actual toolbar action in headed agent-browser against production; isolated fixtures do not replace it. Explicitly use Boopmark's `chrome://extensions` Reload control (Developer mode on), or bump/reinstall the extension, after source changes. Closing/relaunching a same-version build alone can retain a stale MV3 worker, so verify current runtime behavior before recording evidence.

The local headed harness includes an exact-PID native helper for opening the toolbar action (`scripts/e2e/chrome-toolbar.applescript`) and a guarded Node wrapper for outside dismissal (`scripts/e2e/chrome-dismiss-popup.mjs`). The wrapper verifies this checkout's exact local session/profile, Chrome process, unpacked extension, and static fixture page before invoking its AppleScript; do not invoke `chrome-dismiss-popup.applescript` directly. A synthetic click in the underlying page target does not necessarily move native popup focus.

Icons are rasterized from `static/boopmark-logo.svg`; regenerate with `node scripts/e2e/chrome-icons.mjs` from the root. Create the release ZIP with `npm run extension:package`; its strict allowlist keeps tests, documentation, and store artwork out of the extension package. Store listing copy, disclosures, and release steps are in [`docs/chrome-web-store-listing.md`](../../docs/chrome-web-store-listing.md).

For a traceable release build, manually run the **Chrome extension artifacts** GitHub Actions workflow on the revision to ship. It runs the extension suite, validates the package allowlist and manifest version, and publishes two 90-day artifacts: a Web Store ZIP with its SHA-256 checksum and an unpacked directory for sideloading through `chrome://extensions`.
