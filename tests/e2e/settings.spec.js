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

  for (const id of ["delete-anthropic-api-key", "delete-openai-api-key"]) {
    const deleteKey = page.getByTestId(id);
    if (await deleteKey.count()) {
      await deleteKey.check();
    }
  }

  const enableLlm = page.getByLabel("Enable LLM integration");
  if (await enableLlm.isChecked()) {
    await enableLlm.uncheck();
  }

  await page
    .getByLabel("Anthropic model")
    .selectOption("claude-haiku-4-5-20251001");
  const metadataProvider = page.getByTestId("metadata-provider");
  if (await metadataProvider.count()) {
    await metadataProvider.selectOption("anthropic");
  }
  const imageEnabled = page.getByTestId("image-enabled");
  if ((await imageEnabled.count()) && (await imageEnabled.isChecked())) {
    await imageEnabled.uncheck();
  }
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

test("settings page supports add and delete key flows", async ({ page }) => {
  const anthropicApiKey = readAnthropicApiKeyFromDotEnv();

  await signIn(page);
  await resetSettings(page);
  await page.goto("/settings");

  // Add a key: fill the input and save
  await page.getByLabel("Enable LLM integration").check();
  await page.getByLabel("Anthropic API key").fill(anthropicApiKey);
  await page.getByLabel("Anthropic model").selectOption("claude-sonnet-4-6");
  await page.getByRole("button", { name: "Save settings" }).click();

  // Verify key-saved state
  await expect(page).toHaveURL(/\/settings\?saved=1$/);
  await expect(page.getByText("Settings saved")).toBeVisible();
  await expect(page.getByTestId("anthropic-api-key-status")).toBeVisible();
  await expect(page.getByText("Anthropic API key saved securely")).toBeVisible();
  await expect(page.getByLabel("Anthropic API key")).toHaveCount(0);
  expect(await page.content()).not.toContain(anthropicApiKey);
  await expect(page.getByTestId("delete-anthropic-api-key")).toBeVisible();
  await expect(page.getByLabel("Anthropic model")).toHaveValue("claude-sonnet-4-6");

  // Delete the key
  await page.getByTestId("delete-anthropic-api-key").check();
  await page.getByRole("button", { name: "Save settings" }).click();

  // Verify back to add-key state
  await expect(page).toHaveURL(/\/settings\?saved=1$/);
  await expect(page.getByText("No Anthropic API key saved yet.")).toBeVisible();
  await expect(page.getByLabel("Anthropic API key")).toBeEditable();
  await expect(page.getByTestId("anthropic-api-key-status")).toHaveCount(0);
  await expect(page.getByTestId("delete-anthropic-api-key")).toHaveCount(0);
});

test("settings stores an OpenAI key without rehydrating the secret", async ({
  page,
}) => {
  await signIn(page);
  await resetSettings(page);
  await page.goto("/settings");

  const openaiApiKey = "sk-openai-settings-e2e";
  await page.getByLabel("Enable LLM integration").check();
  await page.getByTestId("metadata-provider").selectOption("openai");
  await page.getByLabel("OpenAI API key").fill(openaiApiKey);
  await page.getByTestId("openai-model").selectOption("gpt-5.6-sol");
  await page.getByRole("button", { name: "Save settings" }).click();

  await expect(page).toHaveURL(/\/settings\?saved=1$/);
  await expect(page.getByTestId("openai-api-key-status")).toBeVisible();
  await expect(page.getByLabel("OpenAI API key")).toHaveCount(0);
  expect(await page.content()).not.toContain(openaiApiKey);
  await expect(page.getByTestId("delete-openai-api-key")).toBeVisible();
  await expect(page.getByTestId("openai-model")).toHaveValue("gpt-5.6-sol");

  await resetSettings(page);
  await expect(page.getByTestId("openai-api-key-status")).toHaveCount(0);
});

test("settings page shows API Keys section", async ({ page }) => {
  await signIn(page);
  await page.goto("/settings");

  await expect(page.getByRole("heading", { name: "API Keys" })).toBeVisible();
  await expect(
    page.getByText("Create keys to use the Boopmark API and CLI.")
  ).toBeVisible();
  await expect(page.getByTestId("create-api-key-form")).toBeVisible();
});

