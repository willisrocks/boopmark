# Boopmark Chrome Web Store listing

Release package: `dist/boopmark-chrome-0.1.0.zip`

Chrome Web Store item ID: `oncfakjpmbjhmahhccdboahbhgjihgbo`

Dashboard status on August 30, 2026: draft created; package, listing copy, icon, screenshot, promo tile, privacy disclosures, and public/free/all-regions distribution configured. The three limited-use certifications and final review submission require the publisher's explicit confirmation.

## Store listing

- Product name: **Boopmark**
- Category: **Productivity**
- Language: **English**
- Homepage: `https://boopmark.com`
- Support: `https://boopmark.com/support`
- Privacy policy: `https://boopmark.com/privacy`

### Summary

Capture the current page, review automatic metadata, and save to Boopmark.

### Detailed description

Save the page you're viewing to Boopmark without leaving your tab.

Boopmark for Chrome brings the same capture-and-review workflow as the Boopmark iOS share extension:

- Capture the active page's URL and title with one click.
- Automatically suggest a title, description, and tags when AI is enabled in your Boopmark account.
- Review or edit every field before saving.
- Preserve your draft if the popup is dismissed.
- Connect to boopmark.com or your own HTTPS Boopmark server.

The extension runs only when you open it. It does not collect browsing data in the background. Your Boopmark API key is stored locally in Chrome and sent only to the server you choose. Page URLs are sent to that server for metadata lookup and, when enabled in your account, server-side AI enrichment.

## Privacy disclosures

Single purpose:

> Let a user capture the current web page, review or enrich its bookmark metadata, and save it to the user's chosen Boopmark server.

Permission justifications:

- `activeTab`: read the current tab's URL and title only after the user opens Boopmark, so the capture form can be prefilled.
- `storage`: retain the selected server and API key locally, preserve an unsaved session draft, and track an in-flight save safely across extension worker restarts.
- Optional host access: communicate only with the Boopmark server the user explicitly configures and grants. HTTPS is required outside local development.

Remote code: **No.** All executable extension code is included in the package. Network responses contain data only and are never executed as code.

Data handled:

- Authentication information: the user's Boopmark API key is stored in Chrome extension-local storage and sent only to the configured Boopmark server.
- Web history / browsing activity: the active page URL and title are read only when the user invokes the extension and are sent to the configured server for metadata lookup and saving.
- Website content: the configured server may fetch the selected URL to extract metadata and may send the URL and extracted content to its configured AI provider when AI is enabled.
- User-provided content: reviewed title, description, and tags are sent to the configured server when saving.

Boopmark does not sell or transfer this data for advertising, creditworthiness, or unrelated purposes. It does not collect data while the extension is closed.

## Store artwork

- Screenshot, 1280×800: `extensions/chrome/store-assets/screenshot-capture-and-review.png`
- Small promo tile, 440×280: `extensions/chrome/store-assets/small-promo-tile.png`
- Store icon, 128×128: `extensions/chrome/icons/128.png`

Regenerate artwork from the latest production E2E capture with `npm run extension:store-assets`. Create the upload archive with `npm run extension:package`; the archive intentionally excludes tests, documentation, and store artwork.
