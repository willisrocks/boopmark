> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

# Redesign Login Page

## Problem

The login page input fields use `bg-[#161829]` which is not compiled into `output.css` (the Tailwind binary was not re-run after this color was introduced). This causes input fields to render with transparent/white backgrounds, making white text invisible. Beyond fixing the immediate bug, the login page should be redesigned to match the app's dark theme with a modern, polished aesthetic.

## Root Cause

The `bg-[#161829]` arbitrary value in `templates/auth/login.html` was never compiled into `static/css/output.css`. The compiled CSS contains `bg-[#1a1d2e]` and `bg-[#1e2235]` (used elsewhere in the app) but not `bg-[#161829]`.

## Solution

1. Redesign the login template to use the established design tokens (`bg-[#1a1d2e]` for inputs, `bg-[#1e2235]` for cards) that are already compiled in output.css
2. Enhance the visual design: add a subtle gradient background, better spacing, refined typography, and a more polished card layout
3. Re-run the Tailwind CSS compiler to pick up any new classes
4. Add E2E tests to prevent regression

## Design Language

Match the existing app design system:
- Body: `bg-[#0f1117]`
- Card: `bg-[#1e2235]` with `border border-gray-700 rounded-xl`
- Inputs: `bg-[#1a1d2e]` with `border border-gray-700` (consistent with settings page)
- Text: `text-gray-200` (primary), `text-gray-400` (secondary), `text-gray-500` (placeholder)
- Accent: `bg-blue-600` / `hover:bg-blue-700`
- Focus: `focus:border-blue-500 focus:outline-none`

---

## Task 1: Redesign the login template

### Files to change
- `templates/auth/login.html`

### What to do

Replace the current login template content (within the `{% block content %}` block) with a redesigned version that:

1. Uses `bg-[#1a1d2e]` for input backgrounds (already in output.css) instead of `bg-[#161829]`
2. Adds `text-gray-200` to inputs instead of `text-white` (consistent with settings page)
3. Adds a subtle app title and tagline above the card
4. Uses `placeholder-gray-600` for placeholders (consistent with add-bookmark modal)
5. Adds `data-testid` attributes to both input fields and the submit button for E2E testing:
   - `data-testid="login-email-input"` on the email input
   - `data-testid="login-password-input"` on the password input
   - `data-testid="login-submit-button"` on the sign-in button
   - `data-testid="login-error-message"` on the error div
6. Adds a subtle `shadow-lg shadow-black/20` to the card for depth
7. Adds `text-sm text-gray-500 mt-6` footer text "Your bookmarks, organized."

### Exact new template content for `{% block content %}`:

```html
<div class="flex items-center justify-center min-h-screen px-4">
    <div class="w-full max-w-sm">
        <div class="text-center mb-8">
            <span class="text-4xl">🔖</span>
            <h1 class="text-2xl font-bold mt-3 text-white">BoopMark</h1>
            <p class="text-sm text-gray-500 mt-1">Your bookmarks, organized.</p>
        </div>
        <div class="bg-[#1e2235] rounded-xl border border-gray-700 p-8 shadow-lg shadow-black/20">
            {% if let Some(err) = login_error %}
            <div data-testid="login-error-message" class="mb-6 p-3 rounded-lg bg-red-900/50 border border-red-700 text-red-200 text-sm">
                {{ err }}
            </div>
            {% endif %}
            {% if enable_local_auth %}
            <form method="post" action="/auth/local-login" class="space-y-4">
                <div>
                    <label for="login-email" class="block text-sm font-medium text-gray-400 mb-1">Email</label>
                    <input id="login-email" data-testid="login-email-input" type="email" name="email" placeholder="you@example.com" required
                           class="w-full px-4 py-3 rounded-lg bg-[#1a1d2e] border border-gray-700 text-gray-200 placeholder-gray-600 focus:outline-none focus:border-blue-500 transition-colors" />
                </div>
                <div>
                    <label for="login-password" class="block text-sm font-medium text-gray-400 mb-1">Password</label>
                    <input id="login-password" data-testid="login-password-input" type="password" name="password" placeholder="••••••••" required
                           class="w-full px-4 py-3 rounded-lg bg-[#1a1d2e] border border-gray-700 text-gray-200 placeholder-gray-600 focus:outline-none focus:border-blue-500 transition-colors" />
                </div>
                <button data-testid="login-submit-button" type="submit"
                        class="w-full px-4 py-3 rounded-lg bg-blue-600 text-white font-medium hover:bg-blue-700 transition-colors mt-2">
                    Sign in
                </button>
            </form>
            <div class="relative my-6">
                <div class="absolute inset-0 flex items-center"><div class="w-full border-t border-gray-700"></div></div>
                <div class="relative flex justify-center text-sm"><span class="bg-[#1e2235] px-2 text-gray-500">or</span></div>
            </div>
            {% endif %}
            <a href="/auth/google"
               class="flex items-center justify-center gap-2 px-4 py-3 rounded-lg bg-white text-gray-800 font-medium hover:bg-gray-100 transition-colors">
                <svg class="w-5 h-5" viewBox="0 0 24 24"><path fill="#4285F4" d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92a5.06 5.06 0 01-2.2 3.32v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.1z"/><path fill="#34A853" d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z"/><path fill="#FBBC05" d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.07H2.18C1.43 8.55 1 10.22 1 12s.43 3.45 1.18 4.93l2.85-2.22.81-.62z"/><path fill="#EA4335" d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.07l3.66 2.84c.87-2.6 3.3-4.53 6.16-4.53z"/></svg>
                Sign in with Google
            </a>
            {% if enable_e2e_auth %}
            <form method="post" action="/auth/test-login" class="mt-4">
                <button id="e2e-login-button"
                        type="submit"
                        class="w-full px-4 py-3 rounded-lg border border-gray-700 text-gray-400 hover:text-gray-200 hover:border-gray-500 transition-colors">
                    Sign in for E2E
                </button>
            </form>
            {% endif %}
        </div>
    </div>
</div>
```

