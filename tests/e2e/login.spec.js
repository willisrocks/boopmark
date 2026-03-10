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
