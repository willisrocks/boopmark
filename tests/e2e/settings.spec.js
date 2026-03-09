const fs = require("node:fs");
const path = require("node:path");
const { test, expect } = require("@playwright/test");

async function signIn(page) {
  await page.goto("/");
  await page.getByRole("button", { name: "Sign in for E2E" }).click();
  await expect(page).toHaveURL(/\/bookmarks$/);
}

async function resetSettings(page) {
  await page.goto("/settings");

  const clearSavedKey = page.getByLabel("Clear saved key");
  if (await clearSavedKey.count()) {
    await clearSavedKey.check();
  }

  const enableLlm = page.getByLabel("Enable LLM integration");
  if (await enableLlm.isChecked()) {
    await enableLlm.uncheck();
  }

  await page
    .getByLabel("Anthropic model")
    .selectOption("claude-haiku-4-5-20251001");
  await page.getByRole("button", { name: "Save settings" }).click();
  await expect(page).toHaveURL(/\/settings\?saved=1$/);
}

function readAnthropicApiKeyFromDotEnv() {
  const envPath = path.resolve(__dirname, "..", "..", ".env");
  const contents = fs.readFileSync(envPath, "utf8");
  const match = contents.match(/^ANTHROPIC_API_KEY=(.+)$/m);
  if (!match || !match[1].trim()) {
    throw new Error("ANTHROPIC_API_KEY must exist in the copied worktree .env");
  }

  return match[1].trim();
}

test("settings page renders in the app shell with the official default model", async ({
  page,
}) => {
  await signIn(page);
  await resetSettings(page);
  await page.goto("/settings");

  await expect(page.getByRole("banner")).toBeVisible();
  await expect(page.getByRole("link", { name: "BoopMark" })).toHaveAttribute(
    "href",
    "/bookmarks",
  );
  await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "LLM Integration" })).toBeVisible();
  await expect(page.getByLabel("Enable LLM integration")).not.toBeChecked();
  await expect(page.getByText("No Anthropic API key saved yet.")).toBeVisible();
  await expect(page.getByLabel("Anthropic API key")).toBeEditable();
  await expect(page.getByLabel("Anthropic model")).toHaveValue(
    "claude-haiku-4-5-20251001",
  );
  await expect(page.locator("#anthropic_model option").nth(0)).toHaveAttribute(
    "value",
    "claude-opus-4-6",
  );
  await expect(page.locator("#anthropic_model option").nth(1)).toHaveAttribute(
    "value",
    "claude-sonnet-4-6",
  );
  await expect(page.locator("#anthropic_model option").nth(2)).toHaveAttribute(
    "value",
    "claude-haiku-4-5-20251001",
  );
  // Normal official-only path after resetSettings; preserved legacy values are covered in unit tests.
  await expect(page.locator("#anthropic_model option")).toHaveCount(3);
});

test("settings page uses explicit keep replace and clear flows for saved Anthropic keys", async ({
  page,
}) => {
  const anthropicApiKey = readAnthropicApiKeyFromDotEnv();

  await signIn(page);
  await resetSettings(page);
  await page.goto("/settings");

  await page.getByLabel("Enable LLM integration").check();
  await page.getByLabel("Anthropic API key").fill(anthropicApiKey);
  await page.getByLabel("Anthropic model").selectOption("claude-sonnet-4-6");
  await page.getByRole("button", { name: "Save settings" }).click();

  await expect(page).toHaveURL(/\/settings\?saved=1$/);
  await expect(page.getByText("Settings saved")).toBeVisible();
  await expect(page.getByTestId("anthropic-api-key-status")).toBeVisible();
  await expect(page.getByText("Anthropic API key saved securely")).toBeVisible();
  await expect(page.getByLabel("Anthropic API key")).toHaveCount(0);
  await expect(page.getByLabel("Keep current saved key")).toBeChecked();
  await expect(page.getByLabel("Anthropic model")).toHaveValue("claude-sonnet-4-6");

  await page.getByLabel("Replace saved key").check();
  await expect(page.getByTestId("anthropic-api-key-replacement")).toBeVisible();
  await page.getByLabel("Replacement Anthropic API key").fill(anthropicApiKey);
  await page.getByLabel("Clear saved key").check();
  await expect(page.getByTestId("anthropic-api-key-replacement")).toHaveCount(0);
  await expect(
    page.getByText("Saving will remove the stored Anthropic API key."),
  ).toBeVisible();
  await page.getByRole("button", { name: "Save settings" }).click();

  await expect(page.getByText("No Anthropic API key saved yet.")).toBeVisible();
  await expect(page.getByLabel("Anthropic API key")).toBeEditable();
});

test("legacy api keys route redirects to settings", async ({ page }) => {
  await signIn(page);

  await page.goto("/settings/api-keys");

  await expect(page).toHaveURL(/\/settings$/);
  await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "LLM Integration" })).toBeVisible();
});

test("unauthenticated requests cannot read or save settings", async ({ page, request }) => {
  const getResponse = await request.get("/settings");
  expect(getResponse.status()).toBe(401);

  const legacyResponse = await request.get("/settings/api-keys", {
    maxRedirects: 0,
  });
  expect(legacyResponse.status()).toBe(401);

  const postResponse = await request.post("/settings", {
    form: {
      llm_enabled: "on",
      anthropic_api_key: "sk-ant-test",
      anthropic_model: "claude-haiku-4-5-20251001",
    },
  });
  expect(postResponse.status()).toBe(401);

  await page.goto("/settings");
  await expect(page.getByRole("heading", { name: "Settings" })).toHaveCount(0);
  await page.goto("/settings/api-keys");
  await expect(page).not.toHaveURL(/\/settings$/);
});

test("settings rejects forged unsupported anthropic model submissions with 400", async ({
  page,
}) => {
  await signIn(page);

  const status = await page.evaluate(async () => {
    const response = await fetch("/settings", {
      method: "POST",
      headers: {
        "Content-Type": "application/x-www-form-urlencoded",
      },
      body: new URLSearchParams({
        llm_enabled: "on",
        anthropic_model: "claude-3-7-sonnet-latest",
      }),
    });
    return response.status;
  });

  expect(status).toBe(400);
});
