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

test("profile menu stays visible while keyboard focus moves into the menu", async ({ page }) => {
  await signIn(page);

  const trigger = page.getByTestId("profile-menu-trigger");
  const menu = page.getByTestId("profile-menu");
  const apiKeysLink = page.getByTestId("profile-menu-api-keys");
  const signOutButton = page.getByTestId("profile-menu-sign-out");

  await trigger.focus();
  await expect(menu).toBeVisible();

  await page.keyboard.press("Tab");
  await expect(apiKeysLink).toBeFocused();
  await expect(menu).toBeVisible();

  await page.keyboard.press("Tab");
  await expect(signOutButton).toBeFocused();
  await expect(menu).toBeVisible();

  await page.keyboard.press("Shift+Tab");
  await expect(apiKeysLink).toBeFocused();
  await expect(menu).toBeVisible();

  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/\/settings\/api-keys$/);
  await expect(page.getByRole("heading", { name: "API Keys" })).toBeVisible();
});