test("unauthenticated requests cannot read or save settings", async ({ page, request }) => {
  const getResponse = await request.get("/settings");
  expect(getResponse.status()).toBe(401);

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

test("settings exposes provider controls and stable section navigation", async ({
  page,
}) => {
  await signIn(page);
  await resetSettings(page);
  await page.goto("/settings");

  const navigation = page.getByTestId("settings-navigation");
  await expect(navigation).toBeVisible();
  await expect(page.getByTestId("settings-nav-ai-models")).toHaveAttribute(
    "href",
    "#ai-models",
  );
  await expect(page.getByTestId("settings-nav-tag-library")).toHaveAttribute(
    "href",
    "#tag-library",
  );
  await expect(page.getByTestId("settings-nav-images")).toHaveAttribute(
    "href",
    "#images",
  );
  await expect(page.getByTestId("settings-nav-api-access")).toHaveAttribute(
    "href",
    "#api-access",
  );
  await expect(page.getByTestId("settings-nav-import-export")).toHaveAttribute(
    "href",
    "#import-export",
  );

  for (const id of [
    "settings-section-ai-models",
    "settings-section-tag-library",
    "settings-section-images",
    "settings-section-api-access",
    "settings-section-import-export",
  ]) {
    await expect(page.getByTestId(id)).toBeVisible();
  }

  await expect(page.getByTestId("metadata-provider")).toBeVisible();
  await expect(page.locator("#metadata_provider option")).toHaveCount(2);
  await expect(page.locator("#metadata_provider option").nth(0)).toHaveAttribute(
    "value",
    "anthropic",
  );
  await expect(page.locator("#metadata_provider option").nth(1)).toHaveAttribute(
    "value",
    "openai",
  );
  await expect(page.getByTestId("openai-model")).toBeVisible();
  await expect(page.getByTestId("openai-model")).toContainText("GPT-5.6");
  await expect(page.locator("#openai_model option")).toHaveCount(3);
  await expect(page.locator("#openai_model option").nth(0)).toHaveAttribute(
    "value",
    "gpt-5.6-luna",
  );
  await expect(page.locator("#openai_model option").nth(1)).toHaveAttribute(
    "value",
    "gpt-5.6-terra",
  );
  await expect(page.locator("#openai_model option").nth(2)).toHaveAttribute(
    "value",
    "gpt-5.6-sol",
  );
  await expect(page.getByTestId("image-enabled")).toBeVisible();
  await expect(page.getByTestId("image-model")).toHaveValue("gpt-image-2");
  await expect(page.getByTestId("text-provider-status")).toBeVisible();
  await expect(page.getByTestId("image-provider-status")).toBeVisible();
});

test("settings provider controls submit independent text and image choices", async ({
  page,
}) => {
  await signIn(page);
  await page.goto("/settings");

  await page.getByTestId("metadata-provider").selectOption("openai");
  await page.getByTestId("openai-model").selectOption("gpt-5.6-terra");
  await page.getByTestId("image-enabled").check();
  await page.getByTestId("image-model").selectOption("gpt-image-2");

  const payload = await page.locator("#settings-form").evaluate((form) => {
    const values = {};
    for (const [name, value] of new FormData(form)) {
      values[name] = value;
    }
    return values;
  });

  expect(payload.metadata_provider).toBe("openai");
  expect(payload.openai_model).toBe("gpt-5.6-terra");
  expect(payload.image_generation_enabled).toBe("on");
  expect(payload.image_generation_model).toBe("gpt-image-2");
  expect(payload.anthropic_api_key).toBe("");
  expect(payload.openai_api_key).toBe("");
});

test("settings sidebar is sticky on desktop", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await signIn(page);
  await page.goto("/settings");

  const navigation = page.getByTestId("settings-navigation");
  await expect(navigation).toBeVisible();
  await expect(navigation).toHaveCSS("position", "sticky");

  const navigationBox = await navigation.boundingBox();
  const contentBox = await page.getByTestId("settings-section-ai-models").boundingBox();
  if (!navigationBox || !contentBox) {
    throw new Error("expected settings navigation and content bounding boxes");
  }
  expect(contentBox.x).toBeGreaterThan(navigationBox.x + navigationBox.width);
});

test("settings navigation and controls fit a mobile viewport", async ({ page }) => {
  await page.setViewportSize({ width: 375, height: 812 });
  await signIn(page);
  await page.goto("/settings");

  const dimensions = await page.evaluate(() => ({
    viewportWidth: window.innerWidth,
    documentWidth: document.documentElement.scrollWidth,
  }));
  expect(dimensions.documentWidth).toBeLessThanOrEqual(dimensions.viewportWidth);

  const navigation = page.getByTestId("settings-navigation");
  await expect(navigation).toBeVisible();
  await expect(page.getByTestId("settings-nav-images")).toBeVisible();
  await expect(page.getByTestId("metadata-provider")).toBeVisible();
  await expect(page.getByTestId("save-settings-button")).toBeVisible();

  await page.getByTestId("settings-nav-images").click();
  await expect(page).toHaveURL(/\/settings#images$/);
  await expect(page.getByTestId("settings-section-images")).toBeInViewport();
  await expect(page.getByTestId("settings-nav-images")).toHaveAttribute(
    "aria-current",
    "page",
  );
});
