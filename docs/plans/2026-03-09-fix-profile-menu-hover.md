# Fix Profile Menu Hover Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Make the profile/settings menu stay available long enough to click its items, and prove it with Playwright end-to-end tests.

**Architecture:** The current menu is controlled entirely by Tailwind `group-hover`, so the panel only exists while the pointer remains over the avatar group. That is brittle and currently broken by the gap between the avatar and the absolutely positioned menu. Replace the hover-only behavior with an explicit disclosure menu driven by a small DOM controller, keep the existing visual layout, and add a real `GET /settings/api-keys` page so the existing menu link can be exercised by E2E instead of navigating to a 404.

**Tech Stack:** Rust, Axum, Askama templates, HTMX, Tailwind CSS, Playwright

---

### Task 1: Add failing Playwright coverage for the profile menu

Write the browser tests first so the implementation is driven by the exact user-visible behavior: the menu must remain visible while moving to a menu item, and both `API Keys` and `Sign Out` must be clickable.

**Files:**
- Create: `tests/e2e/profile-menu.spec.js`
- Modify: `templates/components/header.html`

**Step 1: Write the failing E2E test for the settings link**

Create `tests/e2e/profile-menu.spec.js`:

```js
const { test, expect } = require("@playwright/test");

async function signIn(page) {
  await page.goto("/");
  await page.getByRole("button", { name: "Sign in for E2E" }).click();
  await expect(page).toHaveURL(/\/bookmarks$/);
}

test("profile menu stays open while moving to API Keys and the link is clickable", async ({ page }) => {
  await signIn(page);

  await page.getByTestId("profile-menu-trigger").click();
  const menu = page.getByTestId("profile-menu");
  const apiKeysLink = page.getByTestId("profile-menu-api-keys");

  await expect(menu).toBeVisible();
  await apiKeysLink.hover();
  await expect(menu).toBeVisible();

  await apiKeysLink.click();
  await expect(page).toHaveURL(/\/settings\/api-keys$/);
  await expect(page.getByRole("heading", { name: "API Keys" })).toBeVisible();
});
```

**Step 2: Write the failing E2E test for sign out**

Append a second test in `tests/e2e/profile-menu.spec.js`:

```js
test("profile menu stays open while moving to Sign Out and the button is clickable", async ({ page }) => {
  await signIn(page);

  await page.getByTestId("profile-menu-trigger").click();
  const menu = page.getByTestId("profile-menu");
  const signOutButton = page.getByTestId("profile-menu-sign-out");

  await expect(menu).toBeVisible();
  await signOutButton.hover();
  await expect(menu).toBeVisible();

  await signOutButton.click();
  await expect(page).toHaveURL(/\/auth\/login$/);
  await expect(page.getByRole("button", { name: "Sign in for E2E" })).toBeVisible();
});
```

**Step 3: Add stable test hooks to the header markup**

Update `templates/components/header.html:23-40` so the avatar trigger, menu panel, `API Keys` link, and `Sign Out` button expose deterministic `data-testid` attributes. Do not change behavior yet; only add the selectors the tests need.

Use these IDs in the markup:

```html
data-testid="profile-menu-trigger"
data-testid="profile-menu"
data-testid="profile-menu-api-keys"
data-testid="profile-menu-sign-out"
```

**Step 4: Run the new test file and verify it fails**

Run:

```bash
npx playwright test tests/e2e/profile-menu.spec.js
```

Expected:
- The tests fail against the current implementation.
- The first failure should show that the menu does not stay available while moving to the item, or that `/settings/api-keys` is missing.

**Step 5: Commit the failing tests and selectors scaffold**

```bash
git add tests/e2e/profile-menu.spec.js templates/components/header.html
git commit -m "test: capture profile menu interaction regressions"
```

### Task 2: Add the missing settings page target for the existing API Keys link

The header already advertises `API Keys`, but there is no page route behind it. Land a minimal authenticated page so the menu link can be validated end-to-end and does not send the user to a 404 once the menu bug is fixed.

**Files:**
- Create: `server/src/web/pages/settings.rs`
- Create: `templates/settings/api_keys.html`
- Modify: `server/src/web/pages/mod.rs`

**Step 1: Create the settings page handler**

Create `server/src/web/pages/settings.rs`:

```rust
use askama::Template;
use axum::Router;
use axum::response::{Html, IntoResponse};

use crate::web::extractors::AuthUser;
use crate::web::state::AppState;

#[derive(Template)]
#[template(path = "settings/api_keys.html")]
struct ApiKeysPage {
    email: String,
}

fn render(t: &impl Template) -> axum::response::Response {
    match t.render() {
        Ok(body) => Html(body).into_response(),
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn api_keys_page(AuthUser(user): AuthUser) -> axum::response::Response {
    render(&ApiKeysPage { email: user.email })
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/settings/api-keys", axum::routing::get(api_keys_page))
}
```

**Step 2: Wire the new page routes into the page router**

Update `server/src/web/pages/mod.rs`:

```rust
mod auth;
pub mod bookmarks;
mod settings;

use axum::Router;
use axum::routing::{delete, get, post};

use crate::web::extractors::MaybeUser;
use crate::web::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(home))
        .route("/bookmarks", get(bookmarks::list).post(bookmarks::create))
        .route("/bookmarks/suggest", post(bookmarks::suggest))
        .route("/bookmarks/{id}", delete(bookmarks::delete))
        .merge(auth::routes())
        .merge(settings::routes())
}
```

**Step 3: Create the settings page template**

Create `templates/settings/api_keys.html`:

