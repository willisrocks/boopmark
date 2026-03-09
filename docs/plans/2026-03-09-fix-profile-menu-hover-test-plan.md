# Fix Profile Menu Hover — Test Plan

## Strategy reconciliation

The agreed testing strategy still holds. This is a browser-visible interaction regression, and the existing Playwright end-to-end harness is the right observation surface for the user goal: the profile menu must stay open while the pointer moves off the avatar, and both menu items must remain clickable. Reconciliation against the implementation plan adds one adjacent behavior that also needs coverage: keyboard focus must be able to move from the trigger into the menu without collapsing it. No strategy changes or user approval are required.

## Harness requirements

### Harness 1: Existing Playwright E2E app harness

- **What it does**: Starts PostgreSQL plus the app through `scripts/e2e/start-server.sh`, enables the built-in E2E sign-in path, and drives a real browser against `http://127.0.0.1:4010`.
- **What it exposes**: Browser navigation, stepped mouse movement, keyboard focus traversal, URL assertions, and DOM visibility assertions.
- **Estimated complexity**: Already exists. This task only needs a new spec file plus stable selectors in the header.
- **Tests that depend on it**: Tests 1, 2, 3, 4.

## Test plan

1. **Name**: Hovering from the avatar into Sign Out keeps the menu open and allows sign-out
   **Type**: scenario
   **Harness**: Existing Playwright E2E app harness
   **Preconditions**: The app is running through `scripts/e2e/start-server.sh`; E2E auth is enabled; the user signs in through the `Sign in for E2E` button and lands on `/bookmarks`; the header exposes stable selectors for the profile trigger, menu, and Sign Out control.
   **Actions**:
   1. Hover the profile trigger.
   2. Assert that the profile menu is visible.
   3. Read the trigger and Sign Out bounding boxes.
   4. Move the mouse from the trigger center to the Sign Out center in small steps that cross the former dead zone below the avatar.
   5. Click `Sign Out`.
   **Expected outcome**:
   - The menu remains visible after the stepped pointer movement.
   - The Sign Out control remains actionable and the click completes.
   - The browser lands on `/auth/login`.
   - The `Sign in for E2E` button is visible again.
   - **Source of truth**: The user request requires that the menu not disappear while moving the mouse and that menu items be clickable. The implementation plan, Task 1 Step 2, defines the same stepped-pointer logout regression and its post-logout observations.
   **Interactions**: Tailwind hover behavior, header template markup, browser hit testing, `POST /auth/logout`, session cookie removal, login page rendering.

2. **Name**: Hovering from the avatar into API Keys keeps the menu open and opens the settings page
   **Type**: scenario
   **Harness**: Existing Playwright E2E app harness
   **Preconditions**: The same authenticated state as Test 1; the new `/settings/api-keys` route and page template exist as described in the implementation plan.
   **Actions**:
   1. Hover the profile trigger.
   2. Assert that the profile menu is visible.
   3. Read the trigger and API Keys bounding boxes.
   4. Move the mouse from the trigger center to the API Keys center in small steps.
   5. Click `API Keys`.
   **Expected outcome**:
   - The menu remains visible after the stepped pointer movement.
   - The API Keys link remains actionable and navigation completes.
   - The browser lands on `/settings/api-keys`.
   - A heading named `API Keys` is visible on the destination page.
   - **Source of truth**: The user request requires clickable menu items. The implementation plan, Task 3 Step 4, defines this exact stepped-pointer navigation test and the expected destination page heading.
   **Interactions**: Tailwind hover behavior, header template markup, browser hit testing, page router merge, `AuthUser`-protected page handler, Askama template rendering.

3. **Name**: Keyboard focus can move from the profile trigger into the menu without collapsing it
   **Type**: integration
   **Harness**: Existing Playwright E2E app harness
   **Preconditions**: The same authenticated state as Test 1; the profile trigger is a focusable button; the menu uses `group-focus-within` visibility as specified by the implementation plan.
   **Actions**:
   1. Focus the profile trigger.
   2. Verify that the menu becomes visible from focus alone.
   3. Use `Tab` to move focus into the first menu item and then across the menu items.
   4. Activate the focused `API Keys` link with the keyboard.
   **Expected outcome**:
   - The menu is visible while the trigger has focus.
   - The menu stays visible while focus moves from the trigger into the panel.
   - A focused menu item can be activated without relying on hover state.
   - Activating `API Keys` lands on `/settings/api-keys`.
   - **Source of truth**: The implementation plan architecture says the menu must stay open for focus navigation, and Task 2 Step 1-2 explicitly adds a focusable trigger plus `group-focus-within` visibility.
   **Interactions**: Browser focus management, trigger button semantics, Tailwind `group-focus-within`, tabbable menu controls, authenticated settings-page rendering.

4. **Name**: The existing add-bookmark suggest flow still passes after the header and menu changes
   **Type**: regression
   **Harness**: Existing Playwright E2E app harness
   **Preconditions**: The app is running through the standard E2E harness; the existing suggest flow data source used by `tests/e2e/suggest.spec.js` is reachable.
   **Actions**:
   1. Run `tests/e2e/profile-menu.spec.js` together with `tests/e2e/suggest.spec.js`.
   2. Execute the existing suggest flow: sign in, open the add-bookmark modal, blur the URL field to fetch metadata, and submit the bookmark.
   **Expected outcome**:
   - The existing suggest scenario still passes after the profile-menu changes.
   - The modal remains usable, metadata is populated on blur, and the resulting card shows the stored preview image.
   - **Source of truth**: The implementation plan, Task 4 Step 2, explicitly requires the new profile-menu Playwright suite and the existing `suggest.spec.js` suite to pass together so the header change is proven not to regress the authenticated bookmark flow.
   **Interactions**: Header rendering on the bookmarks page, add-bookmark modal behavior, HTMX suggest request, metadata extraction, bookmark creation, bookmark card rendering.

## Coverage summary

- **Covered areas**: Opening the profile menu from the bookmarks header; stepped pointer movement from the avatar into both menu items; clickability of `Sign Out` and `API Keys`; focus-driven menu persistence; authenticated rendering of the new `/settings/api-keys` page; regression coverage for the existing authenticated add-bookmark flow.
- **Explicitly excluded per agreed strategy**: API key management actions on the settings page, because this change only introduces a real destination page; direct unauthenticated navigation assertions for `/settings/api-keys`, because the user goal is in-menu interaction rather than access-control changes; cross-browser matrix expansion beyond the existing Playwright harness.
- **Risk carried by exclusions**: Future API key management work will need its own tests; if access-control behavior on direct URL entry changes later, this plan will not detect it until that behavior is specified; browser-specific hover quirks outside the existing harness coverage could still surface separately.