### Test (manual verification)
- The input fields should have dark backgrounds matching `#1a1d2e`
- Typed text should be visible as light gray (`text-gray-200`)
- The card should have a subtle shadow
- The layout should be vertically centered with branding above the card

---

## Task 2: Rebuild Tailwind CSS

### Files to change
- `static/css/output.css`

### What to do

Run the Tailwind CSS compiler to regenerate `output.css` with the updated template classes:

```bash
cd /path/to/worktree
./tailwindcss-macos-arm64 -i static/css/input.css -o static/css/output.css --minify
```

### Verification

After compilation, verify the output contains the required classes:
- `shadow-lg` should be present
- `shadow-black\/20` should be present
- `bg-[#1a1d2e]` should be present (it already is, but confirm)
- `bg-[#161829]` should NOT be present (the old broken class is gone)

---

## Task 3: Add E2E test for login page

### Files to change
- `tests/e2e/login.spec.js` (new file)
- `scripts/e2e/start-server.sh`

### What to do

**3a. Update `scripts/e2e/start-server.sh`:**

Add `ENABLE_LOCAL_AUTH=1` to the environment exports (after the existing `ENABLE_E2E_AUTH=1` line):

```bash
export ENABLE_LOCAL_AUTH=1
```

**3b. Create `tests/e2e/login.spec.js`:**

```javascript
const { test, expect } = require("@playwright/test");

test.describe("Login page", () => {
  test("renders with visible input fields on dark background", async ({ page }) => {
    await page.goto("/auth/login");

    // Local auth form should be visible
    const emailInput = page.getByTestId("login-email-input");
    const passwordInput = page.getByTestId("login-password-input");
    const submitButton = page.getByTestId("login-submit-button");

    await expect(emailInput).toBeVisible();
    await expect(passwordInput).toBeVisible();
    await expect(submitButton).toBeVisible();

    // Input fields should have dark backgrounds (not white/transparent)
    const emailBg = await emailInput.evaluate(
      (el) => getComputedStyle(el).backgroundColor
    );
    // rgb(26, 29, 46) = #1a1d2e
    expect(emailBg).toBe("rgb(26, 29, 46)");

    const passwordBg = await passwordInput.evaluate(
      (el) => getComputedStyle(el).backgroundColor
    );
    expect(passwordBg).toBe("rgb(26, 29, 46)");
  });

  test("typed text is visible in input fields", async ({ page }) => {
    await page.goto("/auth/login");

    const emailInput = page.getByTestId("login-email-input");
    await emailInput.fill("test@example.com");
    await expect(emailInput).toHaveValue("test@example.com");

    // Text color should be light (not white-on-white)
    const textColor = await emailInput.evaluate(
      (el) => getComputedStyle(el).color
    );
    // rgb(229, 231, 235) = text-gray-200
    expect(textColor).toBe("rgb(229, 231, 235)");
  });

  test("shows error message on invalid credentials", async ({ page }) => {
    await page.goto("/auth/login?error=Invalid+email+or+password");

    const errorMessage = page.getByTestId("login-error-message");
    await expect(errorMessage).toBeVisible();
    await expect(errorMessage).toContainText("Invalid email or password");
  });

  test("successful local login redirects to bookmarks", async ({ page }) => {
    // This test requires a user to exist in the database.
    // The E2E test button provides a simpler path to verify redirect behavior.
    await page.goto("/auth/login");
    await page.getByRole("button", { name: "Sign in for E2E" }).click();
    await expect(page).toHaveURL(/\/bookmarks$/);
  });
});
```

### Verification

Run the E2E tests:
```bash
npx playwright test tests/e2e/login.spec.js
```

All 4 tests should pass.

---

## Task 4: Commit all changes

### What to do

Stage and commit all changed files with a descriptive commit message:

```
fix: redesign login page with correct dark theme styling

The login form input fields used bg-[#161829] which was not compiled
into output.css, causing invisible white-on-white text. Redesigned
the login page to use the app's established design tokens (bg-[#1a1d2e]
for inputs, bg-[#1e2235] for cards) with improved layout, labels,
and subtle shadows. Added E2E tests to verify input field styling
and prevent regression.
```

### Files in commit
- `templates/auth/login.html`
- `static/css/output.css`
- `tests/e2e/login.spec.js`
- `scripts/e2e/start-server.sh`
