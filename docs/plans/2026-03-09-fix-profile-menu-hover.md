# Fix Profile Menu Hover Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use trycycle-executing to implement this plan task-by-task.

**Goal:** Make the profile/settings menu stay open while the pointer moves from the avatar into the menu, and prove that both menu items are clickable with Playwright end-to-end tests.

**Architecture:** The current menu is controlled entirely by Tailwind `group-hover`, and the panel is rendered with a vertical gap below the 32px avatar (`top-10`), which creates a dead zone while the pointer moves toward the menu. Fix the actual hover path first: anchor the menu flush to the trigger, keep it open for focus navigation with `group-focus-within`, and only add JavaScript if the markup alone cannot satisfy the failing gap-crossing test. Add a real `GET /settings/api-keys` page after the hover regression is covered so the existing menu link can be exercised end-to-end instead of navigating to a 404.

**Tech Stack:** Rust, Axum, Askama templates, HTMX, Tailwind CSS, Playwright

---

### Task 1: Add failing Playwright coverage that reproduces the real hover dead zone

Write the browser test first so it reproduces the current bug on today's code: the menu opens on avatar hover, disappears while the pointer crosses the gap, and prevents clicking a real menu item.

**Files:**
- Create: `tests/e2e/profile-menu.spec.js`
- Modify: `templates/components/header.html`

**Step 1: Write a mouse-movement helper that crosses the trigger-to-menu gap**

Create `tests/e2e/profile-menu.spec.js`:

```js
const { test, expect } = require("@playwright/test");

async function signIn(page) {
  await page.goto("/");
  await page.getByRole("button", { name: "Sign in for E2E" }).click();
  await expect(page).toHaveURL(/\/bookmarks$/);
}

function center(box) {
  return {
    x: box.x + box.width / 2,
    y: box.y + box.height / 2,
  };
}

async function moveMouseInSteps(page, from, to, steps = 16) {
  for (let i = 0; i <= steps; i += 1) {
    const progress = i / steps;
    await page.mouse.move(
      from.x + (to.x - from.x) * progress,
      from.y + (to.y - from.y) * progress,
    );
  }
}
```

**Step 2: Write the failing E2E test against the existing `Sign Out` item**

Append the first test in `tests/e2e/profile-menu.spec.js`:

```js
test("profile menu stays visible while the pointer crosses into Sign Out", async ({ page }) => {
  await signIn(page);

  const trigger = page.getByTestId("profile-menu-trigger");
  const menu = page.getByTestId("profile-menu");
  const signOutButton = page.getByTestId("profile-menu-sign-out");

  await trigger.hover();
  await expect(menu).toBeVisible();

  const triggerBox = await trigger.boundingBox();
  const signOutBox = await signOutButton.boundingBox();
  if (!triggerBox || !signOutBox) {
    throw new Error("expected trigger and sign-out button to have bounding boxes");
  }

  await moveMouseInSteps(page, center(triggerBox), center(signOutBox));
  await expect(menu).toBeVisible();

  await signOutButton.click();
  await expect(page).toHaveURL(/\/auth\/login$/);
  await expect(page.getByRole("button", { name: "Sign in for E2E" })).toBeVisible();
});
```

**Step 3: Add stable test hooks to the current header markup**

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
- The failure should happen before sign out completes, because the menu disappears during the stepped pointer movement across the current gap.

**Step 5: Commit the failing hover-regression test and selectors scaffold**

```bash
git add tests/e2e/profile-menu.spec.js templates/components/header.html
git commit -m "test: capture profile menu interaction regressions"
```

### Task 2: Remove the dead zone from the profile menu without changing the user interaction model

Do not switch the feature to click-only. Keep the menu discoverable on hover, make the pointer path continuous, and keep the panel visible while focus moves into its contents.

**Files:**
- Modify: `templates/components/header.html`
- Modify: `static/css/output.css`

**Step 1: Convert the avatar into a focusable trigger**

Update `templates/components/header.html:23-40` so the trigger is a `button`, not a bare `img` or `div`, and so the menu can stay visible while focus moves into the panel:

```html
<div class="relative group">
    <button
        type="button"
        data-testid="profile-menu-trigger"
        aria-haspopup="menu"
        class="block cursor-pointer"
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

**Step 2: Anchor the panel directly below the trigger and expose it on focus**

Replace the current `top-10` hover panel with one that sits flush under the trigger and stays visible on both hover and focus:

```html
    <div
        data-testid="profile-menu"
        role="menu"
        style="top: 100%;"
        class="hidden group-hover:block group-focus-within:block absolute right-0 bg-[#1e2235] border border-gray-700 rounded-lg p-3 min-w-[200px] z-50"
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

`style="top: 100%;"` is deliberate here: the panel must start exactly at the trigger edge so the stepped mouse path in Task 1 has no dead zone to cross after the fix.

**Step 3: Rebuild Tailwind so `group-focus-within:block` exists in the checked-in CSS**

Run:

```bash
just css-build
```

Expected:
- `static/css/output.css` contains the `group-focus-within:block` utility.
- No unrelated CSS changes are introduced.

**Step 4: Re-run the hover regression test**

Run:

```bash
npx playwright test tests/e2e/profile-menu.spec.js --grep "Sign Out"
```

Expected:
- The stepped mouse movement no longer collapses the menu.
- The `Sign Out` flow completes successfully.

**Step 5: Commit the hover fix**

```bash
git add templates/components/header.html static/css/output.css
git commit -m "fix: remove profile menu hover dead zone"
```

### Task 3: Add the missing settings page target and extend the E2E coverage to API Keys

Once the real hover bug is covered and fixed, add the missing destination page for the existing `API Keys` item and reuse the same stepped pointer movement to prove that link is clickable too.

**Files:**
- Create: `server/src/web/pages/settings.rs`
- Create: `templates/settings/api_keys.html`
- Modify: `server/src/web/pages/mod.rs`
- Modify: `tests/e2e/profile-menu.spec.js`

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

**Step 4: Extend the Playwright file with the API Keys navigation test**

Append a second test to `tests/e2e/profile-menu.spec.js` that uses the same stepped mouse movement helper:

```js
test("profile menu stays visible while the pointer crosses into API Keys", async ({ page }) => {
  await signIn(page);

  const trigger = page.getByTestId("profile-menu-trigger");
  const menu = page.getByTestId("profile-menu");
  const apiKeysLink = page.getByTestId("profile-menu-api-keys");

  await trigger.hover();
  await expect(menu).toBeVisible();

  const triggerBox = await trigger.boundingBox();
  const apiKeysBox = await apiKeysLink.boundingBox();
  if (!triggerBox || !apiKeysBox) {
    throw new Error("expected trigger and api keys link to have bounding boxes");
  }

  await moveMouseInSteps(page, center(triggerBox), center(apiKeysBox));
  await expect(menu).toBeVisible();

  await apiKeysLink.click();
  await expect(page).toHaveURL(/\/settings\/api-keys$/);
  await expect(page.getByRole("heading", { name: "API Keys" })).toBeVisible();
});
```

**Step 5: Build the server and run the full profile menu E2E file**

Run:

```bash
cargo build -p boopmark-server
npx playwright test tests/e2e/profile-menu.spec.js
```

Expected:
- The server builds successfully.
- Both menu-item tests pass against real pointer movement.
- `GET /settings/api-keys` renders for authenticated users.

**Step 6: Commit the settings route and API Keys regression**

```bash
git add server/src/web/pages/mod.rs server/src/web/pages/settings.rs templates/settings/api_keys.html tests/e2e/profile-menu.spec.js
git commit -m "feat: add api keys settings page"
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
