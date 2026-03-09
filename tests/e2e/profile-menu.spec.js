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
