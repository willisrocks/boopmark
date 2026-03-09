const { test, expect } = require("@playwright/test");

test("suggest fills metadata on blur and the submitted card shows the stored preview image", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Sign in for E2E" }).click();
  await expect(page).toHaveURL(/\/bookmarks$/);

  await page.getByTestId("open-add-bookmark-modal").click();
  const modal = page.getByTestId("add-bookmark-modal");
  await expect(modal).toBeVisible();

  const urlInput = page.getByTestId("bookmark-url-input");
  const titleInput = page.getByTestId("bookmark-title-input");
  const descriptionInput = page.getByTestId("bookmark-description-input");

  await urlInput.fill("https://github.com/danshapiro/trycycle");
  await urlInput.press("Tab");

  await expect(titleInput).not.toHaveValue("");
  await expect(descriptionInput).not.toHaveValue("");
  await expect(modal).toBeVisible();
  await expect(page.getByTestId("bookmark-preview-image")).toBeVisible();

  await page.getByTestId("bookmark-submit-button").click();
  await expect(modal).toBeHidden();

  const firstCard = page.getByTestId("bookmark-card").first();
  await expect(firstCard.getByTestId("bookmark-card-image")).toHaveAttribute("src", /\/uploads\/images\//);
  await expect(firstCard.locator("text=🔖")).toHaveCount(0);
});
