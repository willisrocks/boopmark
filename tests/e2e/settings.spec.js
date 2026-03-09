const fs = require("node:fs");
const path = require("node:path");
const { test, expect } = require("@playwright/test");

async function signIn(page) {
  await page.goto("/");
  await page.getByRole("button", { name: "Sign in for E2E" }).click();
  await expect(page).toHaveURL(/\/bookmarks$/);
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

test("settings page shows the default Anthropic model and saves llm integration", async ({
  page,
}) => {
  const anthropicApiKey = readAnthropicApiKeyFromDotEnv();

  await signIn(page);
  await page.goto("/settings");

  await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "LLM Integration" })).toBeVisible();
  await expect(page.getByLabel("Enable LLM integration")).not.toBeChecked();
  await expect(page.getByLabel("Anthropic model")).toHaveValue("claude-haiku-4-5");
  await expect(page.getByText("No Anthropic API key saved yet.")).toBeVisible();

  await page.getByLabel("Enable LLM integration").check();
  await page.getByLabel("Anthropic API key").fill(anthropicApiKey);
  await page.getByLabel("Anthropic model").fill("claude-haiku-4-5-20251001");
  await page.getByRole("button", { name: "Save settings" }).click();

  await expect(page).toHaveURL(/\/settings\?saved=1$/);
  await expect(page.getByText("Settings saved")).toBeVisible();
  await expect(page.getByText("Anthropic API key saved")).toBeVisible();
  await expect(page.getByLabel("Anthropic API key")).toHaveValue("");

  await page.reload();
  await expect(page.getByLabel("Enable LLM integration")).toBeChecked();
  await expect(page.getByLabel("Anthropic model")).toHaveValue(
    "claude-haiku-4-5-20251001",
  );
  await expect(page.getByText("Anthropic API key saved")).toBeVisible();
  await expect(page.getByLabel("Anthropic API key")).toHaveValue("");
});

test("settings page can clear a saved anthropic key", async ({ page }) => {
  const anthropicApiKey = readAnthropicApiKeyFromDotEnv();

  await signIn(page);
  await page.goto("/settings");

  await page.getByLabel("Enable LLM integration").check();
  await page.getByLabel("Anthropic API key").fill(anthropicApiKey);
  await page.getByRole("button", { name: "Save settings" }).click();

  await page.getByLabel("Clear saved Anthropic API key").check();
  await page.getByRole("button", { name: "Save settings" }).click();

  await expect(page).toHaveURL(/\/settings\?saved=1$/);
  await expect(page.getByText("No Anthropic API key saved yet.")).toBeVisible();
  await expect(page.getByLabel("Clear saved Anthropic API key")).not.toBeChecked();
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

  const postResponse = await request.post("/settings", {
    form: {
      llm_enabled: "on",
      anthropic_api_key: "sk-ant-test",
      anthropic_model: "claude-haiku-4-5",
    },
  });
  expect(postResponse.status()).toBe(401);

  await page.goto("/settings");
  await expect(page.getByRole("heading", { name: "Settings" })).toHaveCount(0);
});
