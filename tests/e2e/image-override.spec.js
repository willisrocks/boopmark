const { test, expect } = require("@playwright/test");
const path = require("path");

async function signIn(page) {
  await page.goto("/");
  await page.getByRole("button", { name: "Sign in for E2E" }).click();
  await expect(page).toHaveURL(/\/bookmarks$/);
}

test("edit modal previews, saves, and removes a cropped image override", async ({ page }) => {
  await signIn(page);

  const unique = Date.now();
  const title = `Image override E2E ${unique}`;
  const originalImage = "http://127.0.0.1:4010/static/social-preview.png";
  const created = await page.request.post("/api/v1/bookmarks", {
    data: {
      url: `https://image-override-e2e.example.com/article-${unique}`,
      title,
      description: "A deterministic image test",
      image_url: originalImage,
      tags: [],
    },
  });
  expect(created.ok()).toBeTruthy();
  await page.reload();

  const card = page.getByTestId("bookmark-card").filter({ hasText: title }).first();
  await expect(card).toBeVisible();
  await card.getByRole("button", { name: "Edit" }).click();

  const modal = page.locator("#edit-modal");
  await expect(modal).toBeVisible();
  const input = modal.getByTestId("image-override-input");
  await input.setInputFiles(path.join(process.cwd(), "static", "social-preview.png"));
  await expect(modal.getByTestId("image-override-preview-image")).toHaveAttribute("src", /^blob:/);

  // The preview is interactive, and the focal point is submitted as normalized
  // coordinates for the authoritative server-side crop.
  await modal.getByTestId("image-override-preview").click({ position: { x: 24, y: 12 } });
  await expect(modal.locator("#image-override-focal-x")).not.toHaveValue("0.5");
  await modal.getByTestId("image-override-save").click();
  await expect(modal.getByTestId("image-override-status")).toHaveText("Image saved");

  const updatedCard = page
    .getByTestId("bookmark-card")
    .filter({ hasText: title })
    .first();
  const image = updatedCard.getByTestId("bookmark-card-image");
  await expect(image).toHaveAttribute("src", /\/uploads\/images\/overrides\/[^/]+\/[^/]+\.jpg$/);
  await expect(image).toHaveJSProperty("naturalWidth", 1200);
  await expect(image).toHaveJSProperty("naturalHeight", 630);

  page.once("dialog", (dialog) => dialog.accept());
  await modal.getByTestId("image-override-remove").click();
  await expect(modal).toBeHidden();
  await expect(updatedCard.getByTestId("bookmark-card-image")).toHaveAttribute("src", originalImage);
});