```html
{% extends "base.html" %}
{% block title %}API Keys - BoopMark{% endblock %}
{% block content %}
<main class="max-w-xl mx-8 py-12">
    <div class="bg-[#1e2235] rounded-xl border border-gray-700 p-8">
        <h1 class="text-xl font-bold mb-2">API Keys</h1>
        <p class="text-sm text-gray-400 mb-4">{{ email }}</p>
        <p class="text-sm text-gray-300">
            API key management will live here. This page exists now so the profile menu target is real and testable.
        </p>
    </div>
</main>
{% endblock %}
```

**Step 4: Build the server to verify the new page compiles**

Run:

```bash
cargo build -p boopmark-server
```

Expected:
- The server builds successfully.
- `GET /settings/api-keys` renders for authenticated users.

**Step 5: Commit the route and template**

```bash
git add server/src/web/pages/mod.rs server/src/web/pages/settings.rs templates/settings/api_keys.html
git commit -m "feat: add api keys settings page"
```

### Task 3: Replace the hover-only profile menu with an explicit disclosure menu

Do not try to patch `group-hover`. The menu should remain open because it has explicit open/close state, not because the pointer never crosses a dead zone.

**Files:**
- Modify: `templates/components/header.html`
- Modify: `templates/base.html`

**Step 1: Convert the avatar into a semantic menu trigger**

Update `templates/components/header.html:23-40` so the profile control becomes a `button` with menu semantics:

```html
<div class="relative" data-profile-menu>
    <button
        type="button"
        data-profile-menu-trigger
        data-testid="profile-menu-trigger"
        aria-haspopup="menu"
        aria-expanded="false"
        aria-controls="profile-menu-panel"
        class="cursor-pointer"
    >
        {% if let Some(img) = user.image %}
        <img src="{{ img }}" class="w-8 h-8 rounded-full" alt="{{ user.display_name }}">
        {% else %}
        <div class="w-8 h-8 rounded-full bg-gray-700 flex items-center justify-center text-sm">
            {{ user.email_initial }}
        </div>
        {% endif %}
    </button>
```

**Step 2: Make the menu panel stateful instead of hover-driven**

Replace the `group-hover:block` panel with a hidden panel controlled by script:

```html
    <div
        id="profile-menu-panel"
        data-profile-menu-panel
        data-testid="profile-menu"
        role="menu"
        class="hidden absolute right-0 top-10 bg-[#1e2235] border border-gray-700 rounded-lg p-3 min-w-[200px] z-50"
    >
        <p class="text-sm text-gray-400">{{ user.display_name }}</p>
        <p class="text-xs text-gray-500 mb-2">{{ user.email }}</p>
        <a
            href="/settings/api-keys"
            data-testid="profile-menu-api-keys"
            class="block text-sm text-gray-300 hover:text-white py-1"
        >
            API Keys
        </a>
        <form method="post" action="/auth/logout">
            <button
                type="submit"
                data-testid="profile-menu-sign-out"
                class="text-sm text-gray-300 hover:text-white py-1"
            >
                Sign Out
            </button>
        </form>
    </div>
</div>
```

**Step 3: Add the menu controller script**

Append a small script in `templates/base.html` just before `</body>` so the menu toggles on click, closes on outside click, and closes on `Escape`:

```html
<script>
document.addEventListener("DOMContentLoaded", () => {
    const root = document.querySelector("[data-profile-menu]");
    if (!root) return;

    const trigger = root.querySelector("[data-profile-menu-trigger]");
    const panel = root.querySelector("[data-profile-menu-panel]");

    const setOpen = (open) => {
        panel.classList.toggle("hidden", !open);
        trigger.setAttribute("aria-expanded", open ? "true" : "false");
    };

    trigger.addEventListener("click", (event) => {
        event.stopPropagation();
        setOpen(panel.classList.contains("hidden"));
    });

    panel.addEventListener("click", (event) => {
        event.stopPropagation();
    });

    document.addEventListener("click", () => {
        setOpen(false);
    });

    document.addEventListener("keydown", (event) => {
        if (event.key === "Escape") {
            setOpen(false);
        }
    });
});
</script>
```

**Step 4: Re-run the profile menu E2E tests**

Run:

```bash
npx playwright test tests/e2e/profile-menu.spec.js
```

Expected:
- Both tests pass.
- Hovering the item after opening the menu no longer hides the panel.
- Clicking `API Keys` navigates to `/settings/api-keys`.
- Clicking `Sign Out` returns to `/auth/login`.

**Step 5: Rebuild checked-in Tailwind output if any new classes were introduced**

Run only if the implementation added any utility class not already present in `static/css/output.css`:

```bash
just css-build
```

Expected:
- `static/css/output.css` reflects any newly introduced utility usage.

**Step 6: Commit the menu fix**

```bash
git add templates/components/header.html templates/base.html static/css/output.css
git commit -m "fix: make profile menu clickable and persistent"
```

### Task 4: Run the regression suite for the changed user flows

The new menu behavior must not break the existing add-bookmark E2E flow, so finish with the smallest useful regression pass.

**Files:**
- Modify: none

**Step 1: Run the targeted server build**

```bash
cargo build -p boopmark-server
```

Expected:
- The server still builds cleanly after the menu and settings changes.

**Step 2: Run the new profile-menu Playwright suite and the existing suggest suite**

```bash
npx playwright test tests/e2e/profile-menu.spec.js tests/e2e/suggest.spec.js
```

Expected:
- `profile-menu.spec.js` passes.
- `suggest.spec.js` still passes, proving the header changes did not break the authenticated bookmark flow.

**Step 3: Commit if the verification pass required any follow-up fixes**

```bash
git add -A
git commit -m "test: verify profile menu regression coverage"
```

