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

  // Task 4: preview image should NOT be visible in modal (removed)
  await expect(page.getByTestId("bookmark-preview-image")).toHaveCount(0);

  await page.getByTestId("bookmark-submit-button").click();
  await expect(modal).toBeHidden();

  // Task 3: Reopen modal and verify fields are cleared
  await page.getByTestId("open-add-bookmark-modal").click();
  await expect(modal).toBeVisible();
  await expect(urlInput).toHaveValue("");
  await expect(titleInput).toHaveValue("");
  await expect(descriptionInput).toHaveValue("");

  // Close modal to check the card
  await page.keyboard.press("Escape");
  const closeBtn = modal.locator("button", { hasText: "×" });
  if (await modal.isVisible()) {
    await closeBtn.click();
  }

  // Task 2: Verify card image is wrapped in a clickable link
  const firstCard = page.getByTestId("bookmark-card").first();
  const imageLink = firstCard.getByTestId("bookmark-card-image-link");
  await expect(imageLink).toHaveAttribute("href", /github\.com/);

  // Existing assertions: card image still shows on homepage
  await expect(firstCard.getByTestId("bookmark-card-image")).toHaveAttribute("src", /\/uploads\/images\//);
  await expect(firstCard.locator("text=🔖")).toHaveCount(0);
});
