# Test Plan: Redesign Login Page

## Strategy reconciliation

The agreed testing strategy (Medium Fidelity) maps cleanly to the implementation plan. Key observations:

- The plan adds `data-testid` attributes to all interactive elements, which the strategy assumed. No adjustment needed.
- The E2E server bootstrap (`scripts/e2e/start-server.sh`) needs `ENABLE_LOCAL_AUTH=1` added, as the strategy anticipated. The plan includes this.
- The error message is passed via query parameter (`?error=...`), confirmed in `auth.rs` line 229. The plan's test for error display uses this mechanism correctly.
- The "successful login" test uses the existing E2E auth button rather than creating a local auth user in the DB, which is simpler and avoids test infrastructure changes. This is a reasonable simplification — it tests the redirect flow without needing `just add-user` in the E2E bootstrap.

**No strategy changes requiring user approval.**

## Harness requirements

**No new harness needed.** The existing Playwright + E2E server harness (`scripts/e2e/start-server.sh` + `playwright.config.js`) is sufficient. The only harness change is adding `ENABLE_LOCAL_AUTH=1` to the E2E server bootstrap script so the local auth form renders during tests.

All tests below depend on this harness change.

## Test plan

### 1. Login page renders with dark-themed input fields (regression)

- **Name**: Input fields render with dark backgrounds, not white/transparent
- **Type**: regression
- **Harness**: Playwright E2E (`tests/e2e/login.spec.js`) against E2E server on port 4010
- **Preconditions**: E2E server running with `ENABLE_LOCAL_AUTH=1`; user is not authenticated
- **Actions**:
  1. Navigate to `/auth/login`
  2. Locate the email input by `data-testid="login-email-input"`
  3. Locate the password input by `data-testid="login-password-input"`
  4. Locate the submit button by `data-testid="login-submit-button"`
  5. Assert all three elements are visible
  6. Evaluate `getComputedStyle(emailInput).backgroundColor`
  7. Evaluate `getComputedStyle(passwordInput).backgroundColor`
- **Expected outcome**:
  - All three elements are visible on the page
  - Email input background color is `rgb(26, 29, 46)` (the CSS value of `#1a1d2e`)
  - Password input background color is `rgb(26, 29, 46)`
  - **Source of truth**: The design language section of the implementation plan specifies `bg-[#1a1d2e]` for inputs, matching the existing app design system (settings page). The original bug report confirms `bg-[#161829]` was not compiled into output.css, causing transparent/white backgrounds.
- **Interactions**: Depends on Tailwind CSS compilation (output.css must contain the `bg-[#1a1d2e]` class). This is the core regression — if the CSS class is missing, this test catches it.

### 2. Typed text is visible in input fields (regression)

- **Name**: Typed text appears as light gray on dark background, not invisible white-on-white
- **Type**: regression
- **Harness**: Playwright E2E (`tests/e2e/login.spec.js`)
- **Preconditions**: E2E server running with `ENABLE_LOCAL_AUTH=1`; user is not authenticated
- **Actions**:
  1. Navigate to `/auth/login`
  2. Fill the email input with `test@example.com`
  3. Assert the input's value is `test@example.com`
  4. Evaluate `getComputedStyle(emailInput).color`
- **Expected outcome**:
  - The input value reads back as `test@example.com` (confirms the field is interactive)
  - Text color is `rgb(229, 231, 235)` (Tailwind's `text-gray-200`, the app's standard text color)
  - **Source of truth**: The design language section specifies `text-gray-200` for primary text. The original bug report identifies white text on white background as the core issue; `text-gray-200` on `bg-[#1a1d2e]` provides sufficient contrast.
- **Interactions**: Same CSS compilation dependency as test 1.

### 3. Error message displays on invalid credentials (scenario)

- **Name**: Login error message appears when credentials are wrong
- **Type**: scenario
- **Harness**: Playwright E2E (`tests/e2e/login.spec.js`)
- **Preconditions**: E2E server running with `ENABLE_LOCAL_AUTH=1`; user is not authenticated
- **Actions**:
  1. Navigate to `/auth/login?error=Invalid+email+or+password`
  2. Locate the error div by `data-testid="login-error-message"`
  3. Assert it is visible and contains text "Invalid email or password"
- **Expected outcome**:
  - The error message element is visible
  - It contains the text "Invalid email or password"
  - **Source of truth**: `auth.rs` line 229 redirects to `/auth/login?error=Invalid+email+or+password` on failed local login. The `LoginPage` template struct passes `params.error` as `login_error` (line 43). The template renders it inside the `{% if let Some(err) = login_error %}` block.
- **Interactions**: Tests the server-side error propagation path (query param -> template variable -> rendered HTML). Note: this test navigates directly with the error query param rather than submitting bad credentials, because submitting bad credentials would require a user to NOT exist, which is already the default state. Both approaches test the same rendering path; the direct navigation is more deterministic.

### 4. Successful login redirects to bookmarks (scenario)

- **Name**: Signing in via E2E auth redirects to the bookmarks page
- **Type**: scenario
- **Harness**: Playwright E2E (`tests/e2e/login.spec.js`)
- **Preconditions**: E2E server running with `ENABLE_E2E_AUTH=1`; user is not authenticated
- **Actions**:
  1. Navigate to `/auth/login`
  2. Click the "Sign in for E2E" button
  3. Wait for navigation
- **Expected outcome**:
  - The URL ends with `/bookmarks`
  - **Source of truth**: `auth.rs` line 264 — `test_login` redirects to `/`. The app's root route redirects authenticated users to `/bookmarks`. The existing `suggest.spec.js` already validates this exact flow (line 5-6), serving as a known-good reference.
- **Interactions**: Exercises the full auth flow: session cookie creation, redirect chain (`/` -> `/bookmarks`). Uses E2E auth rather than local auth to avoid needing a pre-seeded user in the database.

## Coverage summary

### Covered

- **Input field visibility** (tests 1, 2): The core bug — dark backgrounds and visible text color. Both computed styles are asserted against specific RGB values derived from the design system.
- **Error display** (test 3): The error message rendering path, ensuring the redesigned template correctly surfaces login failures.
- **Auth redirect flow** (test 4): End-to-end login and redirect, confirming the redesigned page doesn't break the auth flow.

### Explicitly excluded (per agreed Medium Fidelity strategy)

- **Local auth credential flow**: Testing actual local login with email/password would require seeding a user in the E2E database (via `just add-user`). The E2E bootstrap script doesn't currently do this, and adding it would expand scope. The redirect behavior is already covered by the E2E auth button test. **Risk**: A regression in the local auth form's `action` URL or field `name` attributes would not be caught. This risk is low — these are static HTML attributes unlikely to change.
- **Google OAuth button**: Clicking the Google OAuth link would require mocking Google's OAuth flow. The button's presence and styling are implicitly verified by the page rendering without errors. **Risk**: Minimal — the button markup is unchanged from the current template.
- **Responsive layout / mobile viewport**: The strategy didn't include viewport-width testing. **Risk**: Low — the layout uses standard Tailwind responsive utilities (`max-w-sm`, `min-h-screen`, `px-4`).
- **Visual screenshot comparison**: The strategy called for a manual Playwright MCP screenshot verification before committing, not an automated pixel-diff test. **Risk**: Acceptable — the computed style assertions in tests 1-2 provide mechanical regression protection for the specific bug being fixed.
- **Card shadow rendering**: The `shadow-lg shadow-black/20` on the card is a visual enhancement that's difficult to assert meaningfully via computed styles. **Risk**: Negligible — shadow rendering is a Tailwind utility with no interaction surface.
